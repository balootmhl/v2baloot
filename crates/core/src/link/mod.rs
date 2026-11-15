//! Share-link parsing and generation (`vless://`, `vmess://`, `trojan://`, `ss://`).

mod ss;
mod trojan;
mod vless;
mod vmess;

use std::collections::HashMap;

use anyhow::{anyhow, bail, Context, Result};
use percent_encoding::percent_decode_str;
use url::{form_urlencoded, Host, Url};

use crate::model::*;
use crate::util::b64decode_str;

/// Parse a single share link.
pub fn parse(link: &str) -> Result<Server> {
    let link = link.trim();
    let scheme = link
        .split_once("://")
        .map(|(s, _)| s.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("not a share link"))?;
    match scheme.as_str() {
        "vless" => vless::parse(link),
        "vmess" => vmess::parse(link),
        "trojan" => trojan::parse(link),
        "ss" => ss::parse(link),
        other => bail!("unsupported protocol `{other}`"),
    }
}

/// Generate a share link for a server.
pub fn to_link(server: &Server) -> String {
    match &server.protocol {
        Protocol::Vless { .. } => vless::to_link(server),
        Protocol::Vmess { .. } => vmess::to_link(server),
        Protocol::Trojan { .. } => trojan::to_link(server),
        Protocol::Shadowsocks { .. } => ss::to_link(server),
    }
}

/// Result of parsing a blob of text that may contain many links (or be base64 of many links).
#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub servers: Vec<Server>,
    pub errors: Vec<String>,
}

/// Parse any number of links separated by whitespace/newlines. If the text holds no `://` at all
/// it is treated as base64 (raw subscription content).
/// Most servers accepted from one blob of links (a subscription or a paste).
pub const MAX_SERVERS: usize = 5000;
/// Longer lines are not share links; skipping them bounds the work per line.
const MAX_LINE: usize = 16 * 1024;
const MAX_ERRORS: usize = 50;

/// `scheme://host` of a link for error messages — never credentials or the rest of the link.
fn link_label(line: &str) -> String {
    let (scheme, rest) = line.split_once("://").unwrap_or(("?", ""));
    let host = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(
        if scheme.eq_ignore_ascii_case("vmess") || scheme.eq_ignore_ascii_case("ss") {
            "…"
        } else {
            rest
        },
    );
    let host: String = host
        .chars()
        .take_while(|c| !matches!(c, '/' | '?' | '#'))
        .take(60)
        .collect();
    crate::util::clean_text(&format!("{}://{host}", scheme.to_ascii_lowercase()), 80)
}

pub fn parse_many(text: &str) -> ParseOutcome {
    let decoded;
    let text = if !text.contains("://") {
        match b64decode_str(text) {
            Some(d) if d.contains("://") => {
                decoded = d;
                decoded.as_str()
            }
            _ => text,
        }
    } else {
        text
    };

    let mut out = ParseOutcome::default();
    // Split by lines first so names with literal spaces after `#` survive; only split a line
    // on whitespace when it holds several links.
    // Lone CRs (e.g. pasted from a terminal) count as line breaks too.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let text = normalized.as_str();
    let pieces = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .flat_map(|l| {
            if l.matches("://").count() > 1 {
                l.split_whitespace().collect::<Vec<_>>()
            } else {
                vec![l]
            }
        });
    let mut seen = std::collections::HashSet::new();
    let push_err = |out: &mut ParseOutcome, e: String| {
        if out.errors.len() < MAX_ERRORS {
            out.errors.push(crate::util::clean_text(&e, 300));
        }
    };
    for line in pieces {
        if !line.contains("://") {
            continue;
        }
        if line.len() > MAX_LINE {
            push_err(
                &mut out,
                format!("{}: line too long, skipped", link_label(line)),
            );
            continue;
        }
        if out.servers.len() >= MAX_SERVERS {
            push_err(
                &mut out,
                format!("more than {MAX_SERVERS} servers; the rest were ignored"),
            );
            break;
        }
        match parse(line) {
            Ok(s) => {
                if seen.insert(s.id.clone()) {
                    out.servers.push(s);
                }
            }
            Err(e) => push_err(&mut out, format!("{}: {e:#}", link_label(line))),
        }
    }
    out
}

// ---------- shared helpers for URL-style links (vless, trojan, vmess-AEAD) ----------

pub(crate) fn pct_decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Full userinfo: the `url` crate splits it at the first `:`, but Trojan passwords may contain one.
pub(crate) fn userinfo(url: &Url) -> String {
    match url.password() {
        Some(p) => format!("{}:{}", pct_decode(url.username()), pct_decode(p)),
        None => pct_decode(url.username()),
    }
}

