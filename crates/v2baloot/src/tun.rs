//! TUN mode.
//!
//! The user daemon writes the chosen server to `~/.local/state/v2baloot/tun-request.json` and
//! starts `v2baloot-tun@$USER.service`. That root unit runs `v2baloot tun-run`, which:
//!
//! * takes a lock so only one TUN session exists system-wide,
//! * reads the request defensively (no symlinks, regular file owned by that user, size-capped)
//!   and rebuilds the Xray config itself — the user only supplies a server, never a raw config,
//! * runs Xray as `nobody` with just CAP_NET_ADMIN/CAP_NET_RAW, config passed on stdin, and with
//!   no SOCKS/HTTP inbounds (only the TUN and the local stats API),
//! * sets up policy routing:
//!
//! ```text
//! pref 9000  fwmark 255 lookup main                        # Xray's own (marked) traffic leaves normally
//! pref 9001  lookup main suppress_prefixlength 0           # LAN / more-specific routes still win
//! pref 9002  lookup 2022                                   # everything else -> default dev v2tun0
//! ```
//!
//! DNS for the TUN link is set with resolvectl (`~.` = all domains); Xray hijacks port 53 on the
//! TUN and answers through its DNS module over the proxy.

use std::ffi::{CStr, CString};
use std::io::Read;
use std::net::IpAddr;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{info, warn};

use v2baloot_core::paths::Paths;
use v2baloot_core::store::{write_atomic, Settings};
use v2baloot_core::xray::{self, XrayOptions, TUN_MARK, TUN_NAME};
use v2baloot_core::{SecurityKind, Server};

const TABLE: &str = "2022";
const PREFS: [&str; 3] = ["9000", "9001", "9002"];
const TUN_ADDR4: &str = "172.19.0.1/30";
const TUN_ADDR6: &str = "fdfe:dcba:9876::1/126";
const LOG_LEVELS: &[&str] = &["debug", "info", "warning", "error", "none"];
const RUN_DIR: &str = "/run/v2baloot";
const MAX_REQUEST: u64 = 1 << 20;

/// What the user daemon hands to the root helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunRequest {
    pub server: Server,
    pub api_port: u16,
    pub dns: Vec<String>,
    pub tun_dns: String,
    pub bypass_private: bool,
    pub log_level: String,
    #[serde(default = "default_idle")]
    pub conn_idle_secs: u32,
}

fn default_idle() -> u32 {
    30
}

struct Passwd {
    name: String,
    uid: u32,
    gid: u32,
    home: PathBuf,
}

/// SAFETY: getpw* return pointers to static storage (or null); we copy out immediately.
unsafe fn passwd_from(pw: *const libc::passwd) -> Option<Passwd> {
    if pw.is_null() {
        return None;
    }
    let pw = &*pw;
    Some(Passwd {
        name: CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned(),
        uid: pw.pw_uid,
        gid: pw.pw_gid,
        home: PathBuf::from(CStr::from_ptr(pw.pw_dir).to_string_lossy().into_owned()),
    })
}

fn passwd_by_uid(uid: u32) -> Option<Passwd> {
    unsafe { passwd_from(libc::getpwuid(uid)) }
}

fn passwd_by_name(name: &str) -> Option<Passwd> {
    let c = CString::new(name).ok()?;
    unsafe { passwd_from(libc::getpwnam(c.as_ptr())) }
}

fn valid_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 32
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
}

fn me() -> Result<Passwd> {
    passwd_by_uid(unsafe { libc::getuid() })
        .filter(|p| valid_user(&p.name))
        .context("cannot determine the current user")
}

fn unit_name() -> Result<String> {
    Ok(format!("v2baloot-tun@{}.service", me()?.name))
}

// ----------------------------------------------------------------------- user daemon side

/// Point `server` at an already-resolved IP while keeping the TLS/Host semantics the domain gave
/// it. Mirrors the SNI fallback in `xray.rs` (sni → host → address) so Proxy and TUN mode do the
/// same handshake.
pub fn pin_to_ip(server: &Server, ip: IpAddr) -> Server {
    let mut s = server.clone();
    if s.address.parse::<IpAddr>().is_ok() {
        return s;
    }
    let domain = std::mem::replace(&mut s.address, ip.to_string());
    let host = s.transport.host.clone().filter(|h| !h.contains(','));
    if s.security.kind != SecurityKind::None && s.security.sni.is_none() {
        s.security.sni = Some(host.clone().unwrap_or_else(|| domain.clone()));
    }
    if s.transport.host.is_none() && s.security.sni.is_none() {
        s.transport.host = Some(domain);
    }
    s
}

