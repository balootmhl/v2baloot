//! Desktop notifications via `notify-send` (shown by mako on Omarchy).

use tokio::process::Command;

pub const APP_ICON: &str = "org.omarchy.v2baloot";

fn spawn(urgency: &str, summary: &str, body: &str) {
    let child = Command::new("notify-send")
        .args([
            "-a",
            "v2baloot",
            "-i",
            APP_ICON,
            "-u",
            urgency,
            "-h",
            "string:x-canonical-private-synchronous:v2baloot",
        ])
        .arg("--")
        .arg(summary)
        .arg(body)
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
