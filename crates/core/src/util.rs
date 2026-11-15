//! Small helpers.

use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);
const STD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);
const URL: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);

/// Decode base64 the way subscription providers produce it: standard or URL-safe alphabet,
/// with or without padding, possibly wrapped across lines.
pub fn b64decode(input: &str) -> Option<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        return None;
    }
    STD.decode(&cleaned).or_else(|_| URL.decode(&cleaned)).ok()
}

pub fn b64decode_str(input: &str) -> Option<String> {
    b64decode(input).and_then(|b| String::from_utf8(b).ok())
}

pub fn b64encode(input: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(input)
}

pub fn b64encode_url_nopad(input: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(input)
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn human_speed(bps: u64) -> String {
    format!("{}/s", human_bytes(bps))
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Format a unix timestamp as `YYYY-MM-DD` (UTC) without pulling in a date crate.
pub fn format_date(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_variants() {
        assert_eq!(b64decode_str("aGVsbG8=").unwrap(), "hello");
        assert_eq!(b64decode_str("aGVsbG8").unwrap(), "hello");
        assert_eq!(b64decode_str("aGVs\nbG8=\n").unwrap(), "hello");
        // URL-safe alphabet
        assert!(b64decode(">>>?").is_none());
        assert!(b64decode_str(&b64encode_url_nopad(b"\xfb\xff?")).is_none());
        assert_eq!(
            b64decode(&b64encode_url_nopad(b"\xfb\xff")).unwrap(),
            b"\xfb\xff"
        );
    }

    #[test]
    fn dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_700_000_000), "2023-11-14");
    }

    #[test]
    fn bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }
}

/// Make text from untrusted sources (server and subscription names, provider headers, error
/// messages) safe to show anywhere — terminals, notifications, menus: control characters (C0,
/// DEL, C1, which carry terminal escape sequences) become spaces, bidi overrides that can
/// reorder what is displayed are dropped, and the length is capped (in characters).
pub fn clean_text(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_chars * 4));
    let mut n = 0;
    for c in s.chars() {
        if n >= max_chars {
            out.push('…');
            break;
        }
        if c.is_control() {
            if !out.ends_with(' ') {
                out.push(' ');
                n += 1;
            }
            continue;
        }
        if matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
        {
            continue;
        }
        out.push(c);
        n += 1;
    }
    out.trim().to_string()
}

/// 🇸🇬 from "SG".
pub fn country_flag(code: &str) -> Option<String> {
    let code = code.trim();
    if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(
        code.to_ascii_uppercase()
            .chars()
            .map(|c| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32)).unwrap())
            .collect(),
    )
}

fn is_regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// A flag emoji already present in a server name ("🇯🇵 Tokyo 2" → "🇯🇵").
pub fn flag_from_name(name: &str) -> Option<String> {
    let chars: Vec<char> = name.chars().collect();
    chars
        .windows(2)
        .find(|w| is_regional(w[0]) && is_regional(w[1]))
        .map(|w| w.iter().collect())
}

/// The name without its flag emoji and surrounding separators.
pub fn strip_flag(name: &str) -> String {
    let stripped: String = name.chars().filter(|c| !is_regional(*c)).collect();
    stripped
        .trim_matches(|c: char| c.is_whitespace() || c == '|' || c == '-')
        .to_string()
}

#[cfg(test)]
mod flag_tests {
    use super::*;

    #[test]
    fn clean_text_strips_escapes_and_bidi() {
        assert_eq!(
            clean_text("a\u{1b}]52;c;ZXZpbA==\u{7}b", 100),
            "a ]52;c;ZXZpbA== b"
        );
        assert_eq!(clean_text("x\u{202E}gnp.exe", 100), "xgnp.exe");
        assert_eq!(clean_text("\u{9b}31m red", 100), "31m red");
        assert_eq!(clean_text("abcdef", 3), "abc…");
        assert_eq!(clean_text("🇸🇬 Singapore", 100), "🇸🇬 Singapore");
    }

    #[test]
    fn flags() {
        assert_eq!(country_flag("sg").as_deref(), Some("🇸🇬"));
        assert_eq!(country_flag("SGP"), None);
        assert_eq!(flag_from_name("🇯🇵 Tokyo 2").as_deref(), Some("🇯🇵"));
        assert_eq!(flag_from_name("Tokyo"), None);
        assert_eq!(strip_flag("🇯🇵 | Tokyo 2"), "Tokyo 2");
    }
}

/// The calling process's uid.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}