/// Resolve the server's domain while routing is still normal: once TUN is up, resolving it would
/// need the tunnel that depends on it.
async fn resolve(server: &Server) -> Result<Server> {
    if let Ok(ip) = server.address.parse::<IpAddr>() {
        return Ok(pin_to_ip(server, ip));
    }
    let domain = server.address.clone();
    let addrs: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((domain.as_str(), server.port)),
    )
    .await
    .with_context(|| format!("resolving {domain} timed out"))?
    .with_context(|| format!("resolving {domain}"))?
    .collect();
    let ip = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.first())
        .with_context(|| format!("{domain} has no addresses"))?
        .ip();
    Ok(pin_to_ip(server, ip))
}

async fn routes_ready() -> bool {
    match Command::new("ip")
        .args(["rule", "list", "pref", PREFS[2]])
        .output()
        .await
    {
        Ok(o) => o.status.success() && !o.stdout.trim_ascii().is_empty(),
        Err(_) => false,
    }
}

pub async fn start(settings: &Settings, server: &Server) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("TUN mode is only supported on Linux");
    }
    let me = me()?;
    let server = resolve(server).await?;
    let req = TunRequest {
        server,
        api_port: settings.api_port,
        dns: settings.dns.clone(),
        tun_dns: settings.tun_dns.clone(),
        bypass_private: settings.bypass_private,
        log_level: settings.log_level.clone(),
        conn_idle_secs: settings.conn_idle_secs,
    };
    // Always the default XDG location: the root helper can't see our environment.
    write_atomic(
        &Paths::for_home(&me.home).tun_request(),
        &serde_json::to_vec(&req)?,
        true,
    )?;

    let unit = unit_name()?;
    wait_inactive(&unit).await;
    let out = Command::new("systemctl")
        .args(["start", "--no-ask-password", &unit])
        .output()
        .await
        .context("running systemctl")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("authentication") || err.contains("Access denied") {
            bail!("not allowed to start {unit}: {err}. You must be in the `wheel` group with the v2baloot polkit rule installed, or run `sudo systemctl start {unit}`");
        }
        bail!("starting {unit} failed: {err}");
    }

    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if !is_active().await {
            bail!("{unit} stopped while starting — check `journalctl -u {unit} -n 30`");
        }
        if routes_ready().await {
            return Ok(());
        }
        if Instant::now() > deadline {
            stop().await;
            bail!("TUN session did not come up in time — check `journalctl -u {unit} -n 30`");
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Ask systemd to stop the TUN unit without waiting for the teardown; `start` waits for the unit
/// to be fully inactive before starting it again.
pub async fn stop() {
    if !cfg!(target_os = "linux") {
        return;
    }
    if let Ok(unit) = unit_name() {
        let _ = Command::new("systemctl")
            .args(["stop", "--no-block", "--no-ask-password", &unit])
            .status()
            .await;
    }
}

/// `ActiveState` of the TUN unit (active, activating, deactivating, inactive, failed).
async fn unit_state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["show", "-p", "ActiveState", "--value", unit])
        .output()
        .await
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Wait for a previous (non-blocking) stop to finish so stale routes can't look "ready".
async fn wait_inactive(unit: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let st = unit_state(unit).await;
        if st == "inactive" || st == "failed" || st.is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub async fn is_active() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Ok(unit) = unit_name() else { return false };
    Command::new("systemctl")
        .args(["is-active", "--quiet", &unit])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

// ----------------------------------------------------------------------- root helper side

async fn cmd(program: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(program)
        .args(args)
        .output()
        .await
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        bail!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

async fn cmd_quiet(program: &str, args: &[&str]) {
    if let Err(e) = cmd(program, args).await {
        info!("(ignored) {e:#}");
    }
}

/// Exclusive, non-blocking lock so only one TUN session (one user) exists at a time.
fn take_lock() -> Result<Option<std::fs::File>> {
    std::fs::create_dir_all(RUN_DIR)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(Path::new(RUN_DIR).join("lock"))?;
    use std::os::fd::AsRawFd;
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(Some(f))
    } else {
        Ok(None)
    }
}

/// Read the user's request without following symlinks, blocking on FIFOs or reading unbounded.
fn read_request(path: &Path, owner: u32) -> Result<TunRequest> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let meta = f.metadata()?;
    if !meta.is_file() || meta.uid() != owner {
        bail!(
            "{} must be a regular file owned by the requesting user",
            path.display()
        );
    }
    let mut buf = Vec::new();
    (&mut f).take(MAX_REQUEST).read_to_end(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|_| anyhow::anyhow!("invalid TUN request"))
}

