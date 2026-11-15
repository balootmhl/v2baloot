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
