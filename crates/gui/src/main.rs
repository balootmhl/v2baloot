//! v2baloot GTK4 / libadwaita front-end.

mod dashboard;
mod dialogs;
mod servers;
mod theme;
mod widgets;
mod window;

use std::sync::OnceLock;

use adw::prelude::*;
use gtk::glib;
use v2baloot_core::ipc::{Client, Reply, Request};

pub const APP_ID: &str = "org.omarchy.v2baloot";

/// Tokio runtime for IPC; results are handed back to the GTK main loop.
pub fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Send a request to the daemon and run `cb` on the GTK thread with the result.
pub fn call(req: Request, cb: impl FnOnce(Result<Reply, String>) + 'static) {
    let handle = rt().spawn(async move {
        Client::default()
            .call(&req)
            .await
            .map_err(|e| format!("{e:#}"))
    });
    glib::spawn_future_local(async move {
        let res = handle.await.unwrap_or_else(|e| Err(e.to_string()));
        cb(res);
    });
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        theme::init();
        app.set_accels_for_action("win.import", &["<Ctrl>i"]);
        app.set_accels_for_action("win.add-sub", &["<Ctrl>n"]);
        app.set_accels_for_action("win.update-subs", &["<Ctrl>r"]);
        app.set_accels_for_action("win.test-all", &["<Ctrl>t"]);
        app.set_accels_for_action("win.search", &["<Ctrl>f"]);
        app.set_accels_for_action("win.toggle", &["<Ctrl>Return"]);
        app.set_accels_for_action("win.prefs", &["<Ctrl>comma"]);
        app.set_accels_for_action("window.close", &["<Ctrl>w", "<Ctrl>q"]);
    });
    app.connect_activate(|app| {
        if let Some(w) = app.active_window() {
            w.present();
            return;
        }
        let window = window::build(app);
        // Development aid: V2BALOOT_SIZE=500x800 for screenshots of the narrow layout.
        if let Some((w, h)) = std::env::var("V2BALOOT_SIZE").ok().and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        }) {
            window.set_default_size(w, h);
        }
        window.present();
        if let Ok(action) = std::env::var("V2BALOOT_DEBUG_ACTION") {
            let w = window.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                let _ = WidgetExt::activate_action(&w, &action, None);
            });
        }
        if let Ok(path) = std::env::var("V2BALOOT_SCREENSHOT") {
            screenshot_and_quit(window, path);
        }
    });
    app.run()
}

/// Development aid: render the window to a PNG after it settles, then quit.
fn screenshot_and_quit(window: adw::ApplicationWindow, path: String) {
    let delay = std::env::var("V2BALOOT_SCREENSHOT_DELAY")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(3);
    glib::timeout_add_local_once(std::time::Duration::from_secs(delay), move || {
        let paintable = gtk::WidgetPaintable::new(Some(&window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
        if let (Some(node), Some(renderer)) = (
            snapshot.to_node(),
            window.native().and_then(|n| n.renderer()),
        ) {
            let _ = renderer.render_texture(&node, None).save_to_png(&path);
        }
        if let Some(app) = window.application() {
            app.quit();
        }
    });
}
