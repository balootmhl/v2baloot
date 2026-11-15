//! Main window: dashboard (left) + server list (right). On narrow windows the dashboard comes
//! first and the server list is pushed as a second page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use v2baloot_core::ipc::{Client, Event, NoticeLevel, Reply, Request};
use v2baloot_core::{link, Mode, Server, Status, Subscription};

use crate::dashboard::{self, Dashboard};
use crate::servers::ServersPane;
use crate::{call, dialogs, rt, theme};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Group {
    All,
    Sub(String),
    Manual,
}

#[derive(Default)]
pub struct State {
    pub online: bool,
    pub status: Status,
    pub servers: Vec<Server>,
    pub selected: Option<String>,
    pub subs: Vec<Subscription>,
    pub group: Option<Group>,
    pub filter: String,
    pub sort_latency: bool,
}

pub struct Ui {
    pub window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    split: adw::NavigationSplitView,
    dash: Dashboard,
    servers: ServersPane,
    servers_title: adw::WindowTitle,
    test_btn: gtk::Button,
    pub state: RefCell<State>,
    /// Set while we push daemon state into widgets so their handlers don't fire requests.
    updating: Cell<bool>,
    /// Bumped per refresh; results from older refreshes are dropped.
    refresh_gen: Cell<u64>,
}

enum Msg {
    Online(bool),
    Event(Box<Event>),
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let s1 = gio::Menu::new();
    s1.append(Some("Import from Clipboard"), Some("win.import"));
    s1.append(Some("Add Subscription…"), Some("win.add-sub"));
    s1.append(Some("Update Subscriptions"), Some("win.update-subs"));
    menu.append_section(None, &s1);
    let s2 = gio::Menu::new();
    s2.append(Some("Sort by Latency"), Some("win.sort-latency"));
    s2.append(Some("Xray Log"), Some("win.logs"));
    menu.append_section(None, &s2);
    let s3 = gio::Menu::new();
    s3.append(Some("Preferences"), Some("win.prefs"));
    s3.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    s3.append(Some("About v2baloot"), Some("win.about"));
    menu.append_section(None, &s3);
    menu
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("v2baloot")
        .default_width(1040)
        .default_height(700)
        .width_request(360)
        .height_request(480)
        .build();

    // ---------------- dashboard page
    let dash = Dashboard::new();
    let dash_header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("v2baloot", ""))
        .build();
    dash_header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu())
            .primary(true)
            .tooltip_text("Main Menu")
            .build(),
    );
    let to_servers = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("view-list-symbolic")
                .label("Servers")
                .build(),
        )
        .tooltip_text("Server list")
        .visible(false)
        .build();
    dash_header.pack_start(&to_servers);
    let dash_view = adw::ToolbarView::new();
    dash_view.add_top_bar(&dash_header);
    dash_view.set_content(Some(&dash.root));
    let dash_page = adw::NavigationPage::builder()
        .title("v2baloot")
        .tag("dashboard")
        .child(&dash_view)
        .build();

    // ---------------- servers page
    let servers = ServersPane::new();
    let servers_title = adw::WindowTitle::new("Servers", "");
    let servers_header = adw::HeaderBar::builder()
        .title_widget(&servers_title)
        .build();
    let search_btn = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search (Ctrl+F)")
        .build();
    search_btn
        .bind_property("active", &servers.search_bar, "search-mode-enabled")
        .bidirectional()
        .build();
    servers_header.pack_start(&search_btn);
    let test_btn = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("network-transmit-receive-symbolic")
                .label("Test")
                .build(),
        )
        .tooltip_text("Test latency of the shown servers (Ctrl+T)")
        .action_name("win.test-all")
        .build();
    servers_header.pack_end(&test_btn);
    let servers_view = adw::ToolbarView::new();
    servers_view.add_top_bar(&servers_header);
    servers_view.set_content(Some(&servers.root));
    let servers_page = adw::NavigationPage::builder()
        .title("Servers")
        .tag("servers")
        .child(&servers_view)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&dash_page)
        .content(&servers_page)
        .min_sidebar_width(340.0)
        .max_sidebar_width(430.0)
        .sidebar_width_fraction(0.40)
        .build();
    split
        .bind_property("collapsed", &to_servers, "visible")
        .sync_create()
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        760.0,
        adw::LengthUnit::Sp,
    ));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp);

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts,
        split,
        dash,
        servers,
        servers_title,
        test_btn,
        state: RefCell::default(),
        updating: Cell::new(false),
        refresh_gen: Cell::new(0),
    });

    let u = Rc::clone(&ui);
    to_servers.connect_clicked(move |_| u.split.set_show_content(true));
    wire_signals(&ui);
    install_actions(&ui);
    ui.render_all();
    listen(&ui);

    let weak = Rc::downgrade(&ui);
    dashboard::start_clock(move || {
        if let Some(ui) = weak.upgrade() {
            ui.dash.tick(&ui.state.borrow());
        }
    });
    let weak = Rc::downgrade(&ui);
    theme::on_change(move || {
        if let Some(ui) = weak.upgrade() {
            ui.dash.ring.widget.queue_draw();
            ui.dash.spark.widget.queue_draw();
        }
    });
    window
}

