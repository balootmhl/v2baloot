//! The active Omarchy theme's palette, read from its Alacritty config.
//! Shared by the GUI, tray icon, Waybar icons and notification icon.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub bg: String,
    pub fg: String,
    pub accent: String,
    pub green: String,
    pub yellow: String,
    pub red: String,
}

impl Default for Palette {
    /// Tokyo Night (Omarchy's default theme).
    fn default() -> Self {
        Palette {
            bg: "#1a1b26".into(),
            fg: "#a9b1d6".into(),
            accent: "#7aa2f7".into(),
            green: "#9ece6a".into(),
            yellow: "#e0af68".into(),
            red: "#f7768e".into(),
        }
    }
}

impl Palette {
    pub fn is_dark(&self) -> bool {
        luminance(&self.bg) < 0.5
    }
}

pub fn omarchy_dir(home: &Path) -> PathBuf {
    home.join(".config/omarchy/current")
}

/// The theme file whose modification time changes when the theme is switched.
pub fn theme_file(home: &Path) -> PathBuf {
    omarchy_dir(home).join("theme/alacritty.toml")
}

/// `#rrggbb` from `#rrggbb`, `0xrrggbb` or `rrggbb`.
pub fn norm(c: &str) -> Option<String> {
    let c = c.trim().trim_start_matches("0x").trim_start_matches('#');
    (c.len() == 6 && c.chars().all(|x| x.is_ascii_hexdigit()))
        .then(|| format!("#{}", c.to_ascii_lowercase()))
}

pub fn luminance(hex: &str) -> f64 {
    let (r, g, b) = rgb(hex);
    (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64) / 255.0
}

pub fn rgb(hex: &str) -> (u8, u8, u8) {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    ((n >> 16) as u8, (n >> 8) as u8, n as u8)
}

pub fn parse_alacritty(text: &str) -> Option<Palette> {
    let v: toml::Value = toml::from_str(text).ok()?;
    let colors = v.get("colors")?;
    let get = |section: &str, key: &str| {
        colors
            .get(section)
            .and_then(|s| s.get(key))
            .and_then(|x| x.as_str())
            .and_then(norm)
    };
    let d = Palette::default();
    Some(Palette {
        bg: get("primary", "background")?,
        fg: get("primary", "foreground")?,
        accent: get("normal", "blue")
            .or_else(|| get("bright", "blue"))
            .unwrap_or(d.accent),
        green: get("normal", "green").unwrap_or(d.green),
        yellow: get("normal", "yellow").unwrap_or(d.yellow),
        red: get("normal", "red").unwrap_or(d.red),
    })
}

/// Palette of the current Omarchy theme, if there is one.
pub fn load(home: &Path) -> Option<Palette> {
    parse_alacritty(&std::fs::read_to_string(theme_file(home)).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alacritty_palette() {
        let p = parse_alacritty(
            "[colors.primary]\nbackground = '0x1A1B26'\nforeground = '#a9b1d6'\n[colors.normal]\nblue = '#7aa2f7'\n",
        )
        .unwrap();
        assert_eq!(p.bg, "#1a1b26");
        assert_eq!(p.accent, "#7aa2f7");
        assert_eq!(p.green, Palette::default().green);
        assert!(p.is_dark());
        assert!(parse_alacritty("nope").is_none());
    }
}
