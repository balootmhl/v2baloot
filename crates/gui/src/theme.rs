//! Follow the active Omarchy theme: read its Alacritty palette and map it onto libadwaita colors.
//! Re-applied live when the theme changes (`omarchy-theme-set` swaps the `current/theme` link).

use std::cell::RefCell;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

thread_local! {
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PENDING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

/// App-specific styling that does not depend on the theme.
const BASE_CSS: &str = r#"
.latency { font-feature-settings: "tnum"; min-width: 64px; }
.status-strip { padding: 6px 14px; border-top: 1px solid alpha(currentColor, 0.1); }
.status-dot { font-size: 10px; }
.status-dot.connected { color: var(--success-color, @success_color); }
.status-dot.error { color: var(--error-color, @error_color); }
.status-dot.connecting { color: var(--warning-color, @warning_color); }
.mode-badge { font-size: 0.8em; font-weight: bold; padding: 1px 8px; border-radius: 99px;
              background: alpha(var(--accent-bg-color, @accent_bg_color), 0.2); }
.sidebar-subtitle { font-size: 0.85em; }
"#;

fn omarchy_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/omarchy/current"))
}

struct Palette {
    bg: String,
    fg: String,
    accent: String,
    green: String,
    yellow: String,
    red: String,
}

fn norm(c: &str) -> Option<String> {
    let c = c.trim().trim_start_matches("0x").trim_start_matches('#');
    (c.len() == 6 && c.chars().all(|x| x.is_ascii_hexdigit()))
        .then(|| format!("#{}", c.to_ascii_lowercase()))
}

fn load_palette() -> Option<Palette> {
    let path = omarchy_dir()?.join("theme/alacritty.toml");
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    let colors = v.get("colors")?;
    let get = |section: &str, key: &str| {
        colors
            .get(section)
            .and_then(|s| s.get(key))
            .and_then(|x| x.as_str())
            .and_then(norm)
    };
    Some(Palette {
        bg: get("primary", "background")?,
        fg: get("primary", "foreground")?,
        accent: get("normal", "blue").or_else(|| get("bright", "blue"))?,
        green: get("normal", "green").unwrap_or_else(|| "#26a269".into()),
        yellow: get("normal", "yellow").unwrap_or_else(|| "#cd9309".into()),
        red: get("normal", "red").unwrap_or_else(|| "#c01c28".into()),
    })
}

fn luminance(hex: &str) -> f64 {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let (r, g, b) = ((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
    (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64) / 255.0
}

fn theme_css(p: &Palette) -> String {
    let vars = [
        ("accent-bg-color", p.accent.as_str()),
        ("accent-color", p.accent.as_str()),
        ("window-bg-color", p.bg.as_str()),
        ("window-fg-color", p.fg.as_str()),
        ("view-bg-color", p.bg.as_str()),
        ("view-fg-color", p.fg.as_str()),
        ("headerbar-bg-color", p.bg.as_str()),
        ("headerbar-fg-color", p.fg.as_str()),
        ("sidebar-bg-color", p.bg.as_str()),
        ("sidebar-fg-color", p.fg.as_str()),
        ("popover-bg-color", p.bg.as_str()),
        ("popover-fg-color", p.fg.as_str()),
        ("dialog-bg-color", p.bg.as_str()),
        ("dialog-fg-color", p.fg.as_str()),
        ("success-color", p.green.as_str()),
        ("warning-color", p.yellow.as_str()),
        ("error-color", p.red.as_str()),
        ("destructive-bg-color", p.red.as_str()),
    ];
    let mut css = String::new();
    // Legacy named colors (libadwaita < 1.6) …
    for (k, v) in vars {
        css.push_str(&format!("@define-color {} {v};\n", k.replace('-', "_")));
    }
    // … and CSS variables (libadwaita >= 1.6).
    css.push_str(":root {\n");
    for (k, v) in vars {
        css.push_str(&format!("  --{k}: {v};\n"));
    }
    css.push_str(&format!(
        "  --card-bg-color: alpha({}, 0.05);\n  --accent-fg-color: {};\n}}\n",
        p.fg, p.bg
    ));
    css
}

fn apply() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let manager = adw::StyleManager::default();
    let css = match load_palette() {
        Some(p) => {
            manager.set_color_scheme(if luminance(&p.bg) < 0.5 {
                adw::ColorScheme::ForceDark
            } else {
                adw::ColorScheme::ForceLight
            });
            format!("{}\n{BASE_CSS}", theme_css(&p))
        }
        None => {
            manager.set_color_scheme(adw::ColorScheme::PreferDark);
            BASE_CSS.to_string()
        }
    };
    PROVIDER.with(|cell| {
        let mut cell = cell.borrow_mut();
        let provider = cell.get_or_insert_with(|| {
            let p = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(
                &display,
                &p,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            p
        });
        provider.load_from_string(&css);
    });
}

pub fn init() {
    apply();
    let Some(dir) = omarchy_dir().filter(|d| d.exists()) else {
        return;
    };
    let file = gio::File::for_path(dir);
    if let Ok(monitor) =
        file.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    {
        monitor.connect_changed(|_, _, _, _| {
            // Coalesce bursts of events from a theme switch into one reload.
            PENDING.with(|p| {
                if let Some(old) = p.borrow_mut().take() {
                    old.remove();
                }
                let id =
                    glib::timeout_add_local_once(std::time::Duration::from_millis(300), || {
                        PENDING.with(|p| p.borrow_mut().take());
                        apply();
                    });
                *p.borrow_mut() = Some(id);
            });
        });
        MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
    }
}
