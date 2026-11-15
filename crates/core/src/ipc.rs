//! Daemon IPC: newline-delimited JSON over a Unix socket.
//!
//! Each connection sends one [`Request`] and gets one [`Reply`]. A `Subscribe` request instead
//! keeps the connection open and streams [`Reply::Event`]s.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;

use crate::model::*;
use crate::paths::Paths;
use crate::store::Settings;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    ListServers,
    ListSubscriptions,
    /// `None` = last selected server.
    Connect {
        id: Option<String>,
    },
    Disconnect,
    Toggle,
    SetMode {
        mode: Mode,
    },
    /// One or more share links (or base64 subscription content).
    Import {
        text: String,
    },
    AddSubscription {
        name: String,
        url: String,
    },
    RemoveSubscription {
        id: String,
    },
    /// `None` = all subscriptions.
    UpdateSubscriptions {
        id: Option<String>,
    },
    RemoveServer {
        id: String,
    },
    /// `None` = every server. Results arrive as `Event::Latency`.
    TestLatency {
        ids: Option<Vec<String>>,
    },
    Logs,
    GetSettings,
    SetSettings {
        settings: Box<Settings>,
    },
    Subscribe,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Error {
        message: String,
    },
    Status {
        status: Status,
    },
    Servers {
        servers: Vec<Server>,
        selected: Option<String>,
    },
    Subscriptions {
        subscriptions: Vec<Subscription>,
    },
    Imported {
        added: usize,
        errors: Vec<String>,
    },
    Logs {
        lines: Vec<String>,
    },
    Settings {
        settings: Box<Settings>,
    },
    Event {
        event: Event,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Status {
        status: Status,
    },
    /// Server list or subscriptions changed; refetch.
    ServersChanged,
    Latency {
        id: String,
        latency: Latency,
    },
    TestFinished,
    /// Settings were changed; re-read them if you use any.
    SettingsChanged,
    Notice {
        level: NoticeLevel,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Error,
}

#[derive(Debug, Clone)]
pub struct Client {
    socket: PathBuf,
}

impl Default for Client {
    fn default() -> Self {
        Client::new(Paths::current().socket())
    }
}

impl Client {
    pub fn new(socket: impl AsRef<Path>) -> Client {
        Client {
            socket: socket.as_ref().to_path_buf(),
        }
    }

    async fn connect(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.socket).await.map_err(|e| {
            anyhow!(
                "cannot reach v2baloot daemon at {} ({e}). Start it with: systemctl --user start v2baloot",
                self.socket.display()
            )
        })?;
        // Only talk to our own daemon: links, subscription URLs and settings go over this socket.
        #[cfg(unix)]
        {
            let peer = stream.peer_cred()?.uid();
            let me = crate::util::current_uid();
            if peer != me {
                bail!(
                    "{} belongs to another user (uid {peer}); refusing to use it",
                    self.socket.display()
                );
            }
        }
        Ok(stream)
    }

    /// Send a request; daemon-side errors become `Err`.
    pub async fn call(&self, req: &Request) -> Result<Reply> {
        let stream = self.connect().await?;
        let (r, mut w) = stream.into_split();
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        w.write_all(line.as_bytes()).await?;
        let mut lines = BufReader::new(r).lines();
        let resp = lines
            .next_line()
            .await?
            .context("daemon closed the connection")?;
        match serde_json::from_str(&resp).context("invalid reply from daemon")? {
            Reply::Error { message } => bail!(message),
            r => Ok(r),
        }
    }

    pub async fn status(&self) -> Result<Status> {
        match self.call(&Request::Status).await? {
            Reply::Status { status } => Ok(status),
            r => bail!("unexpected reply {r:?}"),
        }
    }

    pub async fn servers(&self) -> Result<(Vec<Server>, Option<String>)> {
        match self.call(&Request::ListServers).await? {
            Reply::Servers { servers, selected } => Ok((servers, selected)),
            r => bail!("unexpected reply {r:?}"),
        }
    }

    pub async fn subscriptions(&self) -> Result<Vec<Subscription>> {
        match self.call(&Request::ListSubscriptions).await? {
            Reply::Subscriptions { subscriptions } => Ok(subscriptions),
            r => bail!("unexpected reply {r:?}"),
        }
    }

    pub async fn settings(&self) -> Result<Settings> {
        match self.call(&Request::GetSettings).await? {
            Reply::Settings { settings } => Ok(*settings),
            r => bail!("unexpected reply {r:?}"),
        }
    }

    /// Open an event stream.
    pub async fn subscribe(&self) -> Result<EventStream> {
        let stream = self.connect().await?;
        let (r, mut w) = stream.into_split();
        let mut line = serde_json::to_string(&Request::Subscribe)?;
        line.push('\n');
        w.write_all(line.as_bytes()).await?;
        Ok(EventStream {
            lines: BufReader::new(r).lines(),
            _w: w,
        })
    }
}

pub struct EventStream {
    lines: Lines<BufReader<OwnedReadHalf>>,
    _w: tokio::net::unix::OwnedWriteHalf,
}

impl EventStream {
    /// Next event; `Ok(None)` when the daemon goes away.
    pub async fn next(&mut self) -> Result<Option<Event>> {
        loop {
            let Some(line) = self.lines.next_line().await? else {
                return Ok(None);
            };
            match serde_json::from_str::<Reply>(&line) {
                Ok(Reply::Event { event }) => return Ok(Some(event)),
                Ok(_) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}
