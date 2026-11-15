//! Right pane: group pills, search, and the server list.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use v2baloot_core::{Latency, Server};

use crate::dashboard::{latency_text, server_flag, server_title};
use crate::window::{Group, State};

type GroupCallback = Rc<dyn Fn(Group)>;

struct Row {
    row: gtk::ListBoxRow,
    check: gtk::Image,
    latency: gtk::Label,
    bars: [gtk::Box; 3],
}

pub struct ServersPane {
    pub root: gtk::Box,
    pub search_bar: gtk::SearchBar,
    pub search: gtk::SearchEntry,
    pub list: gtk::ListBox,
    pills: gtk::Box,
    stack: gtk::Stack,
    progress: gtk::ProgressBar,
    progress_rev: gtk::Revealer,
    rows: RefCell<HashMap<String, Row>>,
    on_group: RefCell<Option<GroupCallback>>,
    test_total: Cell<usize>,
    test_done: Cell<usize>,
}

fn status_page(icon: &str, title: &str, desc: &str) -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name(icon)
        .title(title)
        .description(desc)
        .build()
}

fn bars_for(l: Option<&Latency>) -> (usize, &'static str) {
    match l {
        Some(Latency::Ok { ms }) if *ms < 300 => (3, "good"),
        Some(Latency::Ok { ms }) if *ms < 800 => (2, "ok"),
        Some(Latency::Ok { .. }) => (1, "bad"),
        _ => (0, ""),
    }
}

fn apply_latency(r: &Row, l: Option<&Latency>) {
    let (text, class) = latency_text(l);
    r.latency.set_label(&text);
    for c in ["good", "ok", "bad", "none"] {
        r.latency.remove_css_class(c);
    }
    r.latency.add_css_class(class);
    r.latency.set_tooltip_text(match l {
        Some(Latency::Failed { error }) if !error.is_empty() => Some(error.as_str()),
        _ => None,
    });
    let (on, quality) = bars_for(l);
    for (i, b) in r.bars.iter().enumerate() {
        for c in ["on", "good", "ok", "bad"] {
            b.remove_css_class(c);
        }
        if i < on {
            b.add_css_class("on");
            b.add_css_class(quality);
        }
    }
}

fn menu_for(id: &str) -> gio::Menu {
    let menu = gio::Menu::new();
    for (label, action) in [
        ("Connect", "win.connect-server"),
        ("Test Latency", "win.test-server"),
        ("Copy Share Link", "win.copy-link"),
        ("Details", "win.details"),
    ] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some(action), Some(&id.to_variant()));
        menu.append_item(&item);
    }
    let danger = gio::Menu::new();
    let item = gio::MenuItem::new(Some("Delete"), None);
    item.set_action_and_target_value(Some("win.delete-server"), Some(&id.to_variant()));
    danger.append_item(&item);
    menu.append_section(None, &danger);
    menu
}

