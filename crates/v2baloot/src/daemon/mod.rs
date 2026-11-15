//! The background daemon: owns xray, state and storage; serves IPC on a Unix socket.

pub mod stats;
pub mod xray_proc;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Mutex, Notify};
use tracing::{error, info, warn};

use v2baloot_core::ipc::{Event, NoticeLevel, Reply, Request};
use v2baloot_core::latency::Tester;
use v2baloot_core::paths::Paths;
use v2baloot_core::store::{self, Data, Settings};
use v2baloot_core::theme::Palette;
use v2baloot_core::util::{fnv1a64, now_unix};
use v2baloot_core::{
    link, subscription, xray, ConnState, Latency, Mode, Server, Status, Subscription,
};

use crate::{notify, sysproxy, tun};
use xray_proc::{LogBuf, XrayHandle};

pub struct Daemon {
    paths: Paths,
    inner: Mutex<Inner>,
    events: broadcast::Sender<Event>,
    logs: LogBuf,
    shutdown: Notify,
    /// Current Omarchy theme palette; the tray re-renders its icon when it changes.
    palette: tokio::sync::watch::Sender<Palette>,
}

struct Inner {
    settings: Settings,
    data: Data,
    status: Status,
    xray: Option<XrayHandle>,
    tun_on: bool,
    sysproxy_on: bool,
    /// Bumped on every connect/disconnect so stale watchers can tell they are stale.
    generation: u64,
    last_totals: Option<(u64, u64)>,
    /// Bumped on every user-initiated connection change, so a background reconnect loop can tell
    /// the user took over and must stop.
    user_action: u64,
}

pub async fn run() -> Result<()> {
    let paths = Paths::current();
    create_private_dir(&paths.runtime_dir)?;
    sysproxy::recover(&paths.runtime_dir).await;
    let sock = paths.socket();
    if UnixStream::connect(&sock).await.is_ok() {
        bail!("v2baloot daemon is already running ({})", sock.display());
    }
    let _ = std::fs::remove_file(&sock);
    let listener =
        UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    set_mode(&sock, 0o600);

    let settings = store::load_settings(&paths.settings_file())?;
    let data = store::load_data(&paths.data_file())?;
    if !paths.settings_file().exists() {
        let _ = store::save_settings(&paths.settings_file(), &settings);
    }
    let status = Status {
        mode: data.mode,
        server_id: data.selected.clone(),
        server_name: data
            .selected
            .as_deref()
            .and_then(|id| data.server(id))
            .map(|s| s.name.clone()),
        socks_port: settings.socks_port,
        http_port: settings.http_port,
        ..Default::default()
    };
    let (events, _) = broadcast::channel(256);
    let daemon = Arc::new(Daemon {
        paths,
        inner: Mutex::new(Inner {
            settings,
            data,
            status,
            xray: None,
            tun_on: false,
            sysproxy_on: false,
            generation: 0,
            last_totals: None,
            user_action: 0,
        }),
        events,
        logs: LogBuf::default(),
        shutdown: Notify::new(),
        palette: tokio::sync::watch::channel(load_palette()).0,
    });
    info!("listening on {}", sock.display());

    // A TUN session from a previous crash would leave routes in place; clear it.
    if tun::is_active().await {
        warn!("stopping leftover TUN session");
        tun::stop().await;
    }

    tokio::spawn(Arc::clone(&daemon).stats_loop());
    tokio::spawn(Arc::clone(&daemon).subscription_loop());
    if let Err(e) = crate::icons::write_notify_icon(&daemon.paths) {
        warn!("notification icon: {e:#}");
    }
    tokio::spawn(Arc::clone(&daemon).theme_loop());
    if cfg!(target_os = "linux") && daemon.inner.lock().await.settings.tray {
        crate::tray::spawn(Arc::clone(&daemon));
    }
    {
        let g = daemon.inner.lock().await;
        if g.settings.autoconnect && g.data.selected.is_some() {
            let d = Arc::clone(&daemon);
            tokio::spawn(async move {
                if let Err(e) = d.connect(None).await {
                    warn!("autoconnect failed: {e:#}");
                }
            });
        }
    }

    let conn_limit = Arc::new(tokio::sync::Semaphore::new(64));
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    // Cap concurrent clients; a misbehaving one can't exhaust the daemon.
                    match Arc::clone(&conn_limit).try_acquire_owned() {
                        Ok(permit) => {
                            let d = Arc::clone(&daemon);
                            tokio::spawn(async move {
                                d.serve(stream).await;
                                drop(permit);
                            });
                        }
                        Err(_) => warn!("too many IPC connections; dropping one"),
                    }
                }
                Err(e) => warn!("accept: {e}"),
            },
            _ = daemon.shutdown.notified() => break,
            _ = sigterm.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    info!("shutting down");
    {
        let mut g = daemon.inner.lock().await;
        daemon.stop_locked(&mut g).await;
    }
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

