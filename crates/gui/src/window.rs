//! Main window.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use v2baloot_core::ipc::{Client, Event, NoticeLevel, Reply, Request};
use v2baloot_core::util::{format_date, human_bytes, human_speed};
use v2baloot_core::{link, ConnState, Latency, Mode, Server, Status, Subscription};

use crate::{call, dialogs, rt};

#[derive(Clone, PartialEq, Eq)]
pub enum Group {
    All,
    Sub(String),
    Manual,
}

impl Group {
    fn key(&self) -> String {
        match self {
            Group::All => "all".into(),
            Group::Manual => "manual".into(),
            Group::Sub(id) => format!("sub:{id}"),
        }
    }
    fn from_key(k: &str) -> Group {
        match k {
            "manual" => Group::Manual,
            k if k.starts_with("sub:") => Group::Sub(k[4..].to_string()),
            _ => Group::All,
        }
    }
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
    content_page: adw::NavigationPage,
    sidebar: gtk::ListBox,
    list: gtk::ListBox,
    stack: gtk::Stack,
    banner: adw::Banner,
    connect_switch: gtk::Switch,
    proxy_btn: gtk::ToggleButton,
    tun_btn: gtk::ToggleButton,
    test_btn: gtk::Button,
    search_bar: gtk::SearchBar,
    status_dot: gtk::Label,
    status_label: gtk::Label,
    speed_label: gtk::Label,
    latency_labels: RefCell<HashMap<String, gtk::Label>>,
    /// Check-mark icon per server row, so status changes don't rebuild the list.
    check_icons: RefCell<HashMap<String, gtk::Image>>,
    /// Bumped per refresh; results from older refreshes are dropped.
    refresh_gen: Cell<u64>,
    pub state: RefCell<State>,
    /// Set while we update widgets from daemon state so their signal handlers don't fire requests.
    updating: Cell<bool>,
    /// (state, server) the list was last rendered for, to know when the check mark moves.
    rendered_for: RefCell<(ConnState, Option<String>)>,
}

enum Msg {
    Online(bool),
    Event(Event),
}

