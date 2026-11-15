//! Follow the active Omarchy theme: map its palette onto libadwaita colours and our own styles.
//! Re-applied live when the theme changes (`omarchy-theme-set` swaps the `current/theme` link).
//! Without an Omarchy theme we use Tokyo Night, Omarchy's default.

use std::cell::RefCell;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use v2baloot_core::theme::{self as core_theme, Palette};

const STYLE_CSS: &str = include_str!("style.css");

thread_local! {
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static PENDING: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    static PALETTE: RefCell<Palette> = RefCell::new(Palette::default());
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = const { RefCell::new(Vec::new()) };
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The palette currently applied (custom-drawn widgets read their colours from it).
pub fn palette() -> Palette {
    PALETTE.with(|p| p.borrow().clone())
}

/// Run `f` after every theme change (used to redraw custom widgets).
pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

fn theme_css(p: &Palette) -> String {
    let soft = format!("mix({}, {}, 0.045)", p.bg, p.fg);
    let vars: [(&str, String); 20] = [
        ("accent-bg-color", p.accent.clone()),
        ("accent-color", p.accent.clone()),
        ("accent-fg-color", p.bg.clone()),
        ("window-bg-color", p.bg.clone()),
        ("window-fg-color", p.fg.clone()),
        ("view-bg-color", p.bg.clone()),
        ("view-fg-color", p.fg.clone()),
        ("headerbar-bg-color", p.bg.clone()),
        ("headerbar-fg-color", p.fg.clone()),
        ("sidebar-bg-color", soft.clone()),
        ("sidebar-fg-color", p.fg.clone()),
        ("popover-bg-color", soft.clone()),
        ("popover-fg-color", p.fg.clone()),
        ("dialog-bg-color", soft),
        ("dialog-fg-color", p.fg.clone()),
        ("card-bg-color", format!("alpha({}, 0.05)", p.fg)),
        ("success-color", p.green.clone()),
        ("warning-color", p.yellow.clone()),
        ("error-color", p.red.clone()),
        ("destructive-bg-color", p.red.clone()),
    ];
    let mut css = String::new();
    // Legacy named colours (libadwaita < 1.6) and CSS variables (>= 1.6).
    for (k, v) in &vars {
        css.push_str(&format!("@define-color {} {v};\n", k.replace('-', "_")));
    }
    css.push_str(":root {\n");
    for (k, v) in &vars {
        css.push_str(&format!("  --{k}: {v};\n"));
    }
    css.push_str("}\n");
    css
}

fn apply() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let p = home()
        .and_then(|h| core_theme::load(&h))
        .unwrap_or_default();
    adw::StyleManager::default().set_color_scheme(if p.is_dark() {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
    let css = format!("{}\n{STYLE_CSS}", theme_css(&p));
    PALETTE.with(|cell| *cell.borrow_mut() = p);
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
    LISTENERS.with(|l| l.borrow().iter().for_each(|f| f()));
}

pub fn init() {
    apply();
    let Some(dir) = home()
        .map(|h| core_theme::omarchy_dir(&h))
        .filter(|d| d.exists())
    else {
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