fn build_row(s: &Server, group_name: Option<&str>) -> Row {
    let b = gtk::Box::builder()
        .spacing(12)
        .css_classes(["server-row"])
        .build();
    b.append(
        &gtk::Label::builder()
            .label(server_flag(s))
            .css_classes(["row-flag"])
            .build(),
    );

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(server_title(s))
            .css_classes(["row-name"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let mut kind = s.kind_label();
    if let Some(g) = group_name {
        kind.push_str(&format!(" · {g}"));
    }
    text.append(
        &gtk::Label::builder()
            .label(kind)
            .css_classes(["row-kind"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    b.append(&text);

    let check = gtk::Image::builder()
        .icon_name("object-select-symbolic")
        .css_classes(["check-mark"])
        .visible(false)
        .valign(gtk::Align::Center)
        .build();
    b.append(&check);

    let bars_box = gtk::Box::builder()
        .spacing(2)
        .valign(gtk::Align::Center)
        .height_request(14)
        .build();
    let bars = [5, 9, 13].map(|h| {
        let bar = gtk::Box::builder()
            .css_classes(["sig-bar"])
            .height_request(h)
            .valign(gtk::Align::End)
            .build();
        bars_box.append(&bar);
        bar
    });
    b.append(&bars_box);

    let latency = gtk::Label::builder()
        .css_classes(["latency-ms"])
        .xalign(1.0)
        .build();
    b.append(&latency);

    b.append(
        &gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu_for(&s.id))
            .css_classes(["flat", "circular", "row-menu"])
            .valign(gtk::Align::Center)
            .tooltip_text("More")
            .build(),
    );

    let row = gtk::ListBoxRow::builder()
        .child(&b)
        .activatable(true)
        .build();
    row.set_widget_name(&s.id);
    let r = Row {
        row,
        check,
        latency,
        bars,
    };
    apply_latency(&r, s.latency.as_ref());
    r
}

impl ServersPane {
    pub fn new() -> ServersPane {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search servers")
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(
                &adw::Clamp::builder()
                    .maximum_size(520)
                    .child(&search)
                    .build(),
            )
            .build();
        search_bar.connect_entry(&search);
        root.append(&search_bar);

        let pills = gtk::Box::builder()
            .spacing(6)
            .css_classes(["pills"])
            .build();
        root.append(
            &gtk::ScrolledWindow::builder()
                .vscrollbar_policy(gtk::PolicyType::Never)
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .child(&pills)
                .build(),
        );

        let progress = gtk::ProgressBar::builder()
            .css_classes(["test-progress"])
            .margin_start(16)
            .margin_end(16)
            .build();
        let progress_rev = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&progress)
            .build();
        root.append(&progress_rev);

        let list = gtk::ListBox::builder()
            .css_classes(["server-list"])
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .valign(gtk::Align::Start)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&adw::Clamp::builder().maximum_size(860).child(&list).build())
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
        let empty = status_page(
            crate::APP_ID,
            "No Servers Yet",
            "Copy a vless://, vmess://, trojan:// or ss:// link and import it, or add your provider's subscription URL.",
        );
        empty.set_child(Some(&empty_actions));
        let offline = status_page(
            "network-offline-symbolic",
            "Daemon Not Running",
            "Start it with  systemctl --user enable --now v2baloot\nThis window reconnects automatically.",
        );
        let nomatch = status_page(
            "system-search-symbolic",
            "No Matching Servers",
            "Try another search or group.",
        );

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(180)
            .vexpand(true)
            .build();
        stack.add_named(&scroller, Some("list"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&offline, Some("offline"));
        stack.add_named(&nomatch, Some("nomatch"));
        root.append(&stack);

        ServersPane {
            root,
            search_bar,
            search,
            list,
            pills,
            stack,
            progress,
            progress_rev,
            rows: RefCell::default(),
            on_group: RefCell::default(),
            test_total: Cell::new(0),
            test_done: Cell::new(0),
        }
    }

    pub fn connect_group_changed(&self, f: impl Fn(Group) + 'static) {
        *self.on_group.borrow_mut() = Some(Rc::new(f));
    }

    pub fn visible(st: &State) -> Vec<Server> {
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

    fn render_pills(&self, st: &State) {
        while let Some(c) = self.pills.first_child() {
            self.pills.remove(&c);
        }
        let current = st.group.clone().unwrap_or(Group::All);
        let mut groups = vec![(Group::All, format!("All  {}", st.servers.len()))];
        for sub in &st.subs {
            let n = st
                .servers
                .iter()
                .filter(|s| s.group.as_ref() == Some(&sub.id))
                .count();
            groups.push((Group::Sub(sub.id.clone()), format!("{}  {n}", sub.name)));
        }
        let manual = st.servers.iter().filter(|s| s.group.is_none()).count();
        if manual > 0 && !st.subs.is_empty() {
            groups.push((Group::Manual, format!("Manual  {manual}")));
        }
        let mut first: Option<gtk::ToggleButton> = None;
        for (g, text) in groups {
            let b = gtk::ToggleButton::builder()
                .label(&text)
                .css_classes(["pill-toggle", "flat"])
                .active(g == current)
                .build();
            if let Some(f) = &first {
                b.set_group(Some(f));
            } else {
                first = Some(b.clone());
            }
            if let Some(cb) = self.on_group.borrow().clone() {
                b.connect_toggled(move |b| {
                    if b.is_active() {
                        cb(g.clone());
                    }
                });
            }
            self.pills.append(&b);
        }
        // Only one group: the pills add nothing.
        self.pills.set_visible(
            self.pills
                .first_child()
                .and_then(|c| c.next_sibling())
                .is_some(),
        );
    }

    /// Rebuild the list (data, group, search or sort changed).
    pub fn render(&self, st: &State) {
        self.render_pills(st);
        let visible = Self::visible(st);
        self.list.remove_all();
        let mut rows = self.rows.borrow_mut();
        rows.clear();
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
        let show_group = matches!(st.group, None | Some(Group::All)) && st.subs.len() > 1;
        for s in &visible {
            let group_name = show_group
                .then(|| {
                    s.group
                        .as_ref()
                        .and_then(|g| st.subs.iter().find(|x| &x.id == g))
                        .map(|x| x.name.as_str())
                })
                .flatten();
            let r = build_row(s, group_name);
            self.list.append(&r.row);
            rows.insert(s.id.clone(), r);
        }
        drop(rows);
        self.update_marks(st);
    }

    /// Move the "active" highlight / check mark without rebuilding rows.
    pub fn update_marks(&self, st: &State) {
        let active = st
            .status
            .is_active()
            .then(|| st.status.server_id.clone())
            .flatten();
        for (id, r) in self.rows.borrow().iter() {
            let on = active.as_ref() == Some(id);
            if on {
                r.row.add_css_class("active");
            } else {
                r.row.remove_css_class("active");
            }
            r.check.set_visible(on && st.status.is_connected());
        }
    }

    pub fn set_latency(&self, id: &str, l: &Latency) {
        if let Some(r) = self.rows.borrow().get(id) {
            apply_latency(r, Some(l));
        }
        if self.test_total.get() > 0 {
            self.test_done.set(self.test_done.get() + 1);
            self.progress
                .set_fraction(self.test_done.get() as f64 / self.test_total.get() as f64);
        }
    }

    pub fn begin_test(&self, total: usize) {
        self.test_total.set(total);
        self.test_done.set(0);
        self.progress.set_fraction(0.0);
        self.progress_rev.set_reveal_child(total > 0);
    }

    pub fn end_test(&self) {
        self.test_total.set(0);
        self.progress_rev.set_reveal_child(false);
    }

    /// Scroll the list to a server and focus it.
    pub fn reveal(&self, id: &str) {
        if let Some(r) = self.rows.borrow().get(id) {
            r.row.grab_focus();
        }
    }
}