pub fn build(app: &adw::Application) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("v2baloot")
        .default_width(980)
        .default_height(640)
        .width_request(360)
        .height_request(400)
        .build();

    // ---------------- sidebar
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();
    let sidebar_header = adw::HeaderBar::new();
    let menu = gio::Menu::new();
    let section1 = gio::Menu::new();
    section1.append(Some("Import from Clipboard"), Some("win.import"));
    section1.append(Some("Add Subscription…"), Some("win.add-sub"));
    section1.append(Some("Update Subscriptions"), Some("win.update-subs"));
    menu.append_section(None, &section1);
    let section2 = gio::Menu::new();
    section2.append(Some("Sort by Latency"), Some("win.sort-latency"));
    section2.append(Some("Xray Log"), Some("win.logs"));
    menu.append_section(None, &section2);
    let section3 = gio::Menu::new();
    section3.append(Some("Preferences"), Some("win.prefs"));
    section3.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    section3.append(Some("About v2baloot"), Some("win.about"));
    menu.append_section(None, &section3);
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .primary(true)
        .tooltip_text("Main Menu")
        .build();
    sidebar_header.pack_end(&menu_btn);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .child(&sidebar)
            .vexpand(true)
            .build(),
    ));
    let sidebar_page = adw::NavigationPage::builder()
        .title("v2baloot")
        .child(&sidebar_view)
        .build();

    // ---------------- content header
    let header = adw::HeaderBar::new();
    let proxy_btn = gtk::ToggleButton::builder()
        .label("Proxy")
        .tooltip_text("SOCKS/HTTP proxy + system proxy settings")
        .build();
    let tun_btn = gtk::ToggleButton::builder()
        .label("TUN")
        .tooltip_text("Route all traffic through a virtual interface")
        .group(&proxy_btn)
        .build();
    let mode_box = gtk::Box::builder().css_classes(["linked"]).build();
    mode_box.append(&proxy_btn);
    mode_box.append(&tun_btn);
    header.set_title_widget(Some(&mode_box));

    let search_btn = gtk::ToggleButton::builder()
        .icon_name("system-search-symbolic")
        .tooltip_text("Search (Ctrl+F)")
        .build();
    let test_btn = gtk::Button::builder()
        .label("Test")
        .tooltip_text("Test latency of the shown servers (Ctrl+T)")
        .action_name("win.test-all")
        .build();
    header.pack_start(&search_btn);
    header.pack_start(&test_btn);
    let connect_switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .tooltip_text("Connect (Ctrl+Enter)")
        .build();
    header.pack_end(&connect_switch);

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search servers")
        .hexpand(true)
        .build();
    let search_bar = gtk::SearchBar::builder()
        .child(
            &adw::Clamp::builder()
                .maximum_size(500)
                .child(&search)
                .build(),
        )
        .build();
    search_bar.connect_entry(&search);
    search_btn
        .bind_property("active", &search_bar, "search-mode-enabled")
        .bidirectional()
        .build();

    let banner = adw::Banner::builder().button_label("Dismiss").build();
    banner.connect_button_clicked(|b| b.set_revealed(false));

    // ---------------- content body
    let list = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .build();
    let list_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(
            &adw::Clamp::builder()
                .maximum_size(820)
                .margin_top(12)
                .margin_bottom(18)
                .margin_start(12)
                .margin_end(12)
                .child(&list)
                .build(),
        )
        .build();

    let empty_actions = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    empty_actions.append(
        &gtk::Button::builder()
            .label("Import from Clipboard")
            .action_name("win.import")
            .css_classes(["pill", "suggested-action"])
            .build(),
    );
    empty_actions.append(
        &gtk::Button::builder()
            .label("Add Subscription")
            .action_name("win.add-sub")
            .css_classes(["pill"])
            .build(),
    );
    let empty = adw::StatusPage::builder()
        .icon_name("network-vpn-symbolic")
        .title("No Servers Yet")
        .description("Copy a vless://, vmess://, trojan:// or ss:// link and import it, or add your provider's subscription URL.")
        .child(&empty_actions)
        .build();
    let offline = adw::StatusPage::builder()
        .icon_name("network-offline-symbolic")
        .title("Daemon Not Running")
        .description("Start it with\n<tt>systemctl --user enable --now v2baloot</tt>\nThis window reconnects automatically.")
        .build();
    let nomatch = adw::StatusPage::builder()
        .icon_name("system-search-symbolic")
        .title("No Matching Servers")
        .build();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&list_scroller, Some("list"));
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&offline, Some("offline"));
    stack.add_named(&nomatch, Some("nomatch"));

    // ---------------- status strip
    let status_dot = gtk::Label::builder()
        .label("●")
        .css_classes(["status-dot"])
        .build();
    let status_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .hexpand(true)
        .build();
    let speed_label = gtk::Label::builder()
        .css_classes(["dim-label", "numeric"])
        .build();
    let strip = gtk::Box::builder()
        .spacing(8)
        .css_classes(["status-strip"])
        .build();
    strip.append(&status_dot);
    strip.append(&status_label);
    strip.append(&speed_label);

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&header);
    content_view.add_top_bar(&search_bar);
    content_view.add_top_bar(&banner);
    content_view.set_content(Some(&stack));
    content_view.add_bottom_bar(&strip);
    let content_page = adw::NavigationPage::builder()
        .title("Servers")
        .child(&content_view)
        .build();

    let split = adw::NavigationSplitView::builder()
        .sidebar(&sidebar_page)
        .content(&content_page)
        .min_sidebar_width(220.0)
        .max_sidebar_width(280.0)
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    // Collapse to a single pane on narrow windows.
    let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        600.0,
        adw::LengthUnit::Sp,
    ));
    bp.add_setter(&split, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(bp);

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts,
        split,
        content_page,
        sidebar,
        list,
        stack,
        banner,
        connect_switch,
        proxy_btn,
        tun_btn,
        test_btn,
        search_bar,
        status_dot,
        status_label,
        speed_label,
        latency_labels: RefCell::default(),
        check_icons: RefCell::default(),
        refresh_gen: Cell::new(0),
        state: RefCell::default(),
        updating: Cell::new(false),
        rendered_for: RefCell::new((ConnState::Disconnected, None)),
    });

    wire_signals(&ui, &search);
    install_actions(&ui);
    ui.render_all();
    listen(&ui);
    window
}

