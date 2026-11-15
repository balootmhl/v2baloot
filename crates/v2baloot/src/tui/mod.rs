//! Terminal UI (ratatui). Talks to the daemon over IPC like every other front-end.

mod ui;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent, EventStream, KeyCode,
    KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::StreamExt;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

use v2baloot_core::ipc::{Client, Event, NoticeLevel, Reply, Request};
use v2baloot_core::{Server, Status, Subscription};

use crate::launch;

pub enum Msg {
    Online(bool),
    Daemon(Event),
    Data {
        gen: u64,
        servers: Vec<Server>,
        selected: Option<String>,
        subs: Vec<Subscription>,
    },
    Done(Result<Option<String>, String>),
    Logs(Vec<String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    AddSubscription,
    Import,
}

pub enum Popup {
    None,
    Search,
    Input { kind: InputKind, text: String },
    Confirm { title: String, req: Request },
    Help,
    Logs { scroll: u16 },
}

#[derive(Clone, PartialEq, Eq)]
pub enum Tab {
    All,
    Sub(String),
    Manual,
}

pub struct App {
    client: Client,
    tx: mpsc::UnboundedSender<Msg>,
    pub online: bool,
    pub status: Status,
    pub servers: Vec<Server>,
    pub selected_id: Option<String>,
    pub subs: Vec<Subscription>,
    pub tab: usize,
    pub list: ListState,
    pub filter: String,
    pub sort_latency: bool,
    pub popup: Popup,
    pub message: Option<(String, bool)>,
    pub busy: Option<String>,
    pub logs: Vec<String>,
    /// Server under the cursor, tracked by id so re-sorting can't move what Enter acts on.
    cursor_id: Option<String>,
    refresh_gen: u64,
    quit: bool,
}

impl App {
    pub fn tabs(&self) -> Vec<(Tab, String)> {
        let mut t = vec![(Tab::All, "All".to_string())];
        for s in &self.subs {
            t.push((Tab::Sub(s.id.clone()), s.name.clone()));
        }
        if self.servers.iter().any(|s| s.group.is_none()) && !self.subs.is_empty() {
            t.push((Tab::Manual, "Manual".to_string()));
        }
        t
    }

    pub fn current_tab(&self) -> Tab {
        self.tabs()
            .get(self.tab)
            .map(|t| t.0.clone())
            .unwrap_or(Tab::All)
    }

    /// Servers shown in the list, after tab, search and sort.
    pub fn visible(&self) -> Vec<&Server> {
        let tab = self.current_tab();
        let f = self.filter.to_lowercase();
        let mut v: Vec<&Server> = self
            .servers
            .iter()
            .filter(|s| match &tab {
                Tab::All => true,
                Tab::Sub(id) => s.group.as_ref() == Some(id),
                Tab::Manual => s.group.is_none(),
            })
            .filter(|s| {
                f.is_empty()
                    || s.name.to_lowercase().contains(&f)
                    || s.address.to_lowercase().contains(&f)
            })
            .collect();
        if self.sort_latency {
            v.sort_by_key(|s| s.latency_ms().unwrap_or(u32::MAX));
        }
        v
    }

    pub fn current(&self) -> Option<&Server> {
        let id = self.cursor_id.as_deref()?;
        self.visible().into_iter().find(|s| s.id == id)
    }

    /// Move the cursor to row `i` of the visible list.
    fn set_cursor(&mut self, i: usize) {
        let id = {
            let v = self.visible();
            if v.is_empty() {
                None
            } else {
                Some(v[i.min(v.len() - 1)].id.clone())
            }
        };
        self.cursor_id = id;
        self.sync_cursor();
    }

    /// Recompute the highlighted row from `cursor_id` (rows may have been re-sorted or filtered).
    fn sync_cursor(&mut self) {
        let (idx, first) = {
            let v = self.visible();
            let idx = self
                .cursor_id
                .as_deref()
                .and_then(|id| v.iter().position(|s| s.id == id));
            (idx, v.first().map(|s| s.id.clone()))
        };
        match idx {
            Some(i) => self.list.select(Some(i)),
            None => {
                self.cursor_id = first;
                self.list.select(self.cursor_id.as_ref().map(|_| 0));
            }
        }
    }

    fn select_id(&mut self, id: Option<&str>) {
        if let Some(id) = id {
            self.cursor_id = Some(id.to_string());
        }
        self.sync_cursor();
    }

