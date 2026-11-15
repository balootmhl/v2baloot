//! Dialogs: import, add subscription, details, preferences, logs, about.

use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use v2baloot_core::ipc::{Reply, Request};
use v2baloot_core::store::Settings;
use v2baloot_core::{link, Protocol, SecurityKind, Server};

use crate::window::Ui;
use crate::{call, APP_ID};

async fn clipboard_text(ui: &Ui) -> Option<String> {
    ui.window
        .clipboard()
        .read_text_future()
        .await
        .ok()
        .flatten()
        .map(|s| s.to_string())
}

pub fn confirm(ui: &Rc<Ui>, heading: &str, action: &str, on_yes: impl Fn(&Rc<Ui>) + 'static) {
    let d = adw::AlertDialog::new(Some(heading), None);
    d.add_responses(&[("cancel", "_Cancel"), ("yes", action)]);
    d.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
    d.set_close_response("cancel");
    let u = Rc::clone(ui);
    d.connect_response(None, move |_, r| {
        if r == "yes" {
            on_yes(&u);
        }
    });
    d.present(Some(&ui.window));
}

pub fn import(ui: &Rc<Ui>) {
    let ui = Rc::clone(ui);
    glib::spawn_future_local(async move {
        let clip = clipboard_text(&ui).await.unwrap_or_default();
        let lines: Vec<&str> = clip
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        const SCHEMES: [&str; 4] = ["vless://", "vmess://", "trojan://", "ss://"];
        // One-step import when the clipboard holds nothing but share links.
        if !lines.is_empty()
            && lines.iter().all(|l| {
                SCHEMES
                    .iter()
                    .any(|s| l.to_ascii_lowercase().starts_with(s))
            })
        {
            ui.request(Request::Import { text: clip });
            return;
        }
        // A single http(s) URL is a subscription, not a server.
        if lines.len() == 1 && (lines[0].starts_with("https://") || lines[0].starts_with("http://"))
        {
            add_subscription(&ui);
            return;
        }
        let d = adw::AlertDialog::new(
            Some("Import Servers"),
            Some("Paste one or more vless://, vmess://, trojan:// or ss:// links."),
        );
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::Char)
            .monospace(true)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .min_content_height(140)
            .min_content_width(360)
            .child(&view)
            .css_classes(["card"])
            .build();
        d.set_extra_child(Some(&scroller));
        d.add_responses(&[("cancel", "_Cancel"), ("import", "_Import")]);
        d.set_response_appearance("import", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("import"));
        d.set_close_response("cancel");
        let u = Rc::clone(&ui);
        d.connect_response(None, move |_, r| {
            if r != "import" {
                return;
            }
            let buf = view.buffer();
            let text = buf
                .text(&buf.start_iter(), &buf.end_iter(), false)
                .to_string();
            if !text.trim().is_empty() {
                u.request(Request::Import { text });
            }
        });
        d.present(Some(&ui.window));
    });
}

pub fn add_subscription(ui: &Rc<Ui>) {
    let ui = Rc::clone(ui);
    glib::spawn_future_local(async move {
        let clip = clipboard_text(&ui).await.unwrap_or_default();
        let d = adw::AlertDialog::new(
            Some("Add Subscription"),
            Some("Paste the subscription URL from your provider."),
        );
        let group = adw::PreferencesGroup::new();
        let url = adw::EntryRow::builder().title("URL").build();
        let clip = clip.trim();
        if (clip.starts_with("https://") || clip.starts_with("http://"))
            && !clip.contains(char::is_whitespace)
        {
            url.set_text(clip);
        }
        let name = adw::EntryRow::builder().title("Name (optional)").build();
        group.add(&url);
        group.add(&name);
        d.set_extra_child(Some(&group));
        d.add_responses(&[("cancel", "_Cancel"), ("add", "_Add")]);
        d.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("add"));
        d.set_close_response("cancel");
        url.set_activates_default(true);
        let u = Rc::clone(&ui);
        d.connect_response(None, move |_, r| {
            if r != "add" || url.text().trim().is_empty() {
                return;
            }
            u.toast("Fetching subscription…");
            u.request(Request::AddSubscription {
                name: name.text().to_string(),
                url: url.text().to_string(),
            });
        });
        d.present(Some(&ui.window));
    });
}

fn prop_row(title: &str, value: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .use_markup(false)
        .subtitle_selectable(true)
        .css_classes(["property"])
        .build()
}