fn wire_signals(ui: &Rc<Ui>) {
    let u = Rc::clone(ui);
    ui.dash.ring.connect_clicked(move || {
        let (online, active, has_servers) = {
            let st = u.state.borrow();
            (st.online, st.status.is_active(), !st.servers.is_empty())
        };
        if !online {
            u.error("The v2baloot daemon is not running");
        } else if !active && !has_servers {
            u.split.set_show_content(true);
            dialogs::import(&u);
        } else {
            u.request(if active {
                Request::Disconnect
            } else {
                Request::Connect { id: None }
            });
        }
    });

    let u = Rc::clone(ui);
    ui.dash.mode.connect_active_name_notify(move |g| {
        if u.updating.get() {
            return;
        }
        let mode = if g.active_name().as_deref() == Some("tun") {
            Mode::Tun
        } else {
            Mode::Proxy
        };
        u.request(Request::SetMode { mode });
    });

    let u = Rc::clone(ui);
    ui.dash.connect_server_card(move || {
        u.split.set_show_content(true);
        let id = {
            let st = u.state.borrow();
            st.status.server_id.clone().or(st.selected.clone())
        };
        if let Some(id) = id {
            u.servers.reveal(&id);
        }
    });

    let u = Rc::clone(ui);
    ui.servers.list.connect_row_activated(move |_, row| {
        u.request(Request::Connect {
            id: Some(row.widget_name().to_string()),
        });
    });

    let u = Rc::clone(ui);
    ui.servers.search.connect_search_changed(move |e| {
        u.state.borrow_mut().filter = e.text().to_string();
        u.servers.render(&u.state.borrow());
    });

    let weak = Rc::downgrade(ui);
    ui.servers.connect_group_changed(move |g| {
        let weak = weak.clone();
        // Re-rendering replaces the pill that is emitting this signal; do it after it returns.
        glib::idle_add_local_once(move || {
            if let Some(u) = weak.upgrade() {
                u.state.borrow_mut().group = Some(g);
                u.servers.render(&u.state.borrow());
            }
        });
    });
}

fn add_action(ui: &Rc<Ui>, name: &str, f: impl Fn(&Rc<Ui>) + 'static) {
    let a = gio::SimpleAction::new(name, None);
    let u = Rc::clone(ui);
    a.connect_activate(move |_, _| f(&u));
    ui.window.add_action(&a);
}

fn add_id_action(ui: &Rc<Ui>, name: &str, f: impl Fn(&Rc<Ui>, String) + 'static) {
    let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
    let u = Rc::clone(ui);
    a.connect_activate(move |_, p| {
        if let Some(id) = p.and_then(|v| v.get::<String>()) {
            f(&u, id);
        }
    });
    ui.window.add_action(&a);
}

fn find_server(u: &Ui, id: &str) -> Option<Server> {
    u.state
        .borrow()
        .servers
        .iter()
        .find(|s| s.id == id)
        .cloned()
}

