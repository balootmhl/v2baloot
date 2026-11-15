//! Icons rendered by us, so they show up no matter what the icon theme / notification daemon /
//! tray host can resolve: tray pixmaps (ARGB32), the notification PNG and Waybar's state PNGs.

use std::path::Path;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

use v2baloot_core::paths::Paths;
use v2baloot_core::theme::Palette;
use v2baloot_core::{ConnState, Mode, Status};

const APP_SVG: &str =
    include_str!("../../../packaging/icons/hicolor/scalable/apps/org.omarchy.v2baloot.svg");
/// The "b": tall left wall, floor and arch (16×16 viewBox).
const B_PATH: &str = "M3.5 1.8v12.4h9.2V9.3a4.6 4.6 0 0 0-9.2 0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Connected,
    Disconnected,
    Tun,
    Error,
}

impl IconState {
    pub const ALL: [IconState; 4] = [
        IconState::Connected,
        IconState::Disconnected,
        IconState::Tun,
        IconState::Error,
    ];

    pub fn of(st: &Status) -> IconState {
        match &st.state {
            ConnState::Connected if st.mode == Mode::Tun => IconState::Tun,
            ConnState::Connected | ConnState::Connecting => IconState::Connected,
            ConnState::Error { .. } => IconState::Error,
            ConnState::Disconnected => IconState::Disconnected,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            IconState::Connected => "connected",
            IconState::Disconnected => "disconnected",
            IconState::Tun => "tun",
            IconState::Error => "error",
        }
    }
}

/// 16×16 "b" glyph in `fg`, with a coloured state mark.
pub fn state_svg(state: IconState, p: &Palette) -> String {
    let fg = &p.fg;
    let b = |opacity: f32| {
        format!(
            r#"<path d="{B_PATH}" fill="none" stroke="{fg}" stroke-opacity="{opacity}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#
        )
    };
    let body = match state {
        IconState::Connected => format!(
            r#"{}<path d="M8.1 7.2v4.6M6.3 10l1.8 1.8 1.8-1.8" fill="none" stroke="{}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>"#,
            b(1.0),
            p.green
        ),
        IconState::Disconnected => b(0.55),
        IconState::Tun => format!(
            r#"{}<circle cx="8.1" cy="10.4" r="2.1" fill="{}"/>"#,
            b(1.0),
            p.accent
        ),
        IconState::Error => format!(
            r#"{}<path d="M8.1 6.5v3.4" stroke="{red}" stroke-width="1.8" stroke-linecap="round"/><circle cx="8.1" cy="12.1" r="1" fill="{red}"/>"#,
            b(0.55),
            red = p.red
        ),
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">{body}</svg>"#
    )
}

/// Rasterize an SVG to a square pixmap.
pub fn render(svg: &str, size: u32) -> Result<tiny_skia::Pixmap> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).context("parsing icon SVG")?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size).context("allocating pixmap")?;
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// StatusNotifierItem wants ARGB32 in network byte order, not premultiplied.
pub fn to_argb32(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let c = px.demultiply();
        out.extend_from_slice(&[c.alpha(), c.red(), c.green(), c.blue()]);
    }
    out
}

fn write_png(svg: &str, size: u32, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let png = render(svg, size)?.encode_png().context("encoding PNG")?;
    // Write-then-rename so Waybar never reads a half-written file.
    let tmp = path.with_extension("png.tmp");
    std::fs::write(&tmp, png)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// App icon for notifications.
pub fn write_notify_icon(paths: &Paths) -> Result<()> {
    write_png(APP_SVG, 128, &paths.notify_icon())
}

/// State icons for the Waybar module, tinted with the theme's foreground.
pub fn write_waybar_icons(paths: &Paths, p: &Palette) -> Result<()> {
    for st in IconState::ALL {
        write_png(&state_svg(st, p), 48, &paths.waybar_icon(st.name()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_state_and_app_icon() {
        let p = Palette::default();
        for st in IconState::ALL {
            let pm = render(&state_svg(st, &p), 22).unwrap();
            assert_eq!((pm.width(), pm.height()), (22, 22));
            let argb = to_argb32(&pm);
            assert_eq!(argb.len(), 22 * 22 * 4);
            // Something was drawn.
            assert!(argb.chunks(4).any(|px| px[0] > 0), "{st:?} is empty");
        }
        let app = render(APP_SVG, 64).unwrap();
        // Opaque blue tile in the middle-left area.
        let px = app.pixel(12, 32).unwrap();
        assert_eq!(px.alpha(), 255);
    }

    #[test]
    fn argb_is_unpremultiplied_network_order() {
        let mut pm = tiny_skia::Pixmap::new(1, 1).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 128));
        assert_eq!(to_argb32(&pm), vec![128, 255, 0, 0]);
    }
}