pub fn details(ui: &Rc<Ui>, s: &Server) {
    let page = adw::PreferencesPage::new();

    let conn = adw::PreferencesGroup::builder().title("Connection").build();
    conn.add(&prop_row("Protocol", &s.protocol.name().to_uppercase()));
    conn.add(&prop_row("Address", &s.address));
    conn.add(&prop_row("Port", &s.port.to_string()));
    match &s.protocol {
        Protocol::Vless { flow: Some(f), .. } => conn.add(&prop_row("Flow", f)),
        Protocol::Vmess { cipher, .. } => conn.add(&prop_row("Cipher", cipher)),
        Protocol::Shadowsocks { method, .. } => conn.add(&prop_row("Method", method)),
        _ => {}
    }
    page.add(&conn);

    let t = &s.transport;
    let tr = adw::PreferencesGroup::builder().title("Transport").build();
    tr.add(&prop_row("Network", t.network.name()));
    for (k, v) in [
        ("Path", &t.path),
        ("Host", &t.host),
        ("Service Name", &t.service_name),
        ("Mode", &t.mode),
        ("Header", &t.header_type),
    ] {
        if let Some(v) = v {
            tr.add(&prop_row(k, v));
        }
    }
    page.add(&tr);

    let sec = &s.security;
    let sg = adw::PreferencesGroup::builder().title("Security").build();
    sg.add(&prop_row("Type", sec.kind.name()));
    if sec.kind != SecurityKind::None {
        for (k, v) in [
            ("SNI", &sec.sni),
            ("Fingerprint", &sec.fingerprint),
            ("Public Key", &sec.public_key),
            ("Short ID", &sec.short_id),
        ] {
            if let Some(v) = v {
                sg.add(&prop_row(k, v));
            }
        }
        if !sec.alpn.is_empty() {
            sg.add(&prop_row("ALPN", &sec.alpn.join(", ")));
        }
        if sec.allow_insecure {
            sg.add(&prop_row(
                "allowInsecure",
                "ignored — removed in Xray 26 (use pcs / vcn)",
            ));
        }
        if let Some(p) = &sec.pinned_sha256 {
            sg.add(&prop_row("Pinned cert SHA-256", p));
        }
        if let Some(n) = &sec.verify_name {
            sg.add(&prop_row("Verify cert by name", n));
        }
    }
    page.add(&sg);

    let share = adw::PreferencesGroup::new();
    let copy = adw::ButtonRow::builder()
        .title("Copy Share Link")
        .start_icon_name("edit-copy-symbolic")
        .build();
    let link_text = link::to_link(s);
    let u = Rc::clone(ui);
    copy.connect_activated(move |_| {
        u.window.clipboard().set_text(&link_text);
        u.toast("Share link copied");
    });
    share.add(&copy);
    page.add(&share);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    let d = adw::Dialog::builder()
        .title(&s.name)
        .content_width(460)
        .content_height(620)
        .child(&view)
        .build();
    d.present(Some(&ui.window));
}

pub fn logs(ui: &Rc<Ui>) {
    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&scroller));
    let d = adw::Dialog::builder()
        .title("Xray Log")
        .content_width(860)
        .content_height(520)
        .child(&tv)
        .build();
    d.present(Some(&ui.window));
    call(Request::Logs, move |res| {
        let text = match res {
            Ok(Reply::Logs { lines }) if lines.is_empty() => "No log lines yet.".to_string(),
            Ok(Reply::Logs { lines }) => lines.join("\n"),
            Ok(_) => String::new(),
            Err(e) => e,
        };
        let buf = view.buffer();
        buf.set_text(&text);
        let mut end = buf.end_iter();
        view.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);
    });
}

pub fn about(ui: &Rc<Ui>) {
    let d = adw::AboutDialog::builder()
        .application_name("v2baloot")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Moe Hein Latt")
        .website("https://github.com/balootmhl/v2baloot")
        .issue_url("https://github.com/balootmhl/v2baloot/issues")
        .comments("Xray client for Omarchy: GUI, tray, TUI, Waybar and Walker in one.")
        .license_type(gtk::License::MitX11)
        .build();
    d.present(Some(&ui.window));
}