fn install_actions(ui: &Rc<Ui>) {
    add_action(ui, "import", dialogs::import);
    add_action(ui, "add-sub", dialogs::add_subscription);
    add_action(ui, "update-subs", |u| {
        u.toast("Updating subscriptions…");
        u.request(Request::UpdateSubscriptions { id: None });
    });
    add_action(ui, "test-all", |u| {
        let ids: Vec<String> = ServersPane::visible(&u.state.borrow())
            .iter()
            .map(|s| s.id.clone())
            .collect();
        if !ids.is_empty() {
            u.servers.begin_test(ids.len());
            u.split.set_show_content(true);
            u.request(Request::TestLatency { ids: Some(ids) });
        }
    });
    add_action(ui, "search", |u| {
        u.split.set_show_content(true);
        u.servers
            .search_bar
            .set_search_mode(!u.servers.search_bar.is_search_mode())
    });
    add_action(ui, "toggle", |u| u.request(Request::Toggle));
    add_action(ui, "prefs", dialogs::preferences);
    add_action(ui, "logs", dialogs::logs);
    add_action(ui, "about", dialogs::about);
    add_action(ui, "shortcuts", dialogs::shortcuts);

    let sort = gio::SimpleAction::new_stateful("sort-latency", None, &false.to_variant());
    let u = Rc::clone(ui);
    sort.connect_activate(move |a, _| {
        let on = !a.state().and_then(|s| s.get::<bool>()).unwrap_or(false);
        a.set_state(&on.to_variant());
        u.state.borrow_mut().sort_latency = on;
        u.servers.render(&u.state.borrow());
    });
    ui.window.add_action(&sort);

    add_id_action(ui, "connect-server", |u, id| {
        u.request(Request::Connect { id: Some(id) })
    });
    add_id_action(ui, "test-server", |u, id| {
        u.servers.begin_test(1);
        u.request(Request::TestLatency {
            ids: Some(vec![id]),
        })
    });
    add_id_action(ui, "copy-link", |u, id| {
        if let Some(s) = find_server(u, &id) {
            u.window.clipboard().set_text(&link::to_link(&s));
            u.toast("Share link copied");
        }
    });
    add_id_action(ui, "details", |u, id| {
        if let Some(s) = find_server(u, &id) {
            dialogs::details(u, &s);
        }
    });
    add_id_action(ui, "delete-server", |u, id| {
        let name = find_server(u, &id).map(|s| s.name).unwrap_or_default();
        dialogs::confirm(u, &format!("Delete “{name}”?"), "Delete", move |u| {
            u.request(Request::RemoveServer { id: id.clone() })
        });
    });
    add_id_action(ui, "update-sub", |u, id| {
        u.toast("Updating subscription…");
        u.request(Request::UpdateSubscriptions { id: Some(id) });
    });
    add_id_action(ui, "delete-sub", |u, id| {
        let name = u
            .state
            .borrow()
            .subs
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        dialogs::confirm(
            u,
            &format!("Delete subscription “{name}”?"),
            "Delete",
            move |u| u.request(Request::RemoveSubscription { id: id.clone() }),
        );
    });
}

