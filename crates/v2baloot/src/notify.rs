//! Desktop notifications via `notify-send` (shown by mako on Omarchy).

use tokio::process::Command;

pub const APP_ICON: &str = "org.omarchy.v2baloot";

/// The PNG the daemon renders at start (an absolute path always works in mako), falling back
/// to the icon-theme name.
fn icon() -> String {
    let png = v2baloot_core::paths::Paths::current().notify_icon();
    if png.exists() {
        png.to_string_lossy().into_owned()
    } else {
        APP_ICON.to_string()
    }
}

/// mako (like most notification daemons) renders the body as Pango markup; names and errors come
/// from subscriptions, so show them as text.
fn escape(s: &str) -> String {
    v2baloot_core::util::clean_text(s, 400)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn spawn(urgency: &str, summary: &str, body: &str) {
    let summary = escape(summary);
    let body = escape(body);
    let icon = icon();
    let child = Command::new("notify-send")
        .args([
            "-a",
            "v2baloot",
            "-i",
            &icon,
            "-u",
            urgency,
            "-h",
            "string:x-canonical-private-synchronous:v2baloot",
        ])
        .arg("--")
        .arg(&summary)
        .arg(&body)
        .spawn();
    if let Ok(mut c) = child {
        // Reap it so we don't leave zombies behind.
        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(async move {
                let _ = c.wait().await;
            });
        }
    }
}

pub fn send(summary: &str, body: &str) {
    spawn("low", summary, body);
}

pub fn send_error(summary: &str, body: &str) {
    spawn("critical", summary, body);
}
