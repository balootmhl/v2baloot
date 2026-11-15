//! "System proxy" on Hyprland.
//!
//! There is no global proxy setting on a wlroots/Hyprland desktop, so we set it everywhere apps
//! look: GNOME's gsettings (GTK apps, some Electron apps), the systemd user manager environment
//! (apps started via `uwsm app`, which Omarchy uses) and Hyprland's own environment. Apps that are
//! already running keep their old environment and need a restart — TUN mode avoids that.

use std::path::Path;

use tokio::process::Command;
use tracing::debug;

use v2baloot_core::store::Settings;

const VARS: &[&str] = &[
    "http_proxy",
    "https_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
];
const NO_PROXY: &str = "localhost,127.0.0.1,::1,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,.local";

/// Copies its arguments up front so callers can pass temporaries and run several at once.
fn run(cmd: &'static str, args: &[&str]) -> impl std::future::Future<Output = ()> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    async move {
        match Command::new(cmd).args(&args).output().await {
            Ok(o) if !o.status.success() => {
                debug!(
                    "{cmd} {args:?}: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )
            }
            Err(e) => debug!("{cmd}: {e}"),
            _ => {}
        }
    }
}

/// Address local clients should connect to for `listen`.
pub fn connect_host(listen: &str) -> String {
    match listen {
        "0.0.0.0" | "" => "127.0.0.1".into(),
        "::" => "::1".into(),
        other => other.to_string(),
    }
}

fn url_host(h: &str) -> String {
    if h.contains(':') {
        format!("[{h}]")
    } else {
        h.to_string()
    }
}

/// Marks that we changed the desktop proxy, so a restarted daemon can undo it after a crash.
fn marker(runtime_dir: &Path) -> std::path::PathBuf {
    runtime_dir.join("sysproxy.on")
}

/// Called at daemon start: undo a system proxy left behind by a crashed daemon.
pub async fn recover(runtime_dir: &Path) {
    if marker(runtime_dir).exists() {
        disable(runtime_dir).await;
    }
}

pub async fn enable(s: &Settings, runtime_dir: &Path) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let _ = std::fs::write(marker(runtime_dir), b"");
    let h = connect_host(&s.listen);
    let http = format!("http://{}:{}", url_host(&h), s.http_port);
    let socks = format!("socks5://{}:{}", url_host(&h), s.socks_port);
    let pairs: Vec<(&str, String)> = VARS
        .iter()
        .map(|k| {
            let v = match k.to_ascii_lowercase().as_str() {
                "all_proxy" => socks.clone(),
                "no_proxy" => NO_PROXY.to_string(),
                _ => http.clone(),
            };
            (*k, v)
        })
        .collect();

    let assignments: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut env_args = vec!["--user", "set-environment"];
    env_args.extend(assignments.iter().map(String::as_str));
    let batch = pairs
        .iter()
        .map(|(k, v)| format!("keyword env {k},{v}"))
        .collect::<Vec<_>>()
        .join(" ; ");

    let port_http = s.http_port.to_string();
    let port_socks = s.socks_port.to_string();
    let g = "org.gnome.system.proxy";
    let (http_s, https_s, socks_s) = (
        format!("{g}.http"),
        format!("{g}.https"),
        format!("{g}.socks"),
    );
    let gsettings = async {
        // Hosts/ports in parallel, then flip the mode once they are in place.
        futures::join!(
            run("gsettings", &["set", &http_s, "host", &h]),
            run("gsettings", &["set", &http_s, "port", &port_http]),
            run("gsettings", &["set", &https_s, "host", &h]),
            run("gsettings", &["set", &https_s, "port", &port_http]),
            run("gsettings", &["set", &socks_s, "host", &h]),
            run("gsettings", &["set", &socks_s, "port", &port_socks]),
            run(
                "gsettings",
                &["set", g, "ignore-hosts", "['localhost', '127.0.0.0/8', '::1', '10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']"],
            ),
        );
        run("gsettings", &["set", g, "mode", "manual"]).await;
    };
    futures::join!(
        run("systemctl", &env_args),
        run("hyprctl", &["--batch", &batch]),
        gsettings
    );
}

pub async fn disable(runtime_dir: &Path) {
    let _ = std::fs::remove_file(marker(runtime_dir));
    if !cfg!(target_os = "linux") {
        return;
    }
    let mut args = vec!["--user", "unset-environment"];
    args.extend(VARS);
    // Hyprland has no "unset"; an empty value means "no proxy" to virtually every client.
    let batch = VARS
        .iter()
        .map(|k| format!("keyword env {k},"))
        .collect::<Vec<_>>()
        .join(" ; ");
    futures::join!(
        run("systemctl", &args),
        run("hyprctl", &["--batch", &batch]),
        run(
            "gsettings",
            &["set", "org.gnome.system.proxy", "mode", "none"]
        ),
    );
}