fn validate(req: &TunRequest) -> Result<()> {
    if req.server.address.parse::<IpAddr>().is_err() {
        bail!("server address must be an IP (the daemon resolves it first)");
    }
    if req.server.port == 0 || req.api_port < 1024 {
        bail!("invalid ports in TUN request");
    }
    if req.dns.is_empty()
        || req
            .dns
            .iter()
            .any(|d| d.trim().is_empty() || d.contains("localhost") || d.contains("local"))
    {
        bail!("DNS servers must be remote resolvers (local ones would loop through the tunnel)");
    }
    Ok(())
}

/// `v2baloot tun-run --user NAME [--cleanup]`, run as root by `v2baloot-tun@.service`.
pub async fn run_helper(user: &str, cleanup_only: bool) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        bail!("tun-run must run as root (it is started by v2baloot-tun@.service)");
    }
    if !valid_user(user) {
        bail!("invalid user name");
    }
    let Some(_lock) = take_lock()? else {
        if cleanup_only {
            // Another instance owns the routes; leave them alone.
            return Ok(());
        }
        bail!("TUN mode is already active (for this or another user)");
    };
    let clean_marker = Path::new(RUN_DIR).join("clean");
    if cleanup_only {
        // ExecStopPost: nothing to do if the helper already cleaned up on a normal stop.
        if std::fs::remove_file(&clean_marker).is_err() {
            cleanup().await;
        }
        return Ok(());
    }
    let _ = std::fs::remove_file(&clean_marker);
    cleanup().await;

    let pw = passwd_by_name(user).with_context(|| format!("user {user} not found"))?;
    let req = read_request(&Paths::for_home(&pw.home).tun_request(), pw.uid)?;
    validate(&req)?;

    let opts = XrayOptions {
        listen: "127.0.0.1".into(),
        socks_port: 0,
        http_port: 0,
        api_port: Some(req.api_port),
        log_level: if LOG_LEVELS.contains(&req.log_level.as_str()) {
            req.log_level.clone()
        } else {
            "warning".into()
        },
        dns: req.dns.clone(),
        bypass_private: req.bypass_private,
        tun: true,
        local_inbounds: false,
        conn_idle_secs: req.conn_idle_secs.clamp(5, 600),
        outbound_interface: None,
    };
    let cfg = serde_json::to_vec(&xray::build_config(&req.server, &opts))?;

    info!(
        "starting xray (TUN) for {} via {}:{}",
        pw.name, req.server.address, req.server.port
    );
    let mut child = spawn_xray().await?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&cfg).await?;
        // Dropping closes stdin so xray starts.
    }

    let iface = Path::new("/sys/class/net").join(TUN_NAME);
    let deadline = Instant::now() + Duration::from_secs(8);
    while !iface.exists() {
        if let Some(st) = child.try_wait()? {
            bail!("xray exited early ({st})");
        }
        if Instant::now() > deadline {
            let _ = child.kill().await;
            bail!("{TUN_NAME} did not appear");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if let Err(e) = setup_routes(&req).await {
        let _ = child.kill().await;
        cleanup().await;
        return Err(e);
    }
    info!("TUN mode active");

    // KillMode=mixed: SIGTERM reaches only us; we stop xray ourselves.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let result = tokio::select! {
        st = child.wait() => Err(anyhow::anyhow!("xray exited ({})", st.map(|s| s.to_string()).unwrap_or_default())),
        _ = term.recv() => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };
    let _ = child.kill().await;
    let _ = child.wait().await;
    cleanup().await;
    let _ = std::fs::write(&clean_marker, b"");
    result
}