pub fn shortcuts(ui: &Rc<Ui>) {
    let group = adw::PreferencesGroup::new();
    for (accel, what) in [
        ("<Ctrl>Return", "Connect / disconnect"),
        ("<Ctrl>i", "Import from clipboard"),
        ("<Ctrl>n", "Add subscription"),
        ("<Ctrl>r", "Update subscriptions"),
        ("<Ctrl>t", "Test latency"),
        ("<Ctrl>f", "Search"),
        ("<Ctrl>comma", "Preferences"),
        ("<Ctrl>w", "Close window (stays connected)"),
    ] {
        let row = adw::ActionRow::builder().title(what).build();
        row.add_suffix(
            &gtk::ShortcutLabel::builder()
                .accelerator(accel)
                .valign(gtk::Align::Center)
                .build(),
        );
        group.add(&row);
    }
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let tv = adw::ToolbarView::new();
    tv.add_top_bar(&adw::HeaderBar::new());
    tv.set_content(Some(&page));
    let d = adw::Dialog::builder()
        .title("Keyboard Shortcuts")
        .content_width(420)
        .content_height(480)
        .child(&tv)
        .build();
    d.present(Some(&ui.window));
}

fn spin(title: &str, subtitle: &str, min: f64, max: f64, value: f64) -> adw::SpinRow {
    let r = adw::SpinRow::with_range(min, max, 1.0);
    r.set_title(title);
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_value(value);
    r
}

fn switch(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    let r = adw::SwitchRow::builder()
        .title(title)
        .active(active)
        .build();
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r
}

fn entry(title: &str, text: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).text(text).build()
}

const LOG_LEVELS: [&str; 5] = ["debug", "info", "warning", "error", "none"];

/// Copy every field that differs between `opened` and `edited` into `target`.
fn merge_changed(target: &mut Settings, opened: &Settings, edited: &Settings) {
    macro_rules! merge {
        ($($f:ident),* $(,)?) => {
            // Exhaustive destructuring: adding a Settings field without listing it is a compile error.
            let Settings { $($f: _),* } = edited;
            $(if edited.$f != opened.$f { target.$f = edited.$f.clone(); })*
        };
    }
    merge!(
        xray_path,
        listen,
        socks_port,
        http_port,
        api_port,
        system_proxy,
        bypass_private,
        dns,
        tun_dns,
        log_level,
        autoconnect,
        sub_update_hours,
        sub_user_agent,
        test_url,
        test_timeout_ms,
        test_concurrency,
        tray,
        notifications,
        menu_command,
        waybar_show_name,
        conn_idle_secs,
        outbound_interface,
    );
}

pub fn preferences(ui: &Rc<Ui>) {
    let ui = Rc::clone(ui);
    call(Request::GetSettings, move |res| match res {
        Ok(Reply::Settings { settings }) => show_preferences(&ui, *settings),
        Ok(_) => {}
        Err(e) => ui.error(&e),
    });
}