    fn info(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), false));
    }

    fn error(&mut self, m: impl Into<String>) {
        self.message = Some((m.into(), true));
    }

    fn refresh(&mut self) {
        self.refresh_gen += 1;
        let gen = self.refresh_gen;
        let c = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let (Ok((servers, selected)), Ok(subs)) =
                (c.servers().await, c.subscriptions().await)
            {
                let _ = tx.send(Msg::Data {
                    gen,
                    servers,
                    selected,
                    subs,
                });
            }
        });
    }

    fn request(&mut self, req: Request, busy: Option<&str>) {
        if let Some(b) = busy {
            self.busy = Some(b.to_string());
        }
        let c = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let res = match c.call(&req).await {
                Ok(Reply::Imported { added, errors }) => {
                    let mut m = format!("{added} server(s) added/updated");
                    if !errors.is_empty() {
                        m.push_str(&format!(" · {} skipped", errors.len()));
                    }
                    Ok(Some(m))
                }
                Ok(_) => Ok(None),
                Err(e) => Err(format!("{e:#}")),
            };
            let _ = tx.send(Msg::Done(res));
        });
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Online(on) => {
                self.online = on;
                if on {
                    self.refresh();
                }
            }
            Msg::Data {
                gen,
                servers,
                selected,
                subs,
            } => {
                if gen != self.refresh_gen {
                    return; // an older refresh finished late
                }
                let keep = self.cursor_id.clone().or(selected.clone());
                self.servers = servers;
                self.selected_id = selected;
                self.subs = subs;
                if self.tab >= self.tabs().len() {
                    self.tab = 0;
                }
                self.select_id(keep.as_deref());
            }
            Msg::Done(res) => {
                self.busy = None;
                match res {
                    Ok(Some(m)) => self.info(m),
                    Ok(None) => {}
                    Err(e) => self.error(e),
                }
            }
            Msg::Logs(lines) => {
                self.logs = lines;
                if matches!(self.popup, Popup::None) {
                    let scroll = self.logs.len().saturating_sub(10) as u16;
                    self.popup = Popup::Logs { scroll };
                }
            }
            Msg::Daemon(ev) => match ev {
                Event::Status { status } => {
                    if let v2baloot_core::ConnState::Error { message } = &status.state {
                        if self.status.state != status.state {
                            self.error(message.clone());
                        }
                    }
                    if status.server_id.is_some() {
                        self.selected_id = status.server_id.clone();
                    }
                    self.status = status;
                }
                Event::ServersChanged => self.refresh(),
                Event::Latency { id, latency } => {
                    if let Some(s) = self.servers.iter_mut().find(|s| s.id == id) {
                        s.latency = Some(latency);
                    }
                }
                Event::TestFinished => self.info("Latency test finished"),
                Event::SettingsChanged => {}
                Event::Notice { level, message } => match level {
                    NoticeLevel::Info => self.info(message),
                    NoticeLevel::Error => self.error(message),
                },
            },
        }
    }

    fn move_by(&mut self, delta: isize) {
        self.sync_cursor();
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        let i = self.list.selected().unwrap_or(0) as isize + delta;
        self.set_cursor(i.clamp(0, n as isize - 1) as usize);
    }

    fn switch_tab(&mut self, delta: isize) {
        let n = self.tabs().len() as isize;
        self.tab = ((self.tab as isize + delta).rem_euclid(n)) as usize;
        self.set_cursor(0);
    }

    /// Bracketed paste: the whole pasted text arrives at once instead of as keystrokes.
    fn on_paste(&mut self, text: String) {
        // Terminals usually send pasted newlines as CR.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        match &mut self.popup {
            Popup::Input { kind, text: t } => {
                if *kind == InputKind::AddSubscription {
                    t.push_str(text.trim());
                } else {
                    t.push_str(&text);
                    t.push('\n');
                }
            }
            Popup::Search => {
                self.filter.push_str(&text.replace(['\n', '\r'], " "));
                self.set_cursor(0);
            }
            Popup::None if text.contains("://") && self.online => {
                self.request(Request::Import { text }, None)
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        // Ctrl/Alt chords are not ours (e.g. Ctrl+V must not type a `v`).
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        match &mut self.popup {
            Popup::None => {}
            Popup::Search => {
                let before = self.filter.clone();
                match key.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.popup = Popup::None;
                    }
                    KeyCode::Enter => self.popup = Popup::None,
                    KeyCode::Backspace => {
                        self.filter.pop();
                    }
                    KeyCode::Char(c) => self.filter.push(c),
                    KeyCode::Down => self.move_by(1),
                    KeyCode::Up => self.move_by(-1),
                    _ => {}
                }
                if self.filter != before {
                    self.set_cursor(0);
                }
                return;
            }
            Popup::Input { kind, text } => {
                match key.code {
                    KeyCode::Esc => self.popup = Popup::None,
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char(c) => text.push(c),
                    KeyCode::Enter => {
                        let text = std::mem::take(text);
                        let kind = *kind;
                        self.popup = Popup::None;
                        if !text.trim().is_empty() {
                            match kind {
                                InputKind::AddSubscription => self.request(
                                    Request::AddSubscription {
                                        name: String::new(),
                                        url: text,
                                    },
                                    Some("Adding subscription…"),
                                ),
                                InputKind::Import => self.request(Request::Import { text }, None),
                            }
                        }
                    }
                    _ => {}
                }
                return;
            }
            Popup::Confirm { req, .. } => {
                if matches!(
                    key.code,
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
                ) {
                    let req = req.clone();
                    self.request(req, None);
                }
                self.popup = Popup::None;
                return;
            }
            Popup::Help => {
                self.popup = Popup::None;
                return;
            }
            Popup::Logs { scroll } => {
                match key.code {
                    KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                    KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    _ => self.popup = Popup::None,
                }
                return;
            }
        }

        if !self.online
            && !matches!(
                key.code,
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('?')
            )
        {
            self.error("daemon is not running — systemctl --user start v2baloot");
            return;
        }
        self.message = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::PageDown => self.move_by(10),
            KeyCode::PageUp => self.move_by(-10),
            KeyCode::Char('g') | KeyCode::Home => self.set_cursor(0),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => self.switch_tab(1),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::Enter => {
                if let Some(s) = self.current() {
                    let id = s.id.clone();
                    let name = s.name.clone();
                    self.request(
                        Request::Connect { id: Some(id) },
                        Some(&format!("Connecting to {name}…")),
                    );
                }
            }
            KeyCode::Char('d') => self.request(Request::Disconnect, None),
            KeyCode::Char('c') | KeyCode::Char(' ') => self.request(Request::Toggle, Some("…")),
            KeyCode::Char('m') => {
                let mode = self.status.mode.other();
                self.request(
                    Request::SetMode { mode },
                    Some(&format!("Switching to {}…", mode.label())),
                );
            }
            KeyCode::Char('t') => {
                if let Some(s) = self.current() {
                    let ids = Some(vec![s.id.clone()]);
                    self.request(Request::TestLatency { ids }, None);
                }
            }
            KeyCode::Char('T') => {
                let ids: Vec<String> = self.visible().iter().map(|s| s.id.clone()).collect();
                if !ids.is_empty() {
                    self.request(Request::TestLatency { ids: Some(ids) }, None);
                }
            }
            KeyCode::Char('s') => {
                self.sort_latency = !self.sort_latency;
                self.sync_cursor();
                self.info(if self.sort_latency {
                    "Sorted by latency"
                } else {
                    "Original order"
                });
            }
            KeyCode::Char('/') => self.popup = Popup::Search,
            KeyCode::Char('i') => match launch::clipboard() {
                Some(text) if !text.trim().is_empty() => {
                    self.request(Request::Import { text }, None)
                }
                _ => {
                    self.popup = Popup::Input {
                        kind: InputKind::Import,
                        text: String::new(),
                    }
                }
            },
            KeyCode::Char('I') => {
                self.popup = Popup::Input {
                    kind: InputKind::Import,
                    text: String::new(),
                }
            }
            KeyCode::Char('a') => {
                self.popup = Popup::Input {
                    kind: InputKind::AddSubscription,
                    text: String::new(),
                }
            }
            KeyCode::Char('u') => {
                let id = match self.current_tab() {
                    Tab::Sub(id) => Some(id),
                    _ => None,
                };
                self.request(
                    Request::UpdateSubscriptions { id },
                    Some("Updating subscriptions…"),
                );
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                if let Some(s) = self.current() {
                    self.popup = Popup::Confirm {
                        title: format!("Delete server “{}”?", s.name),
                        req: Request::RemoveServer { id: s.id.clone() },
                    };
                }
            }
            KeyCode::Char('X') => {
                if let Tab::Sub(id) = self.current_tab() {
                    let name = self
                        .subs
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    self.popup = Popup::Confirm {
                        title: format!("Delete subscription “{name}” and its servers?"),
                        req: Request::RemoveSubscription { id },
                    };
                } else {
                    self.error("Switch to a subscription tab first (h/l)");
                }
            }
            KeyCode::Char('L') => {
                let c = self.client.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if let Ok(Reply::Logs { lines }) = c.call(&Request::Logs).await {
                        let _ = tx.send(Msg::Logs(lines));
                    }
                });
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('?') => self.popup = Popup::Help,
            _ => {}
        }
    }
}