/// Xray parses untrusted network data: run it as `nobody` with only the network capabilities
/// it needs (TUN device, SO_MARK).
async fn spawn_xray() -> Result<tokio::process::Child> {
    let mut cmd = if Path::new("/usr/bin/setpriv").exists() {
        let nobody = passwd_by_name("nobody").context("user nobody not found")?;
        let mut c = Command::new("/usr/bin/setpriv");
        c.arg(format!("--reuid={}", nobody.uid))
            .arg(format!("--regid={}", nobody.gid))
            .arg("--clear-groups")
            .arg("--inh-caps=-all,+net_admin,+net_raw")
            .arg("--ambient-caps=-all,+net_admin,+net_raw")
            .arg("--bounding-set=-all,+net_admin,+net_raw")
            .arg("xray");
        c
    } else {
        warn!("setpriv not found; running xray as root");
        Command::new("xray")
    };
    cmd.args(["run", "-c", "stdin:"])
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting xray")
}

async fn setup_routes(req: &TunRequest) -> Result<()> {
    let mark = TUN_MARK.to_string();
    cmd("ip", &["link", "set", TUN_NAME, "up"]).await?;
    cmd_quiet("ip", &["addr", "add", TUN_ADDR4, "dev", TUN_NAME]).await;
    cmd_quiet("ip", &["-6", "addr", "add", TUN_ADDR6, "dev", TUN_NAME]).await;

    cmd(
        "ip",
        &[
            "route", "replace", "default", "dev", TUN_NAME, "table", TABLE,
        ],
    )
    .await?;
    cmd(
        "ip",
        &[
            "rule", "add", "pref", PREFS[0], "fwmark", &mark, "lookup", "main",
        ],
    )
    .await?;
    cmd(
        "ip",
        &[
            "rule",
            "add",
            "pref",
            PREFS[1],
            "lookup",
            "main",
            "suppress_prefixlength",
            "0",
        ],
    )
    .await?;

    // IPv6: into the tunnel if possible, otherwise blackhole it so it can't bypass the tunnel.
    // (Both fail only when IPv6 is disabled entirely, in which case nothing can leak.)
    let v6 = cmd(
        "ip",
        &[
            "-6", "route", "replace", "default", "dev", TUN_NAME, "table", TABLE,
        ],
    )
    .await
    .is_ok()
        || cmd(
            "ip",
            &[
                "-6",
                "route",
                "replace",
                "unreachable",
                "default",
                "table",
                TABLE,
            ],
        )
        .await
        .is_ok();
    if v6 {
        cmd_quiet(
            "ip",
            &[
                "-6", "rule", "add", "pref", PREFS[0], "fwmark", &mark, "lookup", "main",
            ],
        )
        .await;
        cmd_quiet(
            "ip",
            &[
                "-6",
                "rule",
                "add",
                "pref",
                PREFS[1],
                "lookup",
                "main",
                "suppress_prefixlength",
                "0",
            ],
        )
        .await;
        cmd_quiet(
            "ip",
            &["-6", "rule", "add", "pref", PREFS[2], "lookup", TABLE],
        )
        .await;
    }

    // Replies from the proxy server arrive on the physical NIC while the reverse lookup (unmarked)
    // points at the TUN; strict reverse-path filtering would drop them. The effective value is
    // max(all, iface), so loose (2) on `all` covers every interface.
    let rp = Path::new("/proc/sys/net/ipv4/conf/all/rp_filter");
    if let Ok(old) = std::fs::read_to_string(rp) {
        if old.trim() != "2" {
            let _ = std::fs::write(Path::new(RUN_DIR).join("rp_filter.saved"), old.trim());
            let _ = std::fs::write(rp, "2");
        }
    }

    let dns = if req.tun_dns.parse::<IpAddr>().is_ok() {
        req.tun_dns.as_str()
    } else {
        "1.1.1.1"
    };
    if let Err(e) = cmd("resolvectl", &["dns", TUN_NAME, dns]).await {
        warn!("setting DNS failed (is systemd-resolved running?): {e:#}");
    } else {
        cmd_quiet("resolvectl", &["domain", TUN_NAME, "~."]).await;
        cmd_quiet("resolvectl", &["default-route", TUN_NAME, "true"]).await;
    }

    // Added last: the daemon treats this rule as "TUN is ready".
    cmd("ip", &["rule", "add", "pref", PREFS[2], "lookup", TABLE]).await?;
    Ok(())
}

