//! Subscription download and parsing.

use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::link::{self, ParseOutcome};
use crate::model::Usage;
use crate::util::b64decode_str;

#[derive(Debug, Default)]
pub struct FetchResult {
    pub outcome: ParseOutcome,
    pub usage: Option<Usage>,
    /// From the `profile-title` header, if the provider sends one.
    pub title: Option<String>,
}

/// Download a subscription. `proxy` (e.g. `socks5h://127.0.0.1:10808`) is used as a fallback when
/// the direct request fails — subscription hosts are often blocked locally.
pub async fn fetch(url: &str, user_agent: &str, proxy: Option<&str>) -> Result<FetchResult> {
    match fetch_once(url, user_agent, None).await {
        Ok(r) => Ok(r),
        Err(direct_err) => match proxy {
            Some(p) => fetch_once(url, user_agent, Some(p))
                .await
                .with_context(|| format!("direct: {direct_err:#}; via proxy")),
            None => Err(direct_err),
        },
    }
}

/// Largest subscription body we accept (real ones are a few hundred KB at most).
const MAX_BODY: usize = 8 * 1024 * 1024;

/// Redirects: at most 5, never from https down to http, never to the user's own machine.
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            return attempt.error("too many redirects");
        }
        let was_https = attempt
            .previous()
            .last()
            .is_some_and(|u| u.scheme() == "https");
        if was_https && attempt.url().scheme() != "https" {
            return attempt.error("refusing to follow an https → http redirect");
        }
        let local = match attempt.url().host() {
            Some(url::Host::Ipv4(ip)) => {
                ip.is_loopback() || ip.is_unspecified() || ip.is_link_local()
            }
            Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            None => true,
        };
        if local {
            return attempt.error("refusing to redirect to a local address");
        }
        attempt.follow()
    })
}

/// Read a response body, refusing anything larger than `max` bytes.
pub async fn read_capped(mut resp: reqwest::Response, max: usize) -> Result<String> {
    if resp.content_length().is_some_and(|n| n as usize > max) {
        bail!("response too large");
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len() + chunk.len() > max {
            bail!("response too large (over {} MB)", max / (1024 * 1024));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn fetch_once(url: &str, user_agent: &str, proxy: Option<&str>) -> Result<FetchResult> {
    let mut b = reqwest::Client::builder()
        .user_agent(user_agent)
        .redirect(redirect_policy())
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8));
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(p)?),
        None => b.no_proxy(),
    };
    let resp = b.build()?.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    let usage = resp
        .headers()
        .get("subscription-userinfo")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_userinfo);
    let title = resp
        .headers()
        .get("profile-title")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_title);
    let body = read_capped(resp, MAX_BODY).await?;
    // Parsing thousands of links is CPU work; keep it off the async workers.
    let mut outcome = tokio::task::spawn_blocking(move || parse_body(&body)).await?;
    // A subscription must not point the client at this machine (loopback, link-local…).
    let before = outcome.servers.len();
    outcome.servers.retain(|s| !s.is_local_target());
    if outcome.servers.len() < before {
        outcome.errors.push(format!(
            "{} server(s) pointing at local addresses were ignored",
            before - outcome.servers.len()
        ));
    }
    if outcome.servers.is_empty() {
        if outcome.errors.is_empty() {
            bail!("no supported servers in subscription");
        }
        bail!(
            "no supported servers in subscription ({})",
            outcome.errors[0]
        );
    }
    Ok(FetchResult {
        outcome,
        usage,
        title,
    })
}

/// Body is either base64 of newline-separated links or the links themselves.
pub fn parse_body(body: &str) -> ParseOutcome {
    let trimmed = body.trim();
    if let Some(decoded) = b64decode_str(trimmed) {
        if decoded.contains("://") {
            return link::parse_many(&decoded);
        }
    }
    link::parse_many(trimmed)
}

/// `upload=123; download=456; total=789; expire=1700000000`
pub fn parse_userinfo(h: &str) -> Option<Usage> {
    let mut u = Usage::default();
    let mut any = false;
    for part in h.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let v = v.trim();
        // Some panels send floats.
        let n = v
            .parse::<f64>()
            .ok()
            .map(|f| f.max(0.0) as u64)
            .unwrap_or(0);
        match k.trim().to_ascii_lowercase().as_str() {
            "upload" => u.upload = n,
            "download" => u.download = n,
            "total" => u.total = n,
            "expire" => u.expire = n as i64,
            _ => continue,
        }
        any = true;
    }
    any.then_some(u)
}

fn parse_title(h: &str) -> Option<String> {
    let t = match h.strip_prefix("base64:") {
        Some(b) => b64decode_str(b)?,
        None => h.to_string(),
    };
    Some(crate::util::clean_text(&t, 60)).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::b64encode;

    #[test]
    fn userinfo() {
        let u = parse_userinfo("upload=1024; download=2048; total=10737418240; expire=1700000000")
            .unwrap();
        assert_eq!(u.upload, 1024);
        assert_eq!(u.total, 10737418240);
        assert_eq!(u.expire, 1700000000);
        assert!(parse_userinfo("garbage").is_none());
    }

    #[test]
    fn body_base64_and_plain() {
        let links = "trojan://a@a.example.com:443#a\r\ntrojan://b@b.example.com:443#b\r\n";
        assert_eq!(parse_body(&b64encode(links.as_bytes())).servers.len(), 2);
        assert_eq!(parse_body(links).servers.len(), 2);
    }

    #[test]
    fn local_targets_flagged() {
        for (l, local) in [
            ("trojan://p@127.0.0.1:443", true),
            ("trojan://p@localhost:443", true),
            ("trojan://p@[::1]:443", true),
            ("trojan://p@169.254.1.1:443", true),
            ("trojan://p@0.0.0.0:443", true),
            ("trojan://p@192.168.1.10:443", false),
            ("trojan://p@example.com:443", false),
        ] {
            assert_eq!(
                crate::link::parse(l).unwrap().is_local_target(),
                local,
                "{l}"
            );
        }
    }

    #[test]
    fn title() {
        assert_eq!(parse_title("base64:TXkgVlBO").as_deref(), Some("My VPN"));
        assert_eq!(parse_title("Plain").as_deref(), Some("Plain"));
    }
}