fn wire_signals(ui: &Rc<Ui>, search: &gtk::SearchEntry) {
    let u = Rc::clone(ui);
    ui.connect_switch.connect_active_notify(move |sw| {
        if u.updating.get() {
            return;
        }
        let req = if sw.is_active() {
            Request::Connect { id: None }
        } else {
            Request::Disconnect
        };
        u.request(req);
    });
    for (btn, mode) in [
        (ui.proxy_btn.clone(), Mode::Proxy),
        (ui.tun_btn.clone(), Mode::Tun),
    ] {
        let u = Rc::clone(ui);
        btn.connect_toggled(move |b| {
            if u.updating.get() || !b.is_active() {
                return;
            }
            u.request(Request::SetMode { mode });
        });
    }
    let u = Rc::clone(ui);
    search.connect_search_changed(move |e| {
        u.state.borrow_mut().filter = e.text().to_string();
        u.render_list();
    });
    let u = Rc::clone(ui);
    ui.sidebar.connect_row_selected(move |_, row| {
        if u.updating.get() {
            return;
        }
        let Some(row) = row else { return };
        let group = Group::from_key(&row.widget_name());
        u.state.borrow_mut().group = Some(group);
        u.render_list();
        u.split.set_show_content(true);
    });
    // `row-selected` doesn't fire for the already-selected row; in collapsed (narrow) mode the
    // user still needs a way back to the list.
    let u = Rc::clone(ui);
    ui.sidebar
        .connect_row_activated(move |_, _| u.split.set_show_content(true));
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

fn install_actions(ui: &Rc<Ui>) {
    add_action(ui, "import", dialogs::import);
    add_action(ui, "add-sub", dialogs::add_subscription);
    add_action(ui, "update-subs", |u| {
        u.toast("Updating subscriptions…");
        u.request(Request::UpdateSubscriptions { id: None });
    });
    add_action(ui, "test-all", |u| {
        let ids: Vec<String> = u.visible().iter().map(|s| s.id.clone()).collect();
        if !ids.is_empty() {
            u.request(Request::TestLatency { ids: Some(ids) });
        }
    });
    add_action(ui, "search", |u| {
        u.search_bar.set_search_mode(!u.search_bar.is_search_mode())
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
        u.render_list();
    });
    ui.window.add_action(&sort);

    add_id_action(ui, "connect-server", |u, id| {
        u.request(Request::Connect { id: Some(id) })
    });
    add_id_action(ui, "test-server", |u, id| {
        u.request(Request::TestLatency {
            ids: Some(vec![id]),
        })
    });
    add_id_action(ui, "copy-link", |u, id| {
        let server = u
            .state
            .borrow()
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned();
        if let Some(s) = server {
            u.window.clipboard().set_text(&link::to_link(&s));
            u.toast("Share link copied");
        }
    });
    add_id_action(ui, "details", |u, id| {
        let server = u
            .state
            .borrow()
            .servers
            .iter()
            .find(|s| s.id == id)
            .cloned();
        if let Some(s) = server {
            dialogs::details(u, &s);
        }
    });
    add_id_action(ui, "delete-server", |u, id| {
        let name = u
            .state
            .borrow()
            .servers
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_default();
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
                    if tx.send(Msg::Event(ev)).await.is_err() {
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

fn latency_text(l: Option<&Latency>) -> (String, &'static str) {
    match l {
        None => ("—".into(), "dim-label"),
        Some(Latency::Failed { .. }) => ("timeout".into(), "error"),
        Some(Latency::Ok { ms }) if *ms < 300 => (format!("{ms} ms"), "success"),
        Some(Latency::Ok { ms }) if *ms < 800 => (format!("{ms} ms"), "warning"),
        Some(Latency::Ok { ms }) => (format!("{ms} ms"), "error"),
    }
}

fn set_check(img: &gtk::Image, connected: bool, selected: bool) {
    for c in ["success", "dim-label"] {
        img.remove_css_class(c);
    }
    if connected {
        img.set_icon_name(Some("object-select-symbolic"));
        img.add_css_class("success");
    } else if selected {
        // Last used server: same (always available) icon, dimmed.
        img.set_icon_name(Some("object-select-symbolic"));
        img.add_css_class("dim-label");
    } else {
        img.set_icon_name(None);
    }
}

fn set_latency_label(label: &gtk::Label, l: Option<&Latency>) {
    let (text, class) = latency_text(l);
    label.set_label(&text);
    for c in ["dim-label", "success", "warning", "error"] {
        label.remove_css_class(c);
    }
    label.add_css_class(class);
    label.set_tooltip_text(match l {
        Some(Latency::Failed { error }) if !error.is_empty() => Some(error.as_str()),
        _ => None,
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
                Err(e) => ui.error(&e),
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
                if let Some(Group::Sub(id)) = &st.group {
                    if !st.subs.iter().any(|s| &s.id == id) {
                        st.group = Some(Group::All);
                    }
                }
            }
            ui.render_sidebar();
            ui.render_list();
            // Switch/Test sensitivity depend on whether there are servers.
            ui.render_status();
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
            Msg::Event(ev) => match ev {
                Event::Status { status } => {
                    let prev_state = self.state.borrow().status.state.clone();
                    if let ConnState::Error { message } = &status.state {
                        if prev_state != status.state {
                            self.banner.set_title(&glib::markup_escape_text(message));
                            self.banner.set_revealed(true);
                        }
                    } else if status.is_connected() {
                        self.banner.set_revealed(false);
                    }
                    {
                        let mut st = self.state.borrow_mut();
                        if status.server_id.is_some() {
                            st.selected = status.server_id.clone();
                        }
                        st.status = status.clone();
                    }
                    self.render_status();
                    let key = (status.state.clone(), status.server_id.clone());
                    if *self.rendered_for.borrow() != key {
                        *self.rendered_for.borrow_mut() = key;
                        self.update_checks();
                    }
                }
                Event::ServersChanged => self.refresh(),
                Event::Latency { id, latency } => {
                    {
                        let mut st = self.state.borrow_mut();
                        if let Some(s) = st.servers.iter_mut().find(|s| s.id == id) {
                            s.latency = Some(latency.clone());
                        }
                    }
                    if let Some(label) = self.latency_labels.borrow().get(&id) {
                        set_latency_label(label, Some(&latency));
                    }
                }
                Event::SettingsChanged => {}
                Event::TestFinished => {
                    if self.state.borrow().sort_latency {
                        self.render_list();
                    }
                }
                Event::Notice { level, message } => match level {
                    NoticeLevel::Info => self.toast(&message),
                    NoticeLevel::Error => self.error(&message),
                },
            },
        }
    }

    pub fn visible(&self) -> Vec<Server> {
        let st = self.state.borrow();
        let f = st.filter.to_lowercase();
        let group = st.group.clone().unwrap_or(Group::All);
        let mut v: Vec<Server> = st
            .servers
            .iter()
            .filter(|s| match &group {
                Group::All => true,
                Group::Sub(id) => s.group.as_ref() == Some(id),
                Group::Manual => s.group.is_none(),
            })
            .filter(|s| {
                f.is_empty()
                    || s.name.to_lowercase().contains(&f)
                    || s.address.to_lowercase().contains(&f)
            })
            .cloned()
            .collect();
        if st.sort_latency {
            v.sort_by_key(|s| s.latency_ms().unwrap_or(u32::MAX));
        }
        v
    }

    fn render_all(self: &Rc<Self>) {
        self.render_sidebar();
        self.render_list();
        self.render_status();
    }

    fn sidebar_row(
        key: &str,
        icon: &str,
        title: &str,
        subtitle: Option<&str>,
        menu: Option<gio::Menu>,
    ) -> gtk::ListBoxRow {
        let b = gtk::Box::builder()
            .spacing(10)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        b.append(&gtk::Image::from_icon_name(icon));
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        text.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if let Some(s) = subtitle {
            text.append(
                &gtk::Label::builder()
                    .label(s)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["dim-label", "sidebar-subtitle"])
                    .build(),
            );
        }
        b.append(&text);
        if let Some(m) = menu {
            b.append(
                &gtk::MenuButton::builder()
                    .icon_name("view-more-symbolic")
                    .menu_model(&m)
                    .css_classes(["flat"])
                    .valign(gtk::Align::Center)
                    .build(),
            );
        }
        let row = gtk::ListBoxRow::builder().child(&b).build();
        row.set_widget_name(key);
        row
    }

    fn render_sidebar(self: &Rc<Self>) {
        self.updating.set(true);
        self.sidebar.remove_all();
        let st = self.state.borrow();
        let count = |pred: &dyn Fn(&Server) -> bool| st.servers.iter().filter(|s| pred(s)).count();
        let all = Self::sidebar_row(
            "all",
            "view-list-symbolic",
            "All Servers",
            Some(&format!("{} servers", st.servers.len())),
            None,
        );
        self.sidebar.append(&all);
        for sub in &st.subs {
            let n = count(&|s: &Server| s.group.as_ref() == Some(&sub.id));
            let mut sub_line = format!("{n} servers");
            if let Some(u) = &sub.usage {
                if u.total > 0 {
                    sub_line = format!(
                        "{} / {}",
                        human_bytes(u.upload + u.download),
                        human_bytes(u.total)
                    );
                }
                if u.expire > 0 {
                    sub_line.push_str(&format!(" · until {}", format_date(u.expire)));
                }
            }
            if sub.last_error.is_some() {
                sub_line = format!("⚠ {sub_line}");
            }
            let menu = gio::Menu::new();
            let item = gio::MenuItem::new(Some("Update"), None);
            item.set_action_and_target_value(Some("win.update-sub"), Some(&sub.id.to_variant()));
            menu.append_item(&item);
            let item = gio::MenuItem::new(Some("Delete"), None);
            item.set_action_and_target_value(Some("win.delete-sub"), Some(&sub.id.to_variant()));
            menu.append_item(&item);
            let row = Self::sidebar_row(
                &format!("sub:{}", sub.id),
                "folder-download-symbolic",
                &sub.name,
                Some(&sub_line),
                Some(menu),
            );
            if let Some(e) = &sub.last_error {
                row.set_tooltip_text(Some(e));
            }
            self.sidebar.append(&row);
        }
        let manual = count(&|s: &Server| s.group.is_none());
        if manual > 0 && !st.subs.is_empty() {
            self.sidebar.append(&Self::sidebar_row(
                "manual",
                "list-add-symbolic",
                "Added Manually",
                Some(&format!("{manual} servers")),
                None,
            ));
        }
        let want = st.group.clone().unwrap_or(Group::All).key();
        drop(st);
        let mut found = false;
        let mut i = 0;
        while let Some(row) = self.sidebar.row_at_index(i) {
            if row.widget_name() == want {
                self.sidebar.select_row(Some(&row));
                found = true;
                break;
            }
            i += 1;
        }
        if !found {
            // The group vanished (last manual server or subscription deleted): show everything.
            self.state.borrow_mut().group = Some(Group::All);
            self.sidebar
                .select_row(self.sidebar.row_at_index(0).as_ref());
        }
        self.updating.set(false);
    }

    fn server_row(
        &self,
        s: &Server,
        connected: bool,
        selected: bool,
        group_name: Option<&str>,
    ) -> adw::ActionRow {
        let mut subtitle = format!("{} · {}", s.kind_label(), s.address);
        if let Some(g) = group_name {
            subtitle.push_str(&format!(" · {g}"));
        }
        let row = adw::ActionRow::builder()
            .title(&s.name)
            .subtitle(&subtitle)
            .use_markup(false)
            .activatable(true)
            .title_lines(1)
            .subtitle_lines(1)
            .build();
        let check = gtk::Image::builder().pixel_size(16).build();
        set_check(&check, connected, selected);
        row.add_prefix(&check);
        self.check_icons.borrow_mut().insert(s.id.clone(), check);

        let lat = gtk::Label::builder()
            .css_classes(["latency", "numeric"])
            .xalign(1.0)
            .build();
        set_latency_label(&lat, s.latency.as_ref());
        row.add_suffix(&lat);
        self.latency_labels.borrow_mut().insert(s.id.clone(), lat);

        let menu = gio::Menu::new();
        for (label, action) in [
            ("Connect", "win.connect-server"),
            ("Test Latency", "win.test-server"),
            ("Copy Share Link", "win.copy-link"),
            ("Details", "win.details"),
        ] {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&s.id.to_variant()));
            menu.append_item(&item);
        }
        let danger = gio::Menu::new();
        let item = gio::MenuItem::new(Some("Delete"), None);
        item.set_action_and_target_value(Some("win.delete-server"), Some(&s.id.to_variant()));
        danger.append_item(&item);
        menu.append_section(None, &danger);
        row.add_suffix(
            &gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .menu_model(&menu)
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .tooltip_text("More")
                .build(),
        );

        let id = s.id.clone();
        row.connect_activated(move |r| {
            let _ = r.activate_action("win.connect-server", Some(&id.to_variant()));
        });
        row
    }

    fn render_list(self: &Rc<Self>) {
        let visible = self.visible();
        let st = self.state.borrow();
        *self.rendered_for.borrow_mut() = (st.status.state.clone(), st.status.server_id.clone());
        self.list.remove_all();
        self.latency_labels.borrow_mut().clear();
        self.check_icons.borrow_mut().clear();
        let page = if !st.online {
            "offline"
        } else if st.servers.is_empty() {
            "empty"
        } else if visible.is_empty() {
            "nomatch"
        } else {
            "list"
        };
        self.stack.set_visible_child_name(page);
        let title = match st.group.clone().unwrap_or(Group::All) {
            Group::All => "All Servers".to_string(),
            Group::Manual => "Added Manually".to_string(),
            Group::Sub(id) => st
                .subs
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
        };
        self.content_page.set_title(&title);

        let active_id = st
            .status
            .is_active()
            .then(|| st.status.server_id.clone())
            .flatten();
        let show_group = matches!(st.group, None | Some(Group::All)) && !st.subs.is_empty();
        for s in &visible {
            let group_name = if show_group {
                s.group
                    .as_ref()
                    .and_then(|g| st.subs.iter().find(|x| &x.id == g))
                    .map(|x| x.name.as_str())
            } else {
                None
            };
            let row = self.server_row(
                s,
                active_id.as_ref() == Some(&s.id),
                st.selected.as_ref() == Some(&s.id),
                group_name,
            );
            self.list.append(&row);
        }
    }

    /// Move the check marks after a connection change without rebuilding rows (keeps focus and
    /// any open row menu).
    fn update_checks(&self) {
        let st = self.state.borrow();
        let active_id = st
            .status
            .is_active()
            .then(|| st.status.server_id.clone())
            .flatten();
        for (id, img) in self.check_icons.borrow().iter() {
            set_check(
                img,
                active_id.as_ref() == Some(id),
                st.selected.as_ref() == Some(id),
            );
        }
    }

    fn render_status(&self) {
        let st = self.state.borrow();
        self.updating.set(true);
        let s = &st.status;
        self.connect_switch
            .set_sensitive(st.online && !st.servers.is_empty());
        self.connect_switch.set_active(s.is_active());
        self.proxy_btn.set_sensitive(st.online);
        self.tun_btn.set_sensitive(st.online);
        self.proxy_btn.set_active(s.mode == Mode::Proxy);
        self.tun_btn.set_active(s.mode == Mode::Tun);
        self.test_btn
            .set_sensitive(st.online && !st.servers.is_empty() && !s.testing);
        self.updating.set(false);

        for c in ["connected", "connecting", "error"] {
            self.status_dot.remove_css_class(c);
        }
        let name = s.server_name.clone().unwrap_or_default();
        let (class, text) = if !st.online {
            (None, "Daemon offline".to_string())
        } else {
            match &s.state {
                ConnState::Connected => (
                    Some("connected"),
                    format!("Connected to {name} · {}", s.mode.label()),
                ),
                ConnState::Connecting => (Some("connecting"), format!("Connecting to {name}…")),
                ConnState::Disconnected => (None, "Disconnected".to_string()),
                ConnState::Error { message } => (
                    Some("error"),
                    message.lines().next().unwrap_or("").to_string(),
                ),
            }
        };
        if let Some(c) = class {
            self.status_dot.add_css_class(c);
        } else {
            self.status_dot.add_css_class("dim-label");
        }
        if class.is_some() {
            self.status_dot.remove_css_class("dim-label");
        }
        let text = if s.testing {
            format!("{text} · testing latency…")
        } else {
            text
        };
        self.status_label.set_label(&text);
        self.status_label.set_tooltip_text(Some(&text));
        if st.online && s.is_connected() {
            let mut speed = format!(
                "↑ {}   ↓ {}",
                human_speed(s.up_speed),
                human_speed(s.down_speed)
            );
            if s.mode == Mode::Proxy {
                speed.push_str(&format!(
                    "   ·   SOCKS {}  HTTP {}",
                    s.socks_port, s.http_port
                ));
            }
            self.speed_label.set_label(&speed);
        } else {
            self.speed_label.set_label("");
        }
    }
}
