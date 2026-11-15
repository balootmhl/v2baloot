//! Left pane: the big connect ring, live status, mode, current server, speed, exit IP.

use adw::prelude::*;
use gtk::glib;

use v2baloot_core::util::{
    country_flag, flag_from_name, format_date, human_bytes, human_speed, now_unix, strip_flag,
};
use v2baloot_core::{ConnState, Latency, Mode, Server};

use crate::widgets::ring::{Ring, RingMode};
use crate::widgets::sparkline::Sparkline;
use crate::window::State;

pub struct Dashboard {
    pub root: gtk::ScrolledWindow,
    pub ring: Ring,
    pub mode: adw::ToggleGroup,
    title: gtk::Label,
    sub: gtk::Label,
    server_card: gtk::Box,
    server_flag: gtk::Label,
    server_name: gtk::Label,
    server_kind: gtk::Label,
    server_latency: gtk::Label,
    stats: gtk::Revealer,
    down: gtk::Label,
    up: gtk::Label,
    total_down: gtk::Label,
    total_up: gtk::Label,
    pub spark: Sparkline,
    exit: gtk::Revealer,
    exit_stack: gtk::Stack,
    exit_flag: gtk::Label,
    exit_place: gtk::Label,
    exit_ip: gtk::Label,
    exit_isp: gtk::Label,
    exit_pending: gtk::Label,
    usage: gtk::Revealer,
    usage_name: gtk::Label,
    usage_bar: gtk::ProgressBar,
    usage_text: gtk::Label,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(classes.to_vec())
        .xalign(0.0)
        .build()
}

fn stat(caption: &str, class: &str) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .css_classes(["stat"])
        .hexpand(true)
        .build();
    b.append(&label(caption, &["stat-caption"]));
    let v = label("—", &["stat-value", class]);
    b.append(&v);
    (b, v)
}

fn card(title: &str) -> gtk::Box {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .css_classes(["card"])
        .build();
    b.append(&label(title, &["card-title"]));
    b
}

fn revealer(child: &impl IsA<gtk::Widget>) -> gtk::Revealer {
    gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(220)
        .child(child)
        .build()
}

/// "00:12:34", or "2d 03:04:05" for long sessions.
pub fn session_time(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m, s) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

/// Flag for a server: one in its name, else a globe.
pub fn server_flag(s: &Server) -> String {
    flag_from_name(&s.name).unwrap_or_else(|| "🌐".into())
}

pub fn server_title(s: &Server) -> String {
    if flag_from_name(&s.name).is_some() {
        let t = strip_flag(&s.name);
        if t.is_empty() {
            s.name.clone()
        } else {
            t
        }
    } else {
        s.name.clone()
    }
}

pub fn latency_text(l: Option<&Latency>) -> (String, &'static str) {
    match l {
        None => ("—".into(), "none"),
        Some(Latency::Failed { .. }) => ("timeout".into(), "bad"),
        Some(Latency::Ok { ms }) if *ms < 300 => (format!("{ms} ms"), "good"),
        Some(Latency::Ok { ms }) if *ms < 800 => (format!("{ms} ms"), "ok"),
        Some(Latency::Ok { ms }) => (format!("{ms} ms"), "bad"),
    }
}

fn set_class(w: &impl IsA<gtk::Widget>, all: &[&str], on: &str) {
    for c in all {
        w.remove_css_class(c);
    }
    if !on.is_empty() {
        w.add_css_class(on);
    }
}

