//! Launching the GUI / TUI as separate desktop apps.

use std::process::{Command, Stdio};

pub const GUI_BIN: &str = "v2baloot-gui";
pub const TUI_APP_ID: &str = "org.omarchy.v2baloot.tui";

fn has(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

/// Run detached; on Omarchy go through `uwsm app` so the app gets its own systemd scope.
fn detach(args: &[&str]) -> std::io::Result<()> {
    let mut full: Vec<&str> = vec![];
    if has("uwsm") {
        full.extend(["uwsm", "app", "--"]);
    } else if has("setsid") {
        full.push("setsid");
    }
    full.extend(args);
    let mut child = Command::new(full[0])
        .args(&full[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // uwsm/setsid return quickly; reap them in a thread so no zombie lingers.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn gui() -> std::io::Result<()> {
    detach(&[GUI_BIN])
}

pub fn tui() -> std::io::Result<()> {
    // After a package upgrade /proc/self/exe reads "… (deleted)"; fall back to PATH then.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .filter(|p| !p.ends_with(" (deleted)") && std::path::Path::new(p).exists())
        .unwrap_or_else(|| "v2baloot".into());
    if has("xdg-terminal-exec") {
        detach(&[
            "xdg-terminal-exec",
            &format!("--app-id={TUI_APP_ID}"),
            "-e",
            &exe,
            "tui",
        ])
    } else {
        detach(&["alacritty", "--class", TUI_APP_ID, "-e", &exe, "tui"])
    }
}

/// Read text from the Wayland clipboard (pbpaste on macOS for development).
pub fn clipboard() -> Option<String> {
    let out = if cfg!(target_os = "macos") {
        Command::new("pbpaste").output().ok()?
    } else {
        Command::new("wl-paste").arg("--no-newline").output().ok()?
    };
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}
