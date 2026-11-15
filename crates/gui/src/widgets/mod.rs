pub mod ring;
pub mod sparkline;

use v2baloot_core::theme::{rgb, Palette};

/// `(r, g, b)` in 0..1 for cairo.
pub fn rgbf(hex: &str) -> (f64, f64, f64) {
    let (r, g, b) = rgb(hex);
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
}

pub fn palette() -> Palette {
    crate::theme::palette()
}
