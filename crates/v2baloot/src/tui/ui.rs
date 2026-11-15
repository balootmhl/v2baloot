//! TUI rendering. Only ANSI palette colors are used, so the UI follows the terminal (Omarchy) theme.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use v2baloot_core::util::{format_date, human_bytes, human_speed};
use v2baloot_core::{ConnState, Latency, Mode, SecurityKind, Server};

use super::{App, InputKind, Popup, Tab};

const ACCENT: Color = Color::Blue;

fn latency_span(l: Option<&Latency>) -> Span<'static> {
    match l {
        None => Span::styled("—", Style::new().fg(Color::DarkGray)),
        Some(Latency::Failed { .. }) => Span::styled("timeout", Style::new().fg(Color::Red)),
        Some(Latency::Ok { ms }) => {
            let c = match ms {
                0..=299 => Color::Green,
                300..=799 => Color::Yellow,
                _ => Color::Red,
            };
            Span::styled(format!("{ms} ms"), Style::new().fg(c))
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else if n <= 1 {
        "…".into()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(2),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(body);
    draw_list(f, app, left);
    draw_details(f, app, right);
    draw_footer(f, app, footer);
    draw_popup(f, app);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let st = &app.status;
    let (dot, state, color) = if !app.online {
        ("○", "daemon offline".to_string(), Color::DarkGray)
    } else {
        match &st.state {
            ConnState::Connected => ("●", "Connected".into(), Color::Green),
            ConnState::Connecting => ("◌", "Connecting".into(), Color::Yellow),
            ConnState::Disconnected => ("○", "Disconnected".into(), Color::DarkGray),
            ConnState::Error { .. } => ("●", "Error".into(), Color::Red),
        }
    };
    let mut left = vec![
        Span::styled(
            " v2baloot ",
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw(" "),
        Span::styled(format!("{dot} {state}"), Style::new().fg(color).bold()),
    ];
    if app.online {
        let mode_color = if st.mode == Mode::Tun {
            Color::Magenta
        } else {
            Color::Cyan
        };
        left.push(Span::raw("  "));
        left.push(Span::styled(
            format!("[{}]", st.mode.label()),
            Style::new().fg(mode_color),
        ));
        if let Some(n) = &st.server_name {
            left.push(Span::raw("  "));
            left.push(Span::raw(n.clone()));
        }
    }
    f.render_widget(Paragraph::new(Line::from(left)), area);
    if app.online && st.is_connected() {
        let speed = format!(
            "↑ {}  ↓ {} ",
            human_speed(st.up_speed),
            human_speed(st.down_speed)
        );
        f.render_widget(
            Paragraph::new(speed)
                .alignment(Alignment::Right)
                .fg(Color::DarkGray),
            area,
        );
    }
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [tabs_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    let titles: Vec<String> = app.tabs().into_iter().map(|(_, t)| t).collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.tab)
            .style(Style::new().fg(Color::DarkGray))
            .highlight_style(Style::new().fg(ACCENT).bold().underlined())
            .divider("│"),
        tabs_area,
    );

    let connected_id = app
        .status
        .is_active()
        .then(|| app.status.server_id.clone())
        .flatten();
    let width = list_area.width as usize;
    let kind_w = 22.min(width / 3);
    let lat_w = 8;
    let name_w = width.saturating_sub(kind_w + lat_w + 5);
    let visible = app.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .map(|s| {
            let marker = if connected_id.as_ref() == Some(&s.id) {
                Span::styled("● ", Style::new().fg(Color::Green))
            } else if app.selected_id.as_ref() == Some(&s.id) {
                Span::styled("○ ", Style::new().fg(Color::DarkGray))
            } else {
                Span::raw("  ")
            };
            let name = truncate(&s.name, name_w);
            let pad = name_w.saturating_sub(Span::raw(name.as_str()).width());
            let lat = latency_span(s.latency.as_ref());
            let lat_pad = lat_w.saturating_sub(lat.content.chars().count());
            ListItem::new(Line::from(vec![
                marker,
                Span::raw(name),
                Span::raw(" ".repeat(pad + 1)),
                Span::styled(
                    format!("{:<kind_w$}", truncate(&s.kind_label(), kind_w)),
                    Style::new().fg(Color::DarkGray),
                ),
                Span::raw(" ".repeat(lat_pad + 1)),
                lat,
            ]))
        })
        .collect();
    let empty = items.is_empty();
    drop(visible);
    if empty {
        let text = if !app.online {
            "Waiting for the daemon…\n\nStart it with: systemctl --user start v2baloot"
        } else if !app.filter.is_empty() {
            "No servers match the search."
        } else {
            "No servers yet.\n\nPress  i  to import a link from the clipboard\n      a  to add a subscription URL"
        };
        f.render_widget(
            Paragraph::new(text)
                .fg(Color::DarkGray)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            list_area.inner(ratatui::layout::Margin::new(1, 2)),
        );
        return;
    }
    let list = List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, list_area, &mut app.list);
}

fn kv(k: &str, v: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{k:<10}"), Style::new().fg(Color::DarkGray)),
        Span::raw(v.into()),
    ])
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(" Details ");
    let mut lines: Vec<Line> = vec![];
    if let Some(s) = app.current() {
        lines.push(Line::from(Span::styled(
            s.name.clone(),
            Style::new().bold(),
        )));
        lines.push(Line::raw(""));
        lines.extend(server_lines(s));
        if let Some(sub) = s
            .group
            .as_ref()
            .and_then(|g| app.subs.iter().find(|x| &x.id == g))
        {
            lines.push(Line::raw(""));
            lines.push(kv("Group", sub.name.clone()));
        }
    }
    if let Tab::Sub(id) = app.current_tab() {
        if let Some(sub) = app.subs.iter().find(|x| x.id == id) {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "Subscription",
                Style::new().fg(ACCENT).bold(),
            )));
            if let Some(u) = &sub.usage {
                let used = u.upload + u.download;
                let total = if u.total > 0 {
                    human_bytes(u.total)
                } else {
                    "∞".into()
                };
                lines.push(kv("Usage", format!("{} / {total}", human_bytes(used))));
                if u.expire > 0 {
                    lines.push(kv("Expires", format_date(u.expire)));
                }
            }
            if let Some(t) = sub.updated_at {
                lines.push(kv("Updated", format_date(t)));
            }
            if let Some(e) = &sub.last_error {
                lines.push(Line::from(Span::styled(
                    format!("⚠ {e}"),
                    Style::new().fg(Color::Red),
                )));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn server_lines(s: &Server) -> Vec<Line<'static>> {
    let t = &s.transport;
    let sec = &s.security;
    let mut l = vec![
        kv("Protocol", s.protocol.name().to_uppercase()),
        kv("Address", format!("{}:{}", s.address, s.port)),
        kv("Transport", t.network.name()),
    ];
    if let Some(p) = &t.path {
        l.push(kv("Path", p.clone()));
    }
    if let Some(h) = &t.host {
        l.push(kv("Host", h.clone()));
    }
    if let Some(n) = &t.service_name {
        l.push(kv("Service", n.clone()));
    }
    l.push(kv("Security", sec.kind.name()));
    if sec.kind != SecurityKind::None {
        if let Some(sni) = &sec.sni {
            l.push(kv("SNI", sni.clone()));
        }
        if let Some(fp) = &sec.fingerprint {
            l.push(kv("FP", fp.clone()));
        }
    }
    if let v2baloot_core::Protocol::Vless {
        flow: Some(flow), ..
    } = &s.protocol
    {
        l.push(kv("Flow", flow.clone()));
    }
    l.push(Line::from(vec![
        Span::styled(
            format!("{:<10}", "Latency"),
            Style::new().fg(Color::DarkGray),
        ),
        latency_span(s.latency.as_ref()),
    ]));
    l
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let [msg, keys] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let line = if let Some(b) = &app.busy {
        Line::from(Span::styled(
            format!(" {b}"),
            Style::new().fg(Color::Yellow),
        ))
    } else if app.status.testing {
        Line::from(Span::styled(
            " Testing latency…",
            Style::new().fg(Color::Yellow),
        ))
    } else if let Some((m, err)) = &app.message {
        Line::from(Span::styled(
            format!(" {}", m.lines().next().unwrap_or("")),
            Style::new().fg(if *err { Color::Red } else { Color::Green }),
        ))
    } else if !app.filter.is_empty() {
        Line::from(Span::styled(
            format!(" Search: {}", app.filter),
            Style::new().fg(Color::Cyan),
        ))
    } else {
        Line::raw("")
    };
    f.render_widget(Paragraph::new(line), msg);

    let hints = [
        ("⏎", "connect"),
        ("d", "disconnect"),
        ("m", "mode"),
        ("t/T", "test"),
        ("s", "sort"),
        ("i", "import"),
        ("a", "add sub"),
        ("u", "update"),
        ("/", "search"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (k, d) in hints {
        spans.push(Span::styled(k, Style::new().fg(ACCENT).bold()));
        spans.push(Span::styled(
            format!(" {d}  "),
            Style::new().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), keys);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
    .intersection(area)
}

fn popup_block(title: &str) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(" {title} "))
}

fn draw_popup(f: &mut Frame, app: &App) {
    let area = f.area();
    match &app.popup {
        Popup::None => {}
        Popup::Search => {
            let r = Rect {
                x: area.x + 1,
                y: area.bottom().saturating_sub(5),
                width: area.width.saturating_sub(2).min(60),
                height: 3,
            }
            .intersection(area);
            if r.height < 3 {
                return;
            }
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(format!("{}▏", app.filter))
                    .block(popup_block("Search (Enter keep · Esc clear)")),
                r,
            );
        }
        Popup::Input { kind, text } => {
            let (title, hint) = match kind {
                InputKind::AddSubscription => {
                    ("Add subscription", "Paste the subscription URL (https://…)")
                }
                InputKind::Import => ("Import", "Paste vless:// vmess:// trojan:// ss:// link(s)"),
            };
            let r = centered(area, 72, 6);
            f.render_widget(Clear, r);
            let shown: String = {
                let flat = text.trim_end().replace('\n', " ⏎ ");
                let max = r.width.saturating_sub(4) as usize;
                let n = flat.chars().count();
                if n > max {
                    flat.chars().skip(n - max).collect()
                } else {
                    flat
                }
            };
            let body = vec![
                Line::styled(hint, Style::new().fg(Color::DarkGray)),
                Line::raw(""),
                Line::raw(format!("{shown}▏")),
            ];
            f.render_widget(Paragraph::new(body).block(popup_block(title)), r);
        }
        Popup::Confirm { title, .. } => {
            let r = centered(area, (title.chars().count() as u16 + 6).max(30), 5);
            f.render_widget(Clear, r);
            let body = vec![
                Line::raw(title.clone()),
                Line::raw(""),
                Line::styled(
                    "y / Enter = yes · any other key = no",
                    Style::new().fg(Color::DarkGray),
                ),
            ];
            f.render_widget(
                Paragraph::new(body)
                    .block(popup_block("Confirm"))
                    .wrap(Wrap { trim: true }),
                r,
            );
        }
        Popup::Help => {
            let rows = [
                ("j/k ↑/↓", "move"),
                ("h/l Tab", "switch group"),
                ("Enter", "connect to selected server"),
                ("c / Space", "connect / disconnect"),
                ("d", "disconnect"),
                ("m", "switch Proxy / TUN mode"),
                ("t", "test selected server"),
                ("T", "test all servers in this tab"),
                ("s", "sort by latency"),
                ("/", "search"),
                ("i", "import link(s) from clipboard"),
                ("I", "paste link(s) manually"),
                ("a", "add subscription"),
                ("u", "update subscription(s)"),
                ("x", "delete server"),
                ("X", "delete subscription (in its tab)"),
                ("L", "xray logs"),
                ("r", "refresh"),
                ("q / Esc", "quit (connection keeps running)"),
            ];
            let r = centered(area, 56, rows.len() as u16 + 2);
            f.render_widget(Clear, r);
            let lines: Vec<Line> = rows
                .iter()
                .map(|(k, d)| {
                    Line::from(vec![
                        Span::styled(format!(" {k:<12}"), Style::new().fg(ACCENT).bold()),
                        Span::raw(*d),
                    ])
                })
                .collect();
            f.render_widget(Paragraph::new(lines).block(popup_block("Keys")), r);
        }
        Popup::Logs { scroll } => {
            let r = centered(
                area,
                area.width.saturating_sub(6),
                area.height.saturating_sub(4),
            );
            f.render_widget(Clear, r);
            let text = if app.logs.is_empty() {
                "No log lines yet.".to_string()
            } else {
                app.logs.join("\n")
            };
            f.render_widget(
                Paragraph::new(text)
                    .block(popup_block("xray log (j/k scroll · any key close)"))
                    .scroll((*scroll, 0)),
                r,
            );
        }
    }
}