async fn ip_batch(family: &str, script: &str) -> Result<()> {
    let mut child = Command::new("ip")
        .args([family, "-force", "-batch", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("running ip -batch")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await?;
    }
    child.wait().await?;
    Ok(())
}

/// Remove everything `setup_routes` added. Idempotent; only called while holding the lock.
async fn cleanup() {
    // One `ip -batch` process per family instead of dozens of separate `ip` runs. Each rule is
    // deleted a few times because a crashed run may have added duplicates; `-force` keeps going
    // past "No such file or directory".
    let mut script = String::new();
    for pref in PREFS {
        for _ in 0..3 {
            script.push_str(&format!("rule del pref {pref}\n"));
        }
    }
    script.push_str(&format!("route flush table {TABLE}\n"));
    let (v4, v6) = tokio::join!(ip_batch("-4", &script), ip_batch("-6", &script));
    let _ = (v4, v6);
    let saved = Path::new(RUN_DIR).join("rp_filter.saved");
    if let Ok(v) = std::fs::read_to_string(&saved) {
        let _ = std::fs::write("/proc/sys/net/ipv4/conf/all/rp_filter", v.trim());
        let _ = std::fs::remove_file(&saved);
    }
    if Path::new("/sys/class/net").join(TUN_NAME).exists() {
        let _ = cmd("resolvectl", &["revert", TUN_NAME]).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use v2baloot_core::link;

    fn ip() -> IpAddr {
        "203.0.113.9".parse().unwrap()
    }

    #[test]
    fn pin_keeps_cdn_host_as_sni() {
        // CDN fronting: address is a front domain, Host is the real worker, no SNI.
        let s = link::parse(
            "vless://u@speed.cloudflare.com:443?type=ws&security=tls&host=w.workers.dev&path=%2F",
        )
        .unwrap();
        let p = pin_to_ip(&s, ip());
        assert_eq!(p.address, "203.0.113.9");
        assert_eq!(p.security.sni.as_deref(), Some("w.workers.dev"));
        assert_eq!(p.transport.host.as_deref(), Some("w.workers.dev"));
    }

    #[test]
    fn pin_uses_domain_when_nothing_else() {
        let s = link::parse("trojan://pw@t.example.com:443").unwrap();
        let p = pin_to_ip(&s, ip());
        assert_eq!(p.security.sni.as_deref(), Some("t.example.com"));
        // SNI exists, so Host is left for Xray to derive.
        assert_eq!(p.transport.host, None);

        let s = link::parse("vmess://u@plain.example.com:80?type=ws&path=%2F").unwrap();
        let p = pin_to_ip(&s, ip());
        assert_eq!(p.security.sni, None);
        assert_eq!(p.transport.host.as_deref(), Some("plain.example.com"));
    }

    #[test]
    fn pin_leaves_ips_alone() {
        let s = link::parse("trojan://pw@1.2.3.4:443?sni=x.com").unwrap();
        assert_eq!(pin_to_ip(&s, ip()), s);
    }

    #[test]
    fn validate_rejects_local_dns_and_domains() {
        let s = link::parse("trojan://pw@1.2.3.4:443").unwrap();
        let mut req = TunRequest {
            server: s,
            api_port: 10085,
            dns: vec!["1.1.1.1".into()],
            tun_dns: "1.1.1.1".into(),
            bypass_private: true,
            log_level: "warning".into(),
            conn_idle_secs: 30,
        };
        assert!(validate(&req).is_ok());
        req.dns = vec!["localhost".into()];
        assert!(validate(&req).is_err());
        req.dns = vec!["https+local://1.1.1.1/dns-query".into()];
        assert!(validate(&req).is_err());
        req.dns = vec!["1.1.1.1".into()];
        req.server.address = "evil.example.com".into();
        assert!(validate(&req).is_err());
    }
}