/// Pump daemon events into the GTK main loop.
fn listen(ui: &Rc<Ui>) {
    let (tx, rx) = async_channel::unbounded::<Msg>();
    rt().spawn(async move {
        let client = Client::default();
        loop {
            if let Ok(mut events) = client.subscribe().await {
                if tx.send(Msg::Online(true)).await.is_err() {
                    return;
                }
                while let Ok(Some(ev)) = events.next().await {
                    if tx.send(Msg::Event(Box::new(ev))).await.is_err() {
                        return;
                    }
                }
            }
            if tx.send(Msg::Online(false)).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    let weak = Rc::downgrade(ui);
    glib::spawn_future_local(async move {
        while let Ok(msg) = rx.recv().await {
            let Some(ui) = weak.upgrade() else { break };
            ui.on_msg(msg);
        }
    });
}

impl Ui {
    pub fn toast(&self, msg: &str) {
        let t = adw::Toast::builder()
            .title(glib::markup_escape_text(msg))
            .timeout(3)
            .build();
        self.toasts.add_toast(t);
    }

    pub fn error(&self, msg: &str) {
        let first = msg.lines().next().unwrap_or(msg);
        let t = adw::Toast::builder()
            .title(glib::markup_escape_text(first))
            .timeout(6)
            .priority(adw::ToastPriority::High)
            .build();
        self.toasts.add_toast(t);
    }

    /// Send a request; errors become toasts, imports report how many servers changed.
    pub fn request(self: &Rc<Self>, req: Request) {
        let weak = Rc::downgrade(self);
        call(req, move |res| {
            let Some(ui) = weak.upgrade() else { return };
            match res {
                Ok(Reply::Imported { added, errors }) => {
                    let mut m = format!("{added} server(s) added or updated");
                    if !errors.is_empty() {
                        m.push_str(&format!(", {} skipped", errors.len()));
                    }
                    ui.toast(&m);
                }
                Ok(_) => {}
                Err(e) => {
                    ui.servers.end_test();
                    ui.error(&e)
                }
            }
        });
    }

    fn refresh(self: &Rc<Self>) {
        let gen = self.refresh_gen.get() + 1;
        self.refresh_gen.set(gen);
        let weak = Rc::downgrade(self);
        let handle = rt().spawn(async {
            let c = Client::default();
            match (c.servers().await, c.subscriptions().await) {
                (Ok((servers, selected)), Ok(subs)) => Some((servers, selected, subs)),
                _ => None,
            }
        });
        glib::spawn_future_local(async move {
            let Ok(Some((servers, selected, subs))) = handle.await else {
                return;
            };
            let Some(ui) = weak.upgrade() else { return };
            if ui.refresh_gen.get() != gen {
                return; // a newer refresh is in flight
            }
            {
                let mut st = ui.state.borrow_mut();
                st.servers = servers;
                st.selected = selected;
                st.subs = subs;
                let manual = st.servers.iter().any(|s| s.group.is_none()) && !st.subs.is_empty();
                let group_ok = match &st.group {
                    Some(Group::Sub(id)) => st.subs.iter().any(|s| &s.id == id),
                    Some(Group::Manual) => manual,
                    _ => true,
                };
                if !group_ok {
                    st.group = Some(Group::All);
                }
            }
            ui.render_all();
        });
    }

    fn on_msg(self: &Rc<Self>, msg: Msg) {
        match msg {
            Msg::Online(on) => {
                let was = self.state.borrow().online;
                self.state.borrow_mut().online = on;
                if on && !was {
                    self.refresh();
                }
                self.render_all();
            }
            Msg::Event(ev) => match *ev {
                Event::Status { status } => {
                    let prev_testing = self.state.borrow().status.testing;
                    {
                        let mut st = self.state.borrow_mut();
                        if status.server_id.is_some() {
                            st.selected = status.server_id.clone();
                        }
                        st.status = status.clone();
                    }
                    if status.testing && !prev_testing && self.state.borrow().servers.len() > 1 {
                        // Started from elsewhere (tray, CLI): size the bar for all servers.
                        let n = self.state.borrow().servers.len();
                        self.servers.begin_test(n);
                    }
                    if !status.testing && prev_testing {
                        self.servers.end_test();
                    }
                    if status.is_connected() {
                        self.dash.spark.push(status.down_speed, status.up_speed);
                    }
                    self.render_status();
                }
                Event::ServersChanged => self.refresh(),
                Event::Latency { id, latency } => {
                    let current = {
                        let mut st = self.state.borrow_mut();
                        if let Some(s) = st.servers.iter_mut().find(|s| s.id == id) {
                            s.latency = Some(latency.clone());
                        }
                        st.status.server_id.as_ref().or(st.selected.as_ref()) == Some(&id)
                    };
                    self.servers.set_latency(&id, &latency);
                    if current {
                        self.render_status();
                    }
                }
                Event::TestFinished => {
                    self.servers.end_test();
                    if self.state.borrow().sort_latency {
                        self.servers.render(&self.state.borrow());
                    }
                }
                Event::SettingsChanged => {}
                Event::Notice { level, message } => match level {
                    NoticeLevel::Info => self.toast(&message),
                    NoticeLevel::Error => self.error(&message),
                },
            },
        }
    }

    fn render_all(self: &Rc<Self>) {
        {
            let st = self.state.borrow();
            self.servers.render(&st);
            let n = st.servers.len();
            self.servers_title.set_subtitle(&match n {
                0 => String::new(),
                1 => "1 server".into(),
                n => format!("{n} servers"),
            });
        }
        self.render_status();
    }

    fn render_status(&self) {
        let st = self.state.borrow();
        self.updating.set(true);
        self.dash.update(&st);
        self.updating.set(false);
        self.servers.update_marks(&st);
        self.test_btn
            .set_sensitive(st.online && !st.servers.is_empty() && !st.status.testing);
    }
}
