//! Running the xray process.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::{oneshot, watch};

const LOG_LINES: usize = 500;

#[derive(Clone, Default)]
pub struct LogBuf(Arc<Mutex<VecDeque<String>>>);

impl LogBuf {
    pub fn push(&self, line: impl Into<String>) {
        let mut g = self.0.lock().unwrap();
        if g.len() >= LOG_LINES {
            g.pop_front();
        }
        g.push_back(line.into());
    }
    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
    pub fn last_error(&self) -> Option<String> {
        let g = self.0.lock().unwrap();
        g.iter()
            .rev()
            .find(|l| l.contains("Failed") || l.contains("[Error]") || l.contains("error"))
            .cloned()
    }
}

pub struct XrayHandle {
    kill: Option<oneshot::Sender<()>>,
    done: Option<oneshot::Receiver<()>>,
    /// Becomes `Some(description)` when the process exits on its own.
    pub exited: watch::Receiver<Option<String>>,
}

impl XrayHandle {
    /// Stop xray and wait until it is gone (so its ports are free).
    pub async fn stop(mut self) {
        if let Some(k) = self.kill.take() {
            let _ = k.send(());
        }
        if let Some(d) = self.done.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), d).await;
        }
    }
}

fn pipe_lines<R: AsyncRead + Unpin + Send + 'static>(r: R, logs: LogBuf) {
    // Byte-based and lossy: a non-UTF-8 line must not stop us draining the pipe (xray would then
    // block on a full pipe and the proxy would hang).
    tokio::spawn(async move {
        let mut reader = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => break,
                Ok(_) => logs.push(String::from_utf8_lossy(&buf).trim_end().to_string()),
                Err(_) => continue,
            }
        }
    });
}

pub async fn spawn(xray: &str, config: &Path, logs: LogBuf) -> Result<XrayHandle> {
    logs.push(format!("--- starting {xray} run -c {}", config.display()));
    let mut child = Command::new(xray)
        .arg("run")
        .arg("-c")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot run `{xray}` — is xray installed? (pacman -S xray)"))?;
    if let Some(o) = child.stdout.take() {
        pipe_lines(o, logs.clone());
    }
    if let Some(e) = child.stderr.take() {
        pipe_lines(e, logs.clone());
    }

    let (kill_tx, kill_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let (exit_tx, exit_rx) = watch::channel(None);
    tokio::spawn(async move {
        tokio::select! {
            status = child.wait() => {
                let desc = match status {
                    Ok(s) => format!("xray exited ({s})"),
                    Err(e) => format!("xray wait failed: {e}"),
                };
                logs.push(format!("--- {desc}"));
                let _ = exit_tx.send(Some(desc));
            }
            _ = kill_rx => {
                let _ = child.kill().await;
                logs.push("--- xray stopped".to_string());
            }
        }
        let _ = done_tx.send(());
    });

    Ok(XrayHandle {
        kill: Some(kill_tx),
        done: Some(done_rx),
        exited: exit_rx,
    })
}

/// Wait for a local TCP port to accept connections, failing early if xray exits.
pub async fn wait_ready(
    host: &str,
    port: u16,
    exited: &mut watch::Receiver<Option<String>>,
    logs: &LogBuf,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let exited_now = exited.borrow().clone();
        if let Some(desc) = exited_now {
            // Give the stderr reader a moment to collect the reason.
            tokio::time::sleep(Duration::from_millis(100)).await;
            match logs.last_error() {
                Some(e) => bail!("{desc}: {}", clean_xray_error(&e)),
                None => bail!(desc),
            }
        }
        if TcpStream::connect((host, port)).await.is_ok() {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("xray did not open port {port} in time");
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

/// Xray errors are long `a > b > c` chains; the last part is the useful one.
pub fn clean_xray_error(line: &str) -> String {
    line.rsplit(" > ").next().unwrap_or(line).trim().to_string()
}
