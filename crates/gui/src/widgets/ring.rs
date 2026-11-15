//! The big round connect button: a ring that spins while connecting and glows when connected.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

use super::{palette, rgbf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RingMode {
    Off,
    Connecting,
    Connected,
    Error,
}

struct Inner {
    mode: Cell<RingMode>,
    hover: Cell<bool>,
    /// Seconds since the animation started (from the frame clock).
    t: Cell<f64>,
    start: Cell<Option<i64>>,
    /// 0→1 cross-fade when switching modes.
    fade: Cell<f64>,
    on_click: RefCell<Option<Box<dyn Fn()>>>,
}

#[derive(Clone)]
pub struct Ring {
    pub widget: gtk::DrawingArea,
    inner: Rc<Inner>,
}

impl Ring {
    pub fn new(size: i32) -> Ring {
        let widget = gtk::DrawingArea::builder()
            .content_width(size)
            .content_height(size)
            .halign(gtk::Align::Center)
            .focusable(true)
            .can_focus(true)
            .css_classes(["connect-ring"])
            .tooltip_text("Connect / disconnect")
            .build();
        widget.set_cursor_from_name(Some("pointer"));
        let inner = Rc::new(Inner {
            mode: Cell::new(RingMode::Off),
            hover: Cell::new(false),
            t: Cell::new(0.0),
            start: Cell::new(None),
            fade: Cell::new(1.0),
            on_click: RefCell::new(None),
        });

        let i = Rc::clone(&inner);
        widget.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, &i));

        // Animation clock: always running but cheap (one small redraw per frame only while
        // something animates).
        let i = Rc::clone(&inner);
        widget.add_tick_callback(move |w, clock| {
            let now = clock.frame_time();
            let start = i.start.get().unwrap_or(now);
            i.start.set(Some(start));
            i.t.set((now - start) as f64 / 1_000_000.0);
            if i.fade.get() < 1.0 {
                i.fade.set((i.fade.get() + 1.0 / 18.0).min(1.0));
            }
            if matches!(i.mode.get(), RingMode::Connecting | RingMode::Connected)
                || i.fade.get() < 1.0
            {
                w.queue_draw();
            }
            glib::ControlFlow::Continue
        });

        let click = gtk::GestureClick::new();
        let i = Rc::clone(&inner);
        click.connect_released(move |_, _, _, _| {
            if let Some(cb) = i.on_click.borrow().as_ref() {
                cb();
            }
        });
        widget.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let (i1, i2) = (Rc::clone(&inner), Rc::clone(&inner));
        let (w1, w2) = (widget.clone(), widget.clone());
        motion.connect_enter(move |_, _, _| {
            i1.hover.set(true);
            w1.queue_draw();
        });
        motion.connect_leave(move |_| {
            i2.hover.set(false);
            w2.queue_draw();
        });
        widget.add_controller(motion);

        let keys = gtk::EventControllerKey::new();
        let i = Rc::clone(&inner);
        keys.connect_key_pressed(move |_, key, _, _| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space) {
                if let Some(cb) = i.on_click.borrow().as_ref() {
                    cb();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        widget.add_controller(keys);

        Ring { widget, inner }
    }

    pub fn connect_clicked(&self, f: impl Fn() + 'static) {
        *self.inner.on_click.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_mode(&self, mode: RingMode) {
        if self.inner.mode.get() != mode {
            self.inner.mode.set(mode);
            self.inner.fade.set(0.0);
            self.widget.queue_draw();
        }
    }
}

fn ease(x: f64) -> f64 {
    // easeInOutCubic
    if x < 0.5 {
        4.0 * x * x * x
    } else {
        1.0 - (-2.0 * x + 2.0).powi(3) / 2.0
    }
}

fn draw(cr: &cairo::Context, w: f64, h: f64, i: &Inner) {
    let p = palette();
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = w.min(h) / 2.0 - 16.0;
    let lw = 9.0;
    let fade = ease(i.fade.get());
    let t = i.t.get();
    let hover = if i.hover.get() { 1.0 } else { 0.0 };
    let fg = rgbf(&p.fg);
    let accent = rgbf(&p.accent);
    let green = rgbf(&p.green);
    let red = rgbf(&p.red);
    cr.set_line_cap(cairo::LineCap::Round);

    // Soft disc behind everything.
    cr.arc(cx, cy, r + lw / 2.0, 0.0, 2.0 * PI);
    cr.set_source_rgba(fg.0, fg.1, fg.2, 0.035 + 0.025 * hover);
    let _ = cr.fill();

    // Track.
    cr.arc(cx, cy, r, 0.0, 2.0 * PI);
    cr.set_line_width(lw);
    cr.set_source_rgba(fg.0, fg.1, fg.2, 0.10);
    let _ = cr.stroke();

    let mode = i.mode.get();
    let main = match mode {
        RingMode::Off => fg,
        RingMode::Connecting => accent,
        RingMode::Connected => green,
        RingMode::Error => red,
    };

    match mode {
        RingMode::Off => {}
        RingMode::Connecting => {
            // A comet: one bright arc with a fading tail, rotating with easing.
            let turn = t * 0.9;
            let base = (turn - turn.floor()) * 2.0 * PI;
            let start = base - PI / 2.0 + ease((turn * 2.0).fract()) * 0.6;
            let len = 1.1 + 0.5 * (t * 2.2).sin();
            for k in 0..14 {
                let f = k as f64 / 14.0;
                let a0 = start - len * (1.0 - f);
                let a1 = a0 + len / 14.0;
                cr.arc(cx, cy, r, a0, a1);
                cr.set_line_width(lw);
                cr.set_source_rgba(main.0, main.1, main.2, (0.08 + 0.92 * f) * fade);
                let _ = cr.stroke();
            }
        }
        RingMode::Connected | RingMode::Error => {
            // Glow: wide translucent strokes, gently breathing when connected.
            let breathe = if mode == RingMode::Connected {
                0.5 + 0.5 * (t * 1.6).sin()
            } else {
                0.0
            };
            for (extra, alpha) in [(14.0, 0.05), (8.0, 0.08), (4.0, 0.12)] {
                cr.arc(cx, cy, r, 0.0, 2.0 * PI);
                cr.set_line_width(lw + extra + 4.0 * breathe);
                cr.set_source_rgba(main.0, main.1, main.2, alpha * (0.7 + 0.3 * breathe) * fade);
                let _ = cr.stroke();
            }
            // Full ring sweeping in on entry.
            let sweep = 2.0 * PI * fade;
            cr.arc(cx, cy, r, -PI / 2.0, -PI / 2.0 + sweep);
            cr.set_line_width(lw);
            cr.set_source_rgba(main.0, main.1, main.2, 1.0);
            let _ = cr.stroke();
        }
    }

    // Power glyph in the middle.
    let g = r * 0.30;
    let glyph = if mode == RingMode::Off { fg } else { main };
    let alpha = if mode == RingMode::Off {
        0.55 + 0.35 * hover
    } else {
        1.0
    };
    cr.set_source_rgba(glyph.0, glyph.1, glyph.2, alpha);
    cr.set_line_width(lw * 0.9);
    cr.new_path();
    cr.arc(cx, cy + g * 0.08, g, -PI / 2.0 + 0.62, 1.5 * PI - 0.62);
    let _ = cr.stroke();
    cr.move_to(cx, cy - g * 1.15);
    cr.line_to(cx, cy - g * 0.15);
    let _ = cr.stroke();
}