fn show_preferences(ui: &Rc<Ui>, s: Settings) {
    let d = adw::PreferencesDialog::new();
    d.set_search_enabled(false);

    // General
    let general = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    let conn = adw::PreferencesGroup::builder().title("Connection").build();
    let system_proxy = switch(
        "Set system proxy",
        "In Proxy mode, point GTK apps and newly started apps at v2baloot",
        s.system_proxy,
    );
    let bypass = switch(
        "Bypass LAN",
        "Connect to private/local addresses directly",
        s.bypass_private,
    );
    let autoconnect = switch(
        "Connect on login",
        "Reconnect to the last server when the daemon starts",
        s.autoconnect,
    );
    conn.add(&system_proxy);
    conn.add(&bypass);
    conn.add(&autoconnect);
    general.add(&conn);

    let subs = adw::PreferencesGroup::builder()
        .title("Subscriptions")
        .build();
    let sub_hours = spin(
        "Auto-update every (hours)",
        "0 turns automatic updates off",
        0.0,
        168.0,
        s.sub_update_hours as f64,
    );
    let ua = entry("User-Agent", &s.sub_user_agent);
    subs.add(&sub_hours);
    subs.add(&ua);
    general.add(&subs);

    let desktop = adw::PreferencesGroup::builder().title("Desktop").build();
    let tray = switch(
        "Tray icon",
        "Takes effect after restarting the daemon",
        s.tray,
    );
    let notifications = switch(
        "Notifications",
        "Show connect / disconnect notifications",
        s.notifications,
    );
    let waybar_name = switch(
        "Server name in Waybar",
        "Show the server name next to the Waybar icon",
        s.waybar_show_name,
    );
    desktop.add(&tray);
    desktop.add(&notifications);
    desktop.add(&waybar_name);
    general.add(&desktop);
    d.add(&general);

    // Network
    let net = adw::PreferencesPage::builder()
        .title("Network")
        .icon_name("network-wired-symbolic")
        .build();
    let ports = adw::PreferencesGroup::builder()
        .title("Local Ports")
        .description("Changing these reconnects.")
        .build();
    let socks = spin("SOCKS5 port", "", 1.0, 65535.0, s.socks_port as f64);
    let http = spin("HTTP port", "", 1.0, 65535.0, s.http_port as f64);
    let api = spin("Stats API port", "", 1.0, 65535.0, s.api_port as f64);
    let allow_lan = switch(
        "Allow LAN connections",
        "Listen on all interfaces so other devices can use this proxy",
        s.listen == "0.0.0.0",
    );
    ports.add(&socks);
    ports.add(&http);
    ports.add(&api);
    ports.add(&allow_lan);
    net.add(&ports);

    let dns_group = adw::PreferencesGroup::builder().title("DNS").build();
    let dns = entry("DNS servers (comma separated)", &s.dns.join(", "));
    let tun_dns = entry("TUN interface DNS", &s.tun_dns);
    dns_group.add(&dns);
    dns_group.add(&tun_dns);
    net.add(&dns_group);

    let test = adw::PreferencesGroup::builder()
        .title("Latency Test")
        .build();
    let test_url = entry("Test URL", &s.test_url);
    let timeout = spin("Timeout (ms)", "", 500.0, 30000.0, s.test_timeout_ms as f64);
    let conc = spin("Parallel tests", "", 1.0, 64.0, s.test_concurrency as f64);
    test.add(&test_url);
    test.add(&timeout);
    test.add(&conc);
    net.add(&test);

    let adv = adw::PreferencesGroup::builder().title("Advanced").build();
    let xray = entry("Xray binary", &s.xray_path);
    let level_idx = LOG_LEVELS
        .iter()
        .position(|l| *l == s.log_level)
        .unwrap_or(2) as u32;
    let level = adw::ComboRow::builder()
        .title("Xray log level")
        .model(&gtk::StringList::new(&LOG_LEVELS))
        .selected(level_idx)
        .build();
    let menu_cmd = entry("Menu command (v2baloot menu)", &s.menu_command);
    let idle = spin(
        "Idle timeout (seconds)",
        "Drop silent connections after this long so apps reconnect quickly (V2Box: 30)",
        5.0,
        600.0,
        s.conn_idle_secs as f64,
    );
    adv.add(&idle);
    adv.add(&xray);
    adv.add(&level);
    adv.add(&menu_cmd);
    net.add(&adv);
    d.add(&net);

    let u = Rc::clone(ui);
    d.connect_closed(move |_| {
        let mut n = s.clone();
        n.system_proxy = system_proxy.is_active();
        n.bypass_private = bypass.is_active();
        n.autoconnect = autoconnect.is_active();
        n.sub_update_hours = sub_hours.value() as u32;
        n.sub_user_agent = ua.text().trim().to_string();
        n.tray = tray.is_active();
        n.notifications = notifications.is_active();
        n.waybar_show_name = waybar_name.is_active();
        n.socks_port = socks.value() as u16;
        n.http_port = http.value() as u16;
        n.api_port = api.value() as u16;
        // Only touch `listen` if the switch was flipped (keeps a custom address from config.toml).
        if allow_lan.is_active() != (s.listen == "0.0.0.0") {
            n.listen = if allow_lan.is_active() {
                "0.0.0.0".into()
            } else {
                "127.0.0.1".into()
            };
        }
        n.dns = dns
            .text()
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
        if n.dns.is_empty() {
            n.dns = Settings::default().dns;
        }
        n.tun_dns = tun_dns.text().trim().to_string();
        n.test_url = test_url.text().trim().to_string();
        n.test_timeout_ms = timeout.value() as u64;
        n.test_concurrency = conc.value() as usize;
        n.conn_idle_secs = idle.value() as u32;
        n.xray_path = xray.text().trim().to_string();
        if level.selected() != level_idx {
            n.log_level = LOG_LEVELS
                .get(level.selected() as usize)
                .unwrap_or(&"warning")
                .to_string();
        }
        n.menu_command = menu_cmd.text().trim().to_string();
        if n == s {
            return;
        }
        // Apply only what was edited here on top of the daemon's current settings, so changes
        // made elsewhere while the dialog was open aren't reverted.
        let (opened, edited) = (s.clone(), n);
        let u = Rc::clone(&u);
        call(Request::GetSettings, move |res| {
            let mut fresh = match res {
                Ok(Reply::Settings { settings }) => *settings,
                _ => opened.clone(),
            };
            merge_changed(&mut fresh, &opened, &edited);
            u.request(Request::SetSettings {
                settings: Box::new(fresh),
            });
            u.toast("Preferences saved");
        });
    });
    d.present(Some(&ui.window));
}