pub(crate) fn host_port(url: &Url) -> Result<(String, u16)> {
    let host = match url.host().context("missing host")? {
        Host::Domain(d) => pct_decode(d),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    };
    let port = url.port().context("missing port")?;
    Ok((host, port))
}

pub(crate) fn query_map(url: &Url) -> HashMap<String, String> {
    url.query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

pub(crate) fn fragment_name(url: &Url, address: &str, port: u16) -> String {
    url.fragment()
        .map(pct_decode)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_name(address, port))
}

pub(crate) fn default_name(address: &str, port: u16) -> String {
    format!("{address}:{port}")
}

fn nonempty(q: &HashMap<String, String>, key: &str) -> Option<String> {
    q.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub(crate) fn transport_from_query(q: &HashMap<String, String>) -> Result<Transport> {
    let net_raw = q.get("type").map(String::as_str).unwrap_or("tcp");
    let network =
        Network::parse(net_raw).ok_or_else(|| anyhow!("unsupported transport `{net_raw}`"))?;
    let mut t = Transport {
        network,
        path: nonempty(q, "path"),
        host: nonempty(q, "host"),
        service_name: nonempty(q, "serviceName"),
        authority: nonempty(q, "authority"),
        header_type: nonempty(q, "headerType").filter(|h| h != "none"),
        mode: nonempty(q, "mode"),
    };
    if matches!(net_raw, "h2" | "http") && t.mode.is_none() {
        t.mode = Some("stream-one".into());
    }
    Ok(t)
}

pub(crate) fn security_from_query(
    q: &HashMap<String, String>,
    default: SecurityKind,
) -> Result<Security> {
    let kind = match q.get("security").map(|s| s.trim().to_ascii_lowercase()) {
        None => default,
        Some(s) => match s.as_str() {
            "" => default,
            "none" => SecurityKind::None,
            "tls" | "xtls" => SecurityKind::Tls,
            "reality" => SecurityKind::Reality,
            other => bail!("unsupported security `{other}`"),
        },
    };
    let truthy = |k: &str| {
        q.get(k)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    };
    Ok(Security {
        kind,
        sni: nonempty(q, "sni").or_else(|| nonempty(q, "peer")),
        fingerprint: nonempty(q, "fp"),
        alpn: nonempty(q, "alpn")
            .map(|a| {
                a.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        allow_insecure: truthy("allowInsecure") || truthy("insecure"),
        pinned_sha256: nonempty(q, "pcs"),
        verify_name: nonempty(q, "vcn"),
        public_key: nonempty(q, "pbk"),
        short_id: nonempty(q, "sid"),
        spider_x: nonempty(q, "spx"),
    })
}

/// Build the query string shared by vless/trojan links.
pub(crate) fn common_query(server: &Server, extra: &[(&str, String)]) -> String {
    let mut ser = form_urlencoded::Serializer::new(String::new());
    for (k, v) in extra {
        ser.append_pair(k, v);
    }
    let t = &server.transport;
    ser.append_pair("type", t.network.name());
    let opt = |ser: &mut form_urlencoded::Serializer<String>, k: &str, v: &Option<String>| {
        if let Some(v) = v {
            ser.append_pair(k, v);
        }
    };
    opt(&mut ser, "path", &t.path);
    opt(&mut ser, "host", &t.host);
    opt(&mut ser, "serviceName", &t.service_name);
    opt(&mut ser, "authority", &t.authority);
    opt(&mut ser, "headerType", &t.header_type);
    opt(&mut ser, "mode", &t.mode);

    let s = &server.security;
    ser.append_pair("security", s.kind.name());
    opt(&mut ser, "sni", &s.sni);
    opt(&mut ser, "fp", &s.fingerprint);
    if !s.alpn.is_empty() {
        ser.append_pair("alpn", &s.alpn.join(","));
    }
    if s.allow_insecure {
        ser.append_pair("allowInsecure", "1");
    }
    opt(&mut ser, "pcs", &s.pinned_sha256);
    opt(&mut ser, "vcn", &s.verify_name);
    opt(&mut ser, "pbk", &s.public_key);
    opt(&mut ser, "sid", &s.short_id);
    opt(&mut ser, "spx", &s.spider_x);
    ser.finish()
}

pub(crate) fn url_host(address: &str) -> String {
    if address.contains(':') && !address.starts_with('[') {
        format!("[{address}]")
    } else {
        address.to_string()
    }
}

pub(crate) fn encode_component(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests;