impl Dashboard {
    pub fn new() -> Dashboard {
        let col = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .css_classes(["dashboard"])
            .build();

        let ring = Ring::new(212);
        col.append(&ring.widget);

        let title = gtk::Label::builder()
            .css_classes(["status-title"])
            .justify(gtk::Justification::Center)
            .build();
        let sub = gtk::Label::builder()
            .css_classes(["status-sub"])
            .justify(gtk::Justification::Center)
            .wrap(true)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .selectable(false)
            .build();
        let heading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        heading.append(&title);
        heading.append(&sub);
        col.append(&heading);

        let mode = adw::ToggleGroup::builder()
            .css_classes(["mode-switch", "round"])
            .homogeneous(true)
            .build();
        mode.add(
            adw::Toggle::builder()
                .name("proxy")
                .label("Proxy")
                .tooltip("Local SOCKS/HTTP proxy + system proxy")
                .build(),
        );
        mode.add(
            adw::Toggle::builder()
                .name("tun")
                .label("TUN")
                .tooltip("Every app through a virtual interface")
                .build(),
        );
        col.append(&mode);

        // Current server
        let server_card = card("SERVER");
        server_card.add_css_class("clickable");
        server_card.set_cursor_from_name(Some("pointer"));
        let row = gtk::Box::builder().spacing(12).build();
        let server_flag = label("🌐", &["big-flag"]);
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let server_name = gtk::Label::builder()
            .css_classes(["server-name"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let server_kind = label("", &["dim", "small"]);
        names.append(&server_name);
        names.append(&server_kind);
        let server_latency = label("", &["latency-ms"]);
        server_latency.set_xalign(1.0);
        row.append(&server_flag);
        row.append(&names);
        row.append(&server_latency);
        row.append(
            &gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .css_classes(["dim"])
                .build(),
        );
        server_card.append(&row);
        col.append(&server_card);

        // Speed
        let stats_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .build();
        let grid = gtk::Grid::builder()
            .column_spacing(10)
            .row_spacing(10)
            .column_homogeneous(true)
            .build();
        let (b1, down) = stat("DOWNLOAD", "down");
        let (b2, up) = stat("UPLOAD", "up");
        let (b3, total_down) = stat("RECEIVED", "");
        let (b4, total_up) = stat("SENT", "");
        grid.attach(&b1, 0, 0, 1, 1);
        grid.attach(&b2, 1, 0, 1, 1);
        grid.attach(&b3, 0, 1, 1, 1);
        grid.attach(&b4, 1, 1, 1, 1);
        stats_box.append(&grid);
        let spark = Sparkline::new();
        let spark_card = gtk::Box::builder().css_classes(["card"]).build();
        spark_card.append(&spark.widget);
        stats_box.append(&spark_card);
        let stats = revealer(&stats_box);
        col.append(&stats);

        // Exit IP
        let exit_card = card("EXIT");
        let exit_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(200)
            .build();
        let loaded = gtk::Box::builder().spacing(12).build();
        let exit_flag = label("🌐", &["big-flag"]);
        let exit_text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        let exit_place = gtk::Label::builder()
            .css_classes(["server-name"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let exit_ip = gtk::Label::builder()
            .css_classes(["mono", "small"])
            .xalign(0.0)
            .selectable(true)
            .build();
        let exit_isp = gtk::Label::builder()
            .css_classes(["dim", "small"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        exit_text.append(&exit_place);
        exit_text.append(&exit_ip);
        exit_text.append(&exit_isp);
        loaded.append(&exit_flag);
        loaded.append(&exit_text);
        let pending = gtk::Box::builder().spacing(10).build();
        pending.append(
            &adw::Spinner::builder()
                .width_request(18)
                .height_request(18)
                .build(),
        );
        let exit_pending = label("Looking up your exit IP…", &["dim", "small"]);
        pending.append(&exit_pending);
        exit_stack.add_named(&loaded, Some("loaded"));
        exit_stack.add_named(&pending, Some("pending"));
        exit_card.append(&exit_stack);
        let exit = revealer(&exit_card);
        col.append(&exit);

        // Subscription usage
        let usage_card = card("SUBSCRIPTION");
        let usage_name = gtk::Label::builder()
            .css_classes(["server-name"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let usage_bar = gtk::ProgressBar::builder()
            .css_classes(["test-progress"])
            .build();
        let usage_text = label("", &["dim", "small"]);
        usage_card.append(&usage_name);
        usage_card.append(&usage_bar);
        usage_card.append(&usage_text);
        let usage = revealer(&usage_card);
        col.append(&usage);

        let clamp = adw::Clamp::builder().maximum_size(420).child(&col).build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();

        Dashboard {
            root,
            ring,
            mode,
            title,
            sub,
            server_card,
            server_flag,
            server_name,
            server_kind,
            server_latency,
            stats,
            down,
            up,
            total_down,
            total_up,
            spark,
            exit,
            exit_stack,
            exit_flag,
            exit_place,
            exit_ip,
            exit_isp,
            exit_pending,
            usage,
            usage_name,
            usage_bar,
            usage_text,
        }
    }

    pub fn connect_server_card(&self, f: impl Fn() + 'static) {
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| f());
        self.server_card.add_controller(click);
    }

    /// The server shown in the card: the connected one, else the selected one.
    fn current(st: &State) -> Option<&Server> {
        let id = st.status.server_id.as_ref().or(st.selected.as_ref())?;
        st.servers.iter().find(|s| &s.id == id)
    }

    pub fn update(&self, st: &State) {
        let s = &st.status;
        let (mode, title, title_class) = if !st.online {
            (RingMode::Off, "Daemon offline", "")
        } else {
            match &s.state {
                ConnState::Connected => (RingMode::Connected, "Connected", "connected"),
                ConnState::Connecting => (RingMode::Connecting, "Connecting…", ""),
                ConnState::Disconnected => (RingMode::Off, "Not connected", ""),
                ConnState::Error { .. } => (RingMode::Error, "Connection failed", "error"),
            }
        };
        self.ring.set_mode(mode);
        self.title.set_label(title);
        set_class(&self.title, &["connected", "error"], title_class);
        self.tick(st);

        self.mode
            .set_active_name(Some(if s.mode == Mode::Tun { "tun" } else { "proxy" }));
        self.mode.set_sensitive(st.online);

        match Self::current(st) {
            Some(server) => {
                self.server_flag.set_label(&server_flag(server));
                self.server_name.set_label(&server_title(server));
                self.server_kind.set_label(&format!(
                    "{} · {}",
                    server.kind_label(),
                    server.address
                ));
                let (t, c) = latency_text(server.latency.as_ref());
                self.server_latency.set_label(&t);
                set_class(&self.server_latency, &["good", "ok", "bad", "none"], c);
            }
            None => {
                self.server_flag.set_label("🌐");
                self.server_name.set_label(if st.servers.is_empty() {
                    "No servers yet"
                } else {
                    "Choose a server"
                });
                self.server_kind.set_label(if st.servers.is_empty() {
                    "Import a link or add a subscription"
                } else {
                    "Tap to open the server list"
                });
                self.server_latency.set_label("");
            }
        }

        let connected = st.online && s.is_connected();
        self.stats.set_reveal_child(connected);
        if connected {
            self.down.set_label(&human_speed(s.down_speed));
            self.up.set_label(&human_speed(s.up_speed));
            self.total_down.set_label(&human_bytes(s.total_down));
            self.total_up.set_label(&human_bytes(s.total_up));
        } else {
            self.spark.clear();
        }

        self.exit.set_reveal_child(connected);
        match &s.exit {
            Some(e) => {
                self.exit_flag
                    .set_label(&country_flag(&e.country_code).unwrap_or_else(|| "🌐".into()));
                let place = match (e.city.is_empty(), e.country.is_empty()) {
                    (false, false) if e.city != e.country => format!("{}, {}", e.city, e.country),
                    (_, false) => e.country.clone(),
                    _ => "Unknown location".into(),
                };
                self.exit_place.set_label(&place);
                self.exit_ip.set_label(&e.ip);
                self.exit_isp.set_label(&e.isp);
                self.exit_isp.set_visible(!e.isp.is_empty());
                self.exit_stack.set_visible_child_name("loaded");
            }
            None => {
                let waited = s.connected_since.map(|t| now_unix() - t).unwrap_or(0);
                self.exit_pending.set_label(if waited > 10 {
                    "Exit IP unavailable"
                } else {
                    "Looking up your exit IP…"
                });
                self.exit_stack.set_visible_child_name("pending");
            }
        }

        let sub = Self::current(st)
            .and_then(|server| server.group.as_ref())
            .and_then(|g| st.subs.iter().find(|x| &x.id == g));
        match sub.and_then(|x| x.usage.as_ref().map(|u| (x, u))) {
            Some((x, u)) => {
                self.usage_name.set_label(&x.name);
                let used = u.upload + u.download;
                let mut text = if u.total > 0 {
                    self.usage_bar
                        .set_fraction((used as f64 / u.total as f64).clamp(0.0, 1.0));
                    self.usage_bar.set_visible(true);
                    format!("{} of {}", human_bytes(used), human_bytes(u.total))
                } else {
                    self.usage_bar.set_visible(false);
                    format!("{} used", human_bytes(used))
                };
                if u.expire > 0 {
                    let days = (u.expire - now_unix()) / 86_400;
                    if days >= 0 {
                        text.push_str(&format!(" · {days} days left"));
                    } else {
                        text.push_str(&format!(" · expired {}", format_date(u.expire)));
                    }
                }
                self.usage_text.set_label(&text);
                self.usage.set_reveal_child(true);
            }
            None => self.usage.set_reveal_child(false),
        }
    }

    /// Per-second refresh of the session timer / subtitle.
    pub fn tick(&self, st: &State) {
        let s = &st.status;
        let (text, class) = if !st.online {
            (
                "Start it with: systemctl --user start v2baloot".to_string(),
                "",
            )
        } else {
            match &s.state {
                ConnState::Connected => (
                    format!(
                        "{} · {}",
                        session_time(s.connected_since.map(|t| now_unix() - t).unwrap_or(0)),
                        s.mode.label()
                    ),
                    "",
                ),
                ConnState::Connecting => ("Setting up the tunnel".to_string(), ""),
                ConnState::Disconnected => ("Tap the ring to connect".to_string(), ""),
                ConnState::Error { message } => {
                    (message.lines().next().unwrap_or("").to_string(), "error")
                }
            }
        };
        if self.sub.label() != text {
            self.sub.set_label(&text);
        }
        set_class(&self.sub, &["error"], class);
    }
}

/// Keep the session timer ticking.
pub fn start_clock(f: impl Fn() + 'static) {
    glib::timeout_add_seconds_local(1, move || {
        f();
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_time_format() {
        assert_eq!(session_time(0), "00:00:00");
        assert_eq!(session_time(3_725), "01:02:05");
        assert_eq!(session_time(2 * 86_400 + 5), "2d 00:00:05");
    }
}
