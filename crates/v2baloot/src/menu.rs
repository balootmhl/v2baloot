//! `v2baloot menu`: quick picker through Walker's dmenu mode (or any dmenu-like command).

use anyhow::{bail, Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use v2baloot_core::ipc::{Client, Reply, Request};
use v2baloot_core::Server;

use crate::{launch, notify};

enum Action {
    Request(Request),
    ImportClipboard,
    OpenGui,
}

/// Numbered so two servers with the same name/kind/latency still get distinct lines.
fn server_line(n: usize, s: &Server, current: bool) -> String {
    let lat = s
        .latency
        .as_ref()
        .map(|l| l.label())
        .unwrap_or_else(|| "—".into());
    let name = s.name.replace(['\n', '\r'], " ");
    format!(
        "{} {n}. {name}    {} · {lat}",
        if current { "●" } else { "  " },
        s.kind_label()
    )
}

pub async fn run() -> Result<()> {
    let client = Client::default();
    let status = match client.status().await {
        Ok(s) => s,
        Err(e) => {
            notify::send_error("v2baloot", &format!("{e:#}"));
            return Err(e);
        }
    };
    let (servers, selected) = client.servers().await?;
    let settings = client.settings().await?;

    let mut entries: Vec<(String, Action)> = vec![];
    if status.is_active() {
        entries.push(("󰅖  Disconnect".into(), Action::Request(Request::Disconnect)));
    } else {
        entries.push((
            "󰌘  Connect".into(),
            Action::Request(Request::Connect { id: None }),
        ));
    }
    let other = status.mode.other();
    entries.push((
        format!("󰑓  Switch to {} mode", other.label()),
        Action::Request(Request::SetMode { mode: other }),
    ));
    entries.push((
        "󰓅  Test latency".into(),
        Action::Request(Request::TestLatency { ids: None }),
    ));
    entries.push((
        "󰚰  Update subscriptions".into(),
        Action::Request(Request::UpdateSubscriptions { id: None }),
    ));
    entries.push((
        "󰆒  Import link from clipboard".into(),
        Action::ImportClipboard,
    ));
    entries.push(("󰖂  Open v2baloot".into(), Action::OpenGui));
    let current = status.server_id.as_ref().or(selected.as_ref());
    for (i, s) in servers.iter().enumerate() {
        let is_current = current == Some(&s.id);
        entries.push((
            server_line(i + 1, s, is_current),
            Action::Request(Request::Connect {
                id: Some(s.id.clone()),
            }),
        ));
    }

    let input: String = entries.iter().map(|(l, _)| format!("{l}\n")).collect();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&settings.menu_command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("running `{}`", settings.menu_command))?;
    if let Some(mut stdin) = child.stdin.take() {
        // The picker may exit (Esc, already open) before reading everything.
        match stdin.write_all(input.as_bytes()).await {
            Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(e.into()),
            _ => {}
        }
    }
    let out = child.wait_with_output().await?;
    let choice = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    if choice.is_empty() {
        return Ok(());
    }
    let Some((_, action)) = entries.into_iter().find(|(l, _)| l.trim() == choice.trim()) else {
        bail!("unknown selection: {choice}");
    };

    let result = match action {
        Action::Request(req) => client.call(&req).await.map(|r| match r {
            Reply::Imported { added, .. } => Some(format!("{added} servers updated")),
            _ => None,
        }),
        Action::ImportClipboard => match launch::clipboard() {
            Some(text) => client
                .call(&Request::Import { text })
                .await
                .map(|r| match r {
                    Reply::Imported { added, .. } => Some(format!("Imported {added} server(s)")),
                    _ => None,
                }),
            None => Err(anyhow::anyhow!("clipboard is empty")),
        },
        Action::OpenGui => launch::gui().map(|_| None).map_err(Into::into),
    };
    match result {
        Ok(Some(msg)) => notify::send("v2baloot", &msg),
        Ok(None) => {}
        Err(e) => notify::send_error("v2baloot", &format!("{e:#}")),
    }
    // Let the notify-send child finish before the runtime shuts down.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    Ok(())
}