fn spawn_event_listener(client: Client, tx: mpsc::UnboundedSender<Msg>) {
    tokio::spawn(async move {
        loop {
            if let Ok(mut events) = client.subscribe().await {
                let _ = tx.send(Msg::Online(true));
                while let Ok(Some(ev)) = events.next().await {
                    if tx.send(Msg::Daemon(ev)).is_err() {
                        return;
                    }
                }
            }
            if tx.send(Msg::Online(false)).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub async fn run() -> Result<()> {
    let client = Client::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App {
        client: client.clone(),
        tx: tx.clone(),
        online: false,
        status: Status::default(),
        servers: vec![],
        selected_id: None,
        subs: vec![],
        tab: 0,
        list: ListState::default(),
        filter: String::new(),
        sort_latency: false,
        popup: Popup::None,
        message: None,
        busy: None,
        logs: vec![],
        cursor_id: None,
        refresh_gen: 0,
        quit: false,
    };
    spawn_event_listener(client, tx);

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(std::io::stdout(), EnableBracketedPaste);
    let mut keys = EventStream::new();
    let result: Result<()> = async {
        while !app.quit {
            app.sync_cursor();
            terminal.draw(|f| ui::draw(f, &mut app))?;
            tokio::select! {
                Some(ev) = keys.next() => match ev {
                    Ok(TermEvent::Key(k)) => app.on_key(k),
                    Ok(TermEvent::Paste(text)) => app.on_paste(text),
                    _ => {}
                },
                Some(msg) = rx.recv() => app.on_msg(msg),
            }
        }
        Ok(())
    }
    .await;
    let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}
