//! StatusNotifierItem tray icon (shown in Waybar's `tray` module), hosted inside the daemon.

use std::sync::Arc;

use ksni::menu::{RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip, Tray, TrayMethods};
use tokio::sync::mpsc;
use tracing::warn;

use v2baloot_core::ipc::{Event, Reply, Request};
use v2baloot_core::{ConnState, Mode, Server, Status};

use crate::daemon::Daemon;
use crate::{launch, notify};

const MAX_SERVERS: usize = 60;

enum Action {
    Daemon(Request),
    ImportClipboard,
    OpenGui,
    OpenTui,
}

struct V2Tray {
    status: Status,
    servers: Vec<Server>,
    tx: mpsc::UnboundedSender<Action>,
}

impl V2Tray {
    fn send(&self, a: Action) {
        let _ = self.tx.send(a);
    }
}

fn item(
    label: impl Into<String>,
    icon: &str,
    f: impl Fn(&mut V2Tray) + Send + 'static,
) -> MenuItem<V2Tray> {
    StandardItem {
        label: label.into(),
        icon_name: icon.into(),
        activate: Box::new(f),
        ..Default::default()
    }
    .into()
}

impl Tray for V2Tray {
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        "v2baloot".into()
    }

    fn title(&self) -> String {
        "v2baloot".into()
    }

    fn icon_name(&self) -> String {
        match (&self.status.state, self.status.mode) {
            (ConnState::Connected, Mode::Tun) => "v2baloot-tun-symbolic",
            (ConnState::Connected, Mode::Proxy) => "v2baloot-connected-symbolic",
            (ConnState::Error { .. }, _) => "v2baloot-error-symbolic",
            _ => "v2baloot-disconnected-symbolic",
        }
        .into()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "v2baloot".into(),
            description: status_line(&self.status),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::OpenGui);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let st = &self.status;
        let mut m: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: status_line(st),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
        ];
        if st.is_active() {
            m.push(item("Disconnect", "network-offline-symbolic", |t| {
                t.send(Action::Daemon(Request::Disconnect))
            }));
        } else {
            m.push(item("Connect", "network-vpn-symbolic", |t| {
                t.send(Action::Daemon(Request::Connect { id: None }))
            }));
        }

        let current = st.server_id.clone();
        let shown: Vec<&Server> = self.servers.iter().take(MAX_SERVERS).collect();
        let selected = current
            .as_ref()
            .and_then(|id| shown.iter().position(|s| &s.id == id))
            .unwrap_or(usize::MAX);
        let ids: Vec<String> = shown.iter().map(|s| s.id.clone()).collect();
        let servers_menu: Vec<MenuItem<Self>> = if shown.is_empty() {
            vec![StandardItem {
                label: "No servers yet".into(),
                enabled: false,
                ..Default::default()
            }
            .into()]
        } else {
            vec![RadioGroup {
                selected,
                select: Box::new(move |t: &mut Self, i| {
                    if let Some(id) = ids.get(i) {
                        t.send(Action::Daemon(Request::Connect {
                            id: Some(id.clone()),
                        }));
                    }
                }),
                options: shown
                    .iter()
                    .map(|s| RadioItem {
                        label: match s.latency_ms() {
                            Some(ms) => format!("{}  ({ms} ms)", s.name.replace('_', "__")),
                            None => s.name.replace('_', "__"),
                        },
                        ..Default::default()
                    })
                    .collect(),
            }
            .into()]
        };
        m.push(
            SubMenu {
                label: "Servers".into(),
                icon_name: "network-server-symbolic".into(),
                submenu: servers_menu,
                ..Default::default()
            }
            .into(),
        );

        m.push(
            SubMenu {
                label: format!("Mode: {}", st.mode.label()),
                submenu: vec![RadioGroup {
                    selected: if st.mode == Mode::Proxy { 0 } else { 1 },
                    select: Box::new(|t: &mut Self, i| {
                        let mode = if i == 0 { Mode::Proxy } else { Mode::Tun };
                        t.send(Action::Daemon(Request::SetMode { mode }));
                    }),
                    options: vec![
                        RadioItem {
                            label: "Proxy (SOCKS/HTTP + system proxy)".into(),
                            ..Default::default()
                        },
                        RadioItem {
                            label: "TUN (all traffic)".into(),
                            ..Default::default()
                        },
                    ],
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        m.push(MenuItem::Separator);
        m.push(
            StandardItem {
                label: if st.testing {
                    "Testing latency…".into()
                } else {
                    "Test latency".into()
                },
                enabled: !st.testing,
                activate: Box::new(|t: &mut Self| {
                    t.send(Action::Daemon(Request::TestLatency { ids: None }))
                }),
                ..Default::default()
            }
            .into(),
        );
        m.push(item("Update subscriptions", "view-refresh-symbolic", |t| {
            t.send(Action::Daemon(Request::UpdateSubscriptions { id: None }))
        }));
        m.push(item(
            "Import link from clipboard",
            "edit-paste-symbolic",
            |t| t.send(Action::ImportClipboard),
        ));
        m.push(MenuItem::Separator);
        m.push(item("Open v2baloot", "window-new-symbolic", |t| {
            t.send(Action::OpenGui)
        }));
        m.push(item(
            "Open in terminal",
            "utilities-terminal-symbolic",
            |t| t.send(Action::OpenTui),
        ));
        m.push(item("Quit", "application-exit-symbolic", |t| {
            t.send(Action::Daemon(Request::Shutdown))
        }));
        m
    }
}

fn status_line(st: &Status) -> String {
    let name = st.server_name.as_deref().unwrap_or("no server");
    match &st.state {
        ConnState::Connected => format!("● {name} · {}", st.mode.label()),
        ConnState::Connecting => format!("… connecting to {name}"),
        ConnState::Disconnected => "○ Disconnected".into(),
        ConnState::Error { message } => {
            let short: String = message.chars().take(60).collect();
            format!("⚠ {short}")
        }
    }
}

/// Status without the per-second counters, to avoid redrawing the tray every second.
fn tray_key(st: &Status) -> Status {
    Status {
        up_speed: 0,
        down_speed: 0,
        total_up: 0,
        total_down: 0,
        ..st.clone()
    }
}

pub fn spawn(daemon: Arc<Daemon>) {
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (status, servers) = daemon.snapshot().await;
        let mut events = daemon.events();
        let tray = V2Tray {
            status,
            servers,
            tx,
        };
        let handle = match tray.spawn().await {
            Ok(h) => h,
            Err(e) => {
                warn!("tray unavailable: {e}");
                return;
            }
        };

        // Menu actions.
        let d = Arc::clone(&daemon);
        tokio::spawn(async move {
            while let Some(a) = rx.recv().await {
                let d = Arc::clone(&d);
                tokio::spawn(async move {
                    let reply = match a {
                        Action::Daemon(req) => d.handle(req).await,
                        Action::ImportClipboard => match launch::clipboard() {
                            Some(text) => d.handle(Request::Import { text }).await,
                            None => Reply::Error {
                                message: "clipboard is empty".into(),
                            },
                        },
                        Action::OpenGui => {
                            if let Err(e) = launch::gui() {
                                notify::send_error("v2baloot", &format!("cannot open window: {e}"));
                            }
                            Reply::Ok
                        }
                        Action::OpenTui => {
                            if let Err(e) = launch::tui() {
                                notify::send_error(
                                    "v2baloot",
                                    &format!("cannot open terminal: {e}"),
                                );
                            }
                            Reply::Ok
                        }
                    };
                    match reply {
                        Reply::Error { message } => notify::send_error("v2baloot", &message),
                        Reply::Imported { added, .. } => {
                            notify::send("v2baloot", &format!("{added} server(s) added/updated"))
                        }
                        _ => {}
                    }
                });
            }
        });

        // Keep the tray in sync with daemon state.
        let mut last = None;
        loop {
            match events.recv().await {
                Ok(Event::Status { status }) => {
                    let key = tray_key(&status);
                    if last.as_ref() == Some(&key) {
                        continue;
                    }
                    last = Some(key);
                    handle.update(|t| t.status = status).await;
                }
                Ok(Event::ServersChanged | Event::TestFinished) => {
                    let (_, servers) = daemon.snapshot().await;
                    handle.update(|t| t.servers = servers).await;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
}
