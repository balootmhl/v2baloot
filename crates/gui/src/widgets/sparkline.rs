//! A smooth, scrolling throughput graph (download filled, upload as a line).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, glib};

use super::{palette, rgbf};

const POINTS: usize = 60;

struct Inner {
    data: RefCell<VecDeque<(f64, f64)>>,
    /// Frame time of the last sample; drives the smooth scroll between samples.
    last_push: Cell<i64>,
    now: Cell<i64>,
}

#[derive(Clone)]
pub struct Sparkline {
    pub widget: gtk::DrawingArea,
    inner: Rc<Inner>,
}

impl Sparkline {
    pub fn new() -> Sparkline {
        let widget = gtk::DrawingArea::builder()
            .content_height(64)
            .hexpand(true)
            .css_classes(["sparkline"])
            .build();
        let inner = Rc::new(Inner {
            data: RefCell::new(VecDeque::from(vec![(0.0, 0.0); POINTS])),
            last_push: Cell::new(0),
            now: Cell::new(0),
        });
        let i = Rc::clone(&inner);
        widget.set_draw_func(move |_, cr, w, h| draw(cr, w as f64, h as f64, &i));
        let i = Rc::clone(&inner);
        widget.add_tick_callback(move |w, clock| {
            i.now.set(clock.frame_time());
            if i.now.get() - i.last_push.get() < 1_100_000 {
                w.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        Sparkline { widget, inner }
    }

    pub fn push(&self, down: u64, up: u64) {
        let mut d = self.inner.data.borrow_mut();
        d.push_back((down as f64, up as f64));
        while d.len() > POINTS {
            d.pop_front();
        }
        self.inner.last_push.set(self.inner.now.get());
        self.widget.queue_draw();
    }

    pub fn clear(&self) {
        *self.inner.data.borrow_mut() = VecDeque::from(vec![(0.0, 0.0); POINTS]);
        self.widget.queue_draw();
    }
}

fn path(cr: &cairo::Context, pts: &[(f64, f64)]) {
    cr.move_to(pts[0].0, pts[0].1);
    for w in pts.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        let mx = (x0 + x1) / 2.0;
        cr.curve_to(mx, y0, mx, y1, x1, y1);
    }
}

fn draw(cr: &cairo::Context, w: f64, h: f64, i: &Inner) {
    let p = palette();
    let data = i.data.borrow();
    let max = data
        .iter()
        .map(|(d, u)| d.max(*u))
        .fold(16.0 * 1024.0, f64::max)
        * 1.15;
    let step = w / (POINTS as f64 - 2.0);
    // Scroll left smoothly during the second after a new sample.
    let progress = ((i.now.get() - i.last_push.get()) as f64 / 1_000_000.0).clamp(0.0, 1.0);
    let shift = step * (1.0 - progress);
    let pad = 3.0;
    let y = |v: f64| h - pad - (v / max) * (h - 2.0 * pad);
    let pts = |sel: fn(&(f64, f64)) -> f64| -> Vec<(f64, f64)> {
        data.iter()
            .enumerate()
            .map(|(k, v)| (k as f64 * step - step + shift, y(sel(v))))
            .collect()
    };
    let down = pts(|v| v.0);
    let up = pts(|v| v.1);

    let accent = rgbf(&p.accent);
    let green = rgbf(&p.green);
    let fg = rgbf(&p.fg);

    // Baseline.
    cr.set_source_rgba(fg.0, fg.1, fg.2, 0.08);
    cr.set_line_width(1.0);
    cr.move_to(0.0, h - pad);
    cr.line_to(w, h - pad);
    let _ = cr.stroke();

    // Download: gradient-filled area.
    path(cr, &down);
    cr.line_to(down.last().unwrap().0, h);
    cr.line_to(down[0].0, h);
    cr.close_path();
    let grad = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    grad.add_color_stop_rgba(0.0, accent.0, accent.1, accent.2, 0.35);
    grad.add_color_stop_rgba(1.0, accent.0, accent.1, accent.2, 0.0);
    let _ = cr.set_source(&grad);
    let _ = cr.fill();
    path(cr, &down);
    cr.set_source_rgba(accent.0, accent.1, accent.2, 0.95);
    cr.set_line_width(2.0);
    let _ = cr.stroke();

    // Upload: thin line.
    path(cr, &up);
    cr.set_source_rgba(green.0, green.1, green.2, 0.85);
    cr.set_line_width(1.5);
    let _ = cr.stroke();
}
