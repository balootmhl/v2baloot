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

async fn fetch_once(url: &str, user_agent: &str, proxy: Option<&str>) -> Result<FetchResult> {
    let mut b = reqwest::Client::builder()
        .user_agent(user_agent)
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
    let body = resp.text().await?;
    let outcome = parse_body(&body);
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
    Some(t.trim().to_string()).filter(|t| !t.is_empty())
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
    fn title() {
        assert_eq!(parse_title("base64:TXkgVlBO").as_deref(), Some("My VPN"));
        assert_eq!(parse_title("Plain").as_deref(), Some("Plain"));
    }
}
