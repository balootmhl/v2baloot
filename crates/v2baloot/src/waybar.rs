//! `v2baloot waybar`: continuous JSON output for a Waybar `custom` module.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use v2baloot_core::ipc::{Client, Event};
use v2baloot_core::util::human_speed;
use v2baloot_core::{ConnState, Mode, Status};

const ICON: &str = "󰖂";

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn render(status: Option<&Status>, show_name: bool) -> String {
    let Some(st) = status else {
        return json!({
            "text": ICON, "alt": "offline", "class": "offline",
            "tooltip": "v2baloot daemon is not running\nsystemctl --user start v2baloot",
        })
        .to_string();
    };
    let name = st.server_name.clone().unwrap_or_else(|| "no server".into());
    let (class, state) = match &st.state {
        ConnState::Connected if st.mode == Mode::Tun => ("tun", "Connected (TUN)".to_string()),
        ConnState::Connected => ("connected", "Connected (Proxy)".to_string()),
        ConnState::Connecting => ("connecting", "Connecting…".to_string()),
        ConnState::Disconnected => ("disconnected", "Disconnected".to_string()),
        ConnState::Error { message } => ("error", format!("Error: {message}")),
    };
    let text = if show_name && st.is_active() {
        format!("{ICON} {}", escape(&name))
    } else {
        ICON.to_string()
    };
    let mut tooltip = format!("<b>v2baloot</b> — {}\n{}", escape(&state), escape(&name));
    if st.is_connected() {
        tooltip.push_str(&format!(
            "\n↑ {}   ↓ {}",
            human_speed(st.up_speed),
            human_speed(st.down_speed)
        ));
        if st.mode == Mode::Proxy {
            tooltip.push_str(&format!(
                "\nSOCKS 127.0.0.1:{} · HTTP 127.0.0.1:{}",
                st.socks_port, st.http_port
            ));
        }
    }
    tooltip.push_str("\n\n<small>Click: open · Middle: TUI · Right: toggle</small>");
    json!({ "text": text, "alt": class, "class": class, "tooltip": tooltip }).to_string()
}

pub async fn run() -> Result<()> {
    let client = Client::default();
    let mut last = String::new();
    let mut emit = |line: String| {
        if line != last {
            println!("{line}");
            last = line;
        }
    };
    loop {
        if let Ok(mut events) = client.subscribe().await {
            let mut show_name = client
                .settings()
                .await
                .map(|s| s.waybar_show_name)
                .unwrap_or(false);
            let mut last_status = None;
            while let Ok(Some(ev)) = events.next().await {
                match ev {
                    Event::Status { status } => {
                        emit(render(Some(&status), show_name));
                        last_status = Some(status);
                    }
                    Event::SettingsChanged => {
                        show_name = client
                            .settings()
                            .await
                            .map(|s| s.waybar_show_name)
                            .unwrap_or(show_name);
                        if let Some(st) = &last_status {
                            emit(render(Some(st), show_name));
                        }
                    }
                    _ => {}
                }
            }
        }
        emit(render(None, false));
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