fn create_private_dir(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    set_mode(dir, 0o700);
    // The socket and xray.json (credentials) live here; refuse a directory someone else controls.
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        bail!(
            "{} must be a directory owned by you with mode 0700",
            dir.display()
        );
    }
    Ok(())
}

fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

async fn write_reply(
    w: &mut tokio::net::unix::OwnedWriteHalf,
    reply: &Reply,
) -> std::io::Result<()> {
    let mut line = serde_json::to_string(reply)
        .unwrap_or_else(|_| r#"{"type":"error","message":"encode"}"#.into());
    line.push('\n');
    w.write_all(line.as_bytes()).await
}

impl Daemon {
    async fn serve(self: Arc<Self>, stream: UnixStream) {
        use tokio::io::AsyncReadExt;
        let (r, mut w) = stream.into_split();
        // Requests are small; bound what a client may make us buffer, and how long it may idle.
        let mut lines = BufReader::new(r.take(4 * 1024 * 1024)).lines();
        let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await
        else {
            return;
        };
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_reply(
                    &mut w,
                    &Reply::Error {
                        message: format!("bad request: {e}"),
                    },
                )
                .await;
                return;
            }
        };

        if let Request::Subscribe = req {
            let mut rx = self.events.subscribe();
            let status = self.inner.lock().await.status.clone();
            if write_reply(
                &mut w,
                &Reply::Event {
                    event: Event::Status { status },
                },
            )
            .await
            .is_err()
            {
                return;
            }
            loop {
                tokio::select! {
                    ev = rx.recv() => match ev {
                        Ok(event) => {
                            if write_reply(&mut w, &Reply::Event { event }).await.is_err() { break; }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    l = lines.next_line() => if !matches!(l, Ok(Some(_))) { break },
                }
            }
            return;
        }

        let reply = self.handle(req).await;
        let _ = write_reply(&mut w, &reply).await;
    }

    /// Execute a request. Used by the socket server and the tray.
    pub async fn handle(self: &Arc<Self>, req: Request) -> Reply {
        match self.handle_inner(req).await {
            Ok(r) => r,
            Err(e) => Reply::Error {
                message: v2baloot_core::util::clean_text(&format!("{e:#}"), 1000),
            },
        }
    }

    async fn handle_inner(self: &Arc<Self>, req: Request) -> Result<Reply> {
        if matches!(
            req,
            Request::Connect { .. }
                | Request::Disconnect
                | Request::Toggle
                | Request::SetMode { .. }
                | Request::Shutdown
        ) {
            self.inner.lock().await.user_action += 1;
        }
        Ok(match req {
            Request::Status => Reply::Status {
                status: self.inner.lock().await.status.clone(),
            },
            Request::ListServers => {
                let g = self.inner.lock().await;
                Reply::Servers {
                    servers: g.data.servers.clone(),
                    selected: g.data.selected.clone(),
                }
            }
            Request::ListSubscriptions => Reply::Subscriptions {
                subscriptions: self.inner.lock().await.data.subscriptions.clone(),
            },
            Request::Connect { id } => {
                self.connect(id).await?;
                Reply::Ok
            }
            Request::Disconnect => {
                self.disconnect().await;
                Reply::Ok
            }
            Request::Toggle => {
                let active = self.inner.lock().await.status.is_active();
                if active {
                    self.disconnect().await;
                } else {
                    self.connect(None).await?;
                }
                Reply::Ok
            }
            Request::SetMode { mode } => {
                self.set_mode(mode).await?;
                Reply::Ok
            }
            Request::Import { text } => self.import(&text).await?,
            Request::AddSubscription { name, url } => self.add_subscription(name, url).await?,
            Request::RemoveSubscription { id } => {
                self.remove_where(|s| s.group.as_deref() == Some(id.as_str()))
                    .await;
                let mut g = self.inner.lock().await;
                g.data.remove_group(&id);
                self.save(&g);
                drop(g);
                self.emit(Event::ServersChanged);
                Reply::Ok
            }
            Request::UpdateSubscriptions { id } => self.update_subscriptions(id).await?,
            Request::RemoveServer { id } => {
                self.remove_where(|s| s.id == id).await;
                Reply::Ok
            }
            Request::TestLatency { ids } => {
                self.test_latency(ids).await?;
                Reply::Ok
            }
            Request::Logs => Reply::Logs {
                lines: self.logs.lines(),
            },
            Request::GetSettings => Reply::Settings {
                settings: Box::new(self.inner.lock().await.settings.clone()),
            },
            Request::SetSettings { settings } => {
                self.set_settings(*settings).await?;
                Reply::Ok
            }
            Request::Subscribe => bail!("subscribe must be the only request on a connection"),
            Request::Shutdown => {
                self.shutdown.notify_one();
                Reply::Ok
            }
        })
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    pub fn palette(&self) -> tokio::sync::watch::Receiver<Palette> {
        self.palette.subscribe()
    }

    /// Re-tint the Waybar and tray icons whenever the Omarchy theme changes.
    async fn theme_loop(self: Arc<Self>) {
        let file = home_dir().map(|h| v2baloot_core::theme::theme_file(&h));
        let mtime = |f: &Option<std::path::PathBuf>| {
            f.as_ref()
                .and_then(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
        };
        let mut last = None;
        let mut first = true;
        loop {
            let now = mtime(&file);
            if first || now != last {
                first = false;
                last = now;
                let p = load_palette();
                if let Err(e) = crate::icons::write_waybar_icons(&self.paths, &p) {
                    warn!("waybar icons: {e:#}");
                }
                self.palette.send_replace(p);
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    pub async fn snapshot(&self) -> (Status, Vec<Server>) {
        let g = self.inner.lock().await;
        (g.status.clone(), g.data.servers.clone())
    }

    fn emit_status(&self, g: &Inner) {
        self.emit(Event::Status {
            status: g.status.clone(),
        });
    }

    fn notice(&self, level: NoticeLevel, message: impl Into<String>) {
        self.emit(Event::Notice {
            level,
            message: message.into(),
        });
    }

    fn save(&self, g: &Inner) {
        if let Err(e) = store::save_data(&self.paths.data_file(), &g.data) {
            error!("saving data: {e:#}");
        }
    }

    // ------------------------------------------------------------------ connection

    pub async fn connect(self: &Arc<Self>, id: Option<String>) -> Result<()> {
        self.connect_with(id, true).await
    }

    /// `report_errors = false` is used by automatic reconnect attempts that will retry anyway.
    async fn connect_with(self: &Arc<Self>, id: Option<String>, report_errors: bool) -> Result<()> {
        let mut g = self.inner.lock().await;
        let id = match id {
            Some(id) => {
                if g.data.server(&id).is_none() {
                    bail!("no server with id {id}");
                }
                id
            }
            None => match g
                .data
                .selected
                .clone()
                .filter(|id| g.data.server(id).is_some())
            {
                Some(id) => id,
                None if g.data.servers.is_empty() => {
                    bail!("no servers yet — import a share link or add a subscription")
                }
                None => bail!("no server selected — pick one from the list first"),
            },
        };
        let server = g.data.server(&id).cloned().unwrap();

        self.stop_locked(&mut g).await;
        g.data.selected = Some(id.clone());
        self.save(&g);
        g.generation += 1;
        let gen = g.generation;
        let mode = g.data.mode;
        g.status.state = ConnState::Connecting;
        g.status.mode = mode;
        g.status.server_id = Some(id);
        g.status.server_name = Some(server.name.clone());
        g.status.socks_port = g.settings.socks_port;
        g.status.http_port = g.settings.http_port;
        self.emit_status(&g);

        let res = match mode {
            Mode::Proxy => self.start_proxy(&mut g, &server, gen).await,
            Mode::Tun => self.start_tun(&mut g, &server).await,
        };
        match res {
            Ok(()) => {
                g.status.state = ConnState::Connected;
                g.status.connected_since = Some(v2baloot_core::util::now_unix());
                g.status.exit = None;
                self.emit_status(&g);
                if g.settings.notifications {
                    notify::send("Connected", &format!("{} · {}", server.name, mode.label()));
                }
                if g.settings.show_exit_ip {
                    self.spawn_exit_lookup(&g);
                }
                Ok(())
            }
            Err(e) => {
                self.stop_locked(&mut g).await;
                let message = v2baloot_core::util::clean_text(&format!("{e:#}"), 500);
                g.status.state = ConnState::Error {
                    message: message.clone(),
                };
                self.emit_status(&g);
                if report_errors && g.settings.notifications {
                    notify::send_error("Connection failed", &message);
                }
                Err(e)
            }
        }
    }

    async fn start_proxy(self: &Arc<Self>, g: &mut Inner, server: &Server, gen: u64) -> Result<()> {
        let cfg = xray::build_config(server, &g.settings.xray_options(false));
        let cfg_path = self.paths.xray_config();
        store::write_atomic(
            &cfg_path,
            serde_json::to_string_pretty(&cfg)?.as_bytes(),
            true,
        )?;
        let handle = xray_proc::spawn(&g.settings.xray_path, &cfg_path, self.logs.clone()).await?;
        let mut exited = handle.exited.clone();
        g.xray = Some(handle);
        let host = sysproxy::connect_host(&g.settings.listen);
        xray_proc::wait_ready(
            &host,
            g.settings.socks_port,
            &mut exited,
            &self.logs,
            Duration::from_secs(6),
        )
        .await?;

        let me = Arc::clone(self);
        tokio::spawn(async move {
            while exited.changed().await.is_ok() {
                let desc = exited.borrow().clone();
                if let Some(desc) = desc {
                    me.on_unexpected_exit(gen, desc).await;
                    break;
                }
            }
        });

        if g.settings.system_proxy {
            sysproxy::enable(&g.settings, &self.paths.runtime_dir).await;
            g.sysproxy_on = true;
        }
        Ok(())
    }

    async fn start_tun(self: &Arc<Self>, g: &mut Inner, server: &Server) -> Result<()> {
        g.tun_on = true;
        self.logs
            .push("--- starting TUN session (v2baloot-tun@.service)");
        tun::start(&g.settings, server).await
    }

    async fn on_unexpected_exit(self: &Arc<Self>, gen: u64, desc: String) {
        let mut g = self.inner.lock().await;
        if g.generation != gen || g.xray.is_none() {
            return;
        }
        let reason = match self.logs.last_error() {
            Some(e) => format!("{desc}: {}", xray_proc::clean_xray_error(&e)),
            None => desc,
        };
        let reason = v2baloot_core::util::clean_text(&reason, 500);
        warn!("{reason}");
        g.xray = None;
        self.stop_locked(&mut g).await;
        drop(g);
        self.spawn_recovery(reason, Duration::from_secs(1));
    }

    /// Bring a dropped connection back, the way an OS-managed "persistent" VPN tunnel does:
    /// retry with backoff, give up only after the last attempt, and stop as soon as the user
    /// connects/disconnects/switches mode themselves.
    fn spawn_recovery(self: &Arc<Self>, reason: String, first_delay: Duration) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let token = {
                let mut g = me.inner.lock().await;
                g.status.state = ConnState::Connecting;
                me.emit_status(&g);
                g.user_action
            };
            info!("reconnecting: {reason}");
            let delays = [first_delay, Duration::from_secs(2), Duration::from_secs(4)];
            for (i, delay) in delays.iter().enumerate() {
                tokio::time::sleep(*delay).await;
                if me.inner.lock().await.user_action != token {
                    return; // the user took over
                }
                let last = i == delays.len() - 1;
                match me.connect_with(None, last).await {
                    Ok(()) => {
                        info!("reconnected");
                        return;
                    }
                    Err(e) => warn!("reconnect attempt {} failed: {e:#}", i + 1),
                }
                if !last {
                    // Keep showing "connecting" between attempts instead of a transient error.
                    let mut g = me.inner.lock().await;
                    if g.user_action == token {
                        g.status.state = ConnState::Connecting;
                        me.emit_status(&g);
                    }
                }
            }
        });
    }

    /// Tear down whatever is running. Keeps `status.state` for the caller to set.
    async fn stop_locked(&self, g: &mut Inner) {
        g.generation += 1;
        if let Some(h) = g.xray.take() {
            h.stop().await;
        }
        if g.sysproxy_on {
            sysproxy::disable(&self.paths.runtime_dir).await;
            g.sysproxy_on = false;
        }
        if g.tun_on {
            tun::stop().await;
            g.tun_on = false;
        }
        g.status.up_speed = 0;
        g.status.down_speed = 0;
        g.status.connected_since = None;
        g.status.exit = None;
        g.last_totals = None;
    }

    /// Find out where traffic exits (IP, country) through the tunnel, then publish it.
    fn spawn_exit_lookup(self: &Arc<Self>, g: &Inner) {
        let gen = g.generation;
        let proxy = (g.data.mode == Mode::Proxy).then(|| {
            format!(
                "socks5h://{}:{}",
                sysproxy::connect_host(&g.settings.listen),
                g.settings.socks_port
            )
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            match v2baloot_core::exitip::lookup(proxy.as_deref()).await {
                Ok(info) => {
                    let mut g = me.inner.lock().await;
                    if g.generation == gen && g.status.is_connected() {
                        g.status.exit = Some(info);
                        me.emit_status(&g);
                    }
                }
                Err(e) => warn!("exit IP lookup failed: {e:#}"),
            }
        });
    }

    pub async fn disconnect(&self) {
        let mut g = self.inner.lock().await;
        let was_active = g.status.is_active();
        // Tell every front-end immediately; stopping xray / the TUN unit / the system proxy
        // finishes in the background while we still hold the lock (so a reconnect waits).
        g.status.state = ConnState::Disconnected;
        g.status.connected_since = None;
        g.status.exit = None;
        g.status.up_speed = 0;
        g.status.down_speed = 0;
        self.emit_status(&g);
        if was_active && g.settings.notifications {
            notify::send("Disconnected", "v2baloot");
        }
        let t0 = std::time::Instant::now();
        self.stop_locked(&mut g).await;
        tracing::debug!("teardown took {:?}", t0.elapsed());
    }

    async fn set_mode(self: &Arc<Self>, mode: Mode) -> Result<()> {
        let mut g = self.inner.lock().await;
        if g.data.mode == mode {
            return Ok(());
        }
        g.data.mode = mode;
        g.status.mode = mode;
        self.save(&g);
        let reconnect = g.status.is_active();
        self.emit_status(&g);
        drop(g);
        if reconnect {
            self.connect(None).await?;
        }
        Ok(())
    }

    async fn set_settings(self: &Arc<Self>, new: Settings) -> Result<()> {
        if new.listen.parse::<std::net::IpAddr>().is_err() {
            bail!("listen must be an IP address");
        }
        if !["debug", "info", "warning", "error", "none"].contains(&new.log_level.as_str()) {
            bail!("unknown log level");
        }
        if new.outbound_interface.as_deref().is_some_and(|i| {
            i.len() > 15
                || !i
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.@".contains(c))
        }) {
            bail!("invalid interface name");
        }
        if new.socks_port == 0 || new.http_port == 0 || new.api_port == 0 {
            bail!("ports must be non-zero");
        }
        if new.socks_port == new.http_port
            || new.socks_port == new.api_port
            || new.http_port == new.api_port
        {
            bail!("socks, http and api ports must differ");
        }
        let mut g = self.inner.lock().await;
        let old = std::mem::replace(&mut g.settings, new);
        store::save_settings(&self.paths.settings_file(), &g.settings)?;
        let s = &g.settings;
        let affects_connection = old.xray_path != s.xray_path
            || old.listen != s.listen
            || old.socks_port != s.socks_port
            || old.http_port != s.http_port
            || old.api_port != s.api_port
            || old.dns != s.dns
            || old.tun_dns != s.tun_dns
            || old.bypass_private != s.bypass_private
            || old.log_level != s.log_level
            || old.system_proxy != s.system_proxy;
        let reconnect = affects_connection && g.status.is_active();
        drop(g);
        self.emit(Event::SettingsChanged);
        if reconnect {
            self.connect(None).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ servers

    async fn import(&self, text: &str) -> Result<Reply> {
        let out = link::parse_many(text);
        if out.servers.is_empty() {
            match out.errors.first() {
                Some(e) => bail!("nothing imported: {e}"),
                None => bail!("no share links found"),
            }
        }
        let mut g = self.inner.lock().await;
        let first = out.servers.first().map(|s| s.id.clone());
        let added = g.data.add_servers(out.servers);
        if g.data.selected.is_none() {
            g.data.selected = first;
        }
        self.save(&g);
        drop(g);
        self.emit(Event::ServersChanged);
        Ok(Reply::Imported {
            added,
            errors: out.errors,
        })
    }

    /// Remove servers matching `pred`, disconnecting first if the active one is among them.
    async fn remove_where(&self, pred: impl Fn(&Server) -> bool) {
        let active_hit = {
            let g = self.inner.lock().await;
            g.status.is_active()
                && g.status
                    .server_id
                    .as_deref()
                    .and_then(|id| g.data.server(id))
                    .map(&pred)
                    .unwrap_or(false)
        };
        if active_hit {
            self.disconnect().await;
        }
        let mut g = self.inner.lock().await;
        g.data.servers.retain(|s| !pred(s));
        if let Some(sel) = g.data.selected.clone() {
            if g.data.server(&sel).is_none() {
                g.data.selected = None;
            }
        }
        self.save(&g);
        drop(g);
        self.emit(Event::ServersChanged);
    }

    async fn add_subscription(&self, name: String, url: String) -> Result<Reply> {
        let url = url.trim().to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            bail!("subscription URL must start with http:// or https://");
        }
        if url.len() > 4096 || url::Url::parse(&url).is_err() {
            bail!("invalid subscription URL");
        }
        if url.starts_with("http://") {
            self.notice(
                NoticeLevel::Error,
                "This subscription uses plain http:// — anyone on the network can read your token and swap its servers. Ask your provider for an https:// link.",
            );
        }
        let id = format!("{:012x}", fnv1a64(url.as_bytes()) & 0xffff_ffff_ffff);
        {
            let mut g = self.inner.lock().await;
            if g.data.subscriptions.iter().any(|s| s.id == id) {
                bail!("this subscription is already added");
            }
            let n = g.data.subscriptions.len() + 1;
            let name = if name.trim().is_empty() {
                format!("Subscription {n}")
            } else {
                v2baloot_core::util::clean_text(&name, 60)
            };
            g.data.subscriptions.push(Subscription {
                id: id.clone(),
                name,
                url,
                updated_at: None,
                usage: None,
                last_error: None,
            });
            self.save(&g);
        }
        self.emit(Event::ServersChanged);
        self.update_subscriptions(Some(id)).await
    }

    pub async fn update_subscriptions(&self, id: Option<String>) -> Result<Reply> {
        let (subs, ua, proxy) = {
            let g = self.inner.lock().await;
            let subs: Vec<Subscription> = g
                .data
                .subscriptions
                .iter()
                .filter(|s| id.as_ref().map(|i| &s.id == i).unwrap_or(true))
                .cloned()
                .collect();
            let proxy = (g.status.is_connected() && g.data.mode == Mode::Proxy)
                .then(|| format!("socks5h://127.0.0.1:{}", g.settings.socks_port));
            (subs, g.settings.sub_user_agent.clone(), proxy)
        };
        if subs.is_empty() {
            bail!(if id.is_some() {
                "no such subscription"
            } else {
                "no subscriptions added yet"
            });
        }

        let mut added = 0;
        let mut errors = vec![];
        let mut failures = 0;
        for sub in &subs {
            let res = subscription::fetch(&sub.url, &ua, proxy.as_deref()).await;
            let mut g = self.inner.lock().await;
            let Some(idx) = g.data.subscriptions.iter().position(|s| s.id == sub.id) else {
                continue;
            };
            match res {
                Ok(r) => {
                    errors.extend(
                        r.outcome
                            .errors
                            .into_iter()
                            .map(|e| format!("{}: {e}", sub.name)),
                    );
                    added += g.data.replace_group(&sub.id, r.outcome.servers);
                    match g.data.selected.clone() {
                        // First subscription: start with its first server.
                        None => {
                            g.data.selected = g
                                .data
                                .servers
                                .iter()
                                .find(|s| s.group.as_deref() == Some(sub.id.as_str()))
                                .map(|s| s.id.clone());
                        }
                        // The chosen server vanished: never silently switch the user to a
                        // different (subscription-controlled) server — make them pick.
                        Some(id) if g.data.server(&id).is_none() => {
                            g.data.selected = None;
                            self.notice(
                                NoticeLevel::Info,
                                "Your selected server was removed by the subscription update; choose another one",
                            );
                        }
                        Some(_) => {}
                    }
                    let s = &mut g.data.subscriptions[idx];
                    s.updated_at = Some(now_unix());
                    s.last_error = None;
                    if r.usage.is_some() {
                        s.usage = r.usage;
                    }
                    if let Some(t) = r.title {
                        if s.name.starts_with("Subscription ") {
                            s.name = t;
                        }
                    }
                }
                Err(e) => {
                    failures += 1;
                    let msg = v2baloot_core::util::clean_text(&format!("{e:#}"), 300);
                    g.data.subscriptions[idx].last_error = Some(msg.clone());
                    errors.push(v2baloot_core::util::clean_text(
                        &format!("{}: {msg}", sub.name),
                        300,
                    ));
                }
            }
            self.save(&g);
        }
        self.emit(Event::ServersChanged);
        if failures == subs.len() {
            bail!(errors.join("\n"));
        }
        Ok(Reply::Imported { added, errors })
    }

    // ------------------------------------------------------------------ latency

    async fn test_latency(self: &Arc<Self>, ids: Option<Vec<String>>) -> Result<()> {
        let (servers, tester) = {
            let mut g = self.inner.lock().await;
            if g.status.testing {
                bail!("a latency test is already running");
            }
            let servers: Vec<Server> = match &ids {
                Some(ids) => g
                    .data
                    .servers
                    .iter()
                    .filter(|s| ids.contains(&s.id))
                    .cloned()
                    .collect(),
                None => g.data.servers.clone(),
            };
            if servers.is_empty() {
                bail!("no servers to test");
            }
            g.status.testing = true;
            self.emit_status(&g);
            let s = &g.settings;
            let tester = Tester {
                xray_path: s.xray_path.clone(),
                work_dir: self.paths.runtime_dir.clone(),
                url: s.test_url.clone(),
                timeout: Duration::from_millis(s.test_timeout_ms.max(500)),
                concurrency: s.test_concurrency.clamp(1, 64),
            };
            if g.tun_on {
                self.notice(
                    NoticeLevel::Info,
                    "TUN mode is on: latency is measured through the tunnel and reads higher",
                );
            }
            (servers, tester)
        };

        let me = Arc::clone(self);
        tokio::spawn(async move {
            let (tx, mut rx) = mpsc::unbounded_channel::<(String, Latency)>();
            let run = tester.run(servers, move |id, lat| {
                let _ = tx.send((id, lat));
            });
            let collect = async {
                while let Some((id, lat)) = rx.recv().await {
                    let mut g = me.inner.lock().await;
                    if let Some(s) = g.data.servers.iter_mut().find(|s| s.id == id) {
                        s.latency = Some(lat.clone());
                    }
                    drop(g);
                    me.emit(Event::Latency { id, latency: lat });
                }
            };
            let (res, ()) = tokio::join!(run, collect);
            let mut g = me.inner.lock().await;
            g.status.testing = false;
            me.save(&g);
            me.emit_status(&g);
            drop(g);
            if let Err(e) = res {
                me.notice(NoticeLevel::Error, format!("latency test failed: {e:#}"));
            }
            me.emit(Event::TestFinished);
        });
        Ok(())
    }

    // ------------------------------------------------------------------ background loops

    async fn stats_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut clocks = (std::time::SystemTime::now(), std::time::Instant::now());
        loop {
            tick.tick().await;
            let now = (std::time::SystemTime::now(), std::time::Instant::now());
            let woke = resumed_from_sleep(clocks, now);
            clocks = now;
            if woke && self.inner.lock().await.status.is_connected() {
                // Every TCP flow from before the sleep is dead; restart now instead of letting
                // apps wait for timeouts. Give the network a moment to come back first.
                self.spawn_recovery("resumed from sleep".into(), Duration::from_secs(2));
                continue;
            }
            let (xray_path, api_port, gen, tun_on) = {
                let g = self.inner.lock().await;
                if !g.status.is_connected() {
                    continue;
                }
                (
                    g.settings.xray_path.clone(),
                    g.settings.api_port,
                    g.generation,
                    g.tun_on,
                )
            };
            if tun_on && !tun::is_active().await {
                let mut g = self.inner.lock().await;
                if g.generation == gen && g.tun_on {
                    self.stop_locked(&mut g).await;
                    drop(g);
                    self.spawn_recovery(
                        "TUN session stopped unexpectedly (see: journalctl -u 'v2baloot-tun@*')"
                            .into(),
                        Duration::from_secs(1),
                    );
                }
                continue;
            }
            let Ok((up, down)) = stats::query(&xray_path, api_port).await else {
                continue;
            };
            let mut g = self.inner.lock().await;
            if g.generation != gen {
                continue;
            }
            if let Some((pu, pd)) = g.last_totals {
                g.status.up_speed = up.saturating_sub(pu);
                g.status.down_speed = down.saturating_sub(pd);
            }
            g.last_totals = Some((up, down));
            g.status.total_up = up;
            g.status.total_down = down;
            self.emit_status(&g);
        }
    }

    async fn subscription_loop(self: Arc<Self>) {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let due: Vec<String> = {
                let g = self.inner.lock().await;
                let hours = g.settings.sub_update_hours as i64;
                if hours == 0 {
                    vec![]
                } else {
                    let now = now_unix();
                    g.data
                        .subscriptions
                        .iter()
                        .filter(|s| {
                            s.updated_at
                                .map(|t| now - t >= hours * 3600)
                                .unwrap_or(true)
                        })
                        .map(|s| s.id.clone())
                        .collect()
                }
            };
            for id in due {
                if let Err(e) = self.update_subscriptions(Some(id)).await {
                    warn!("auto-update: {e:#}");
                }
            }
            tokio::time::sleep(Duration::from_secs(15 * 60)).await;
        }
    }
}

/// The monotonic clock stops while the machine is suspended but the wall clock doesn't, so a
/// wall-clock gap much larger than the monotonic one between two ticks means we just woke up.
fn resumed_from_sleep(
    before: (std::time::SystemTime, std::time::Instant),
    now: (std::time::SystemTime, std::time::Instant),
) -> bool {
    let wall = now.0.duration_since(before.0).unwrap_or_default();
    let mono = now.1.saturating_duration_since(before.1);
    wall > mono + Duration::from_secs(10)
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn load_palette() -> Palette {
    home_dir()
        .and_then(|h| v2baloot_core::theme::load(&h))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, SystemTime};

    #[test]
    fn detects_resume_by_clock_gap() {
        let t0 = (SystemTime::now(), Instant::now());
        let normal = (t0.0 + Duration::from_secs(1), t0.1 + Duration::from_secs(1));
        assert!(!resumed_from_sleep(t0, normal));
        let after_sleep = (
            t0.0 + Duration::from_secs(600),
            t0.1 + Duration::from_secs(1),
        );
        assert!(resumed_from_sleep(t0, after_sleep));
        // Clock set backwards: not a resume.
        let backwards = (
            t0.0 - Duration::from_secs(600),
            t0.1 + Duration::from_secs(1),
        );
        assert!(!resumed_from_sleep(t0, backwards));
    }
}
