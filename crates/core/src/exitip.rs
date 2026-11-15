//! Exit IP / location lookup through the tunnel.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::model::ExitInfo;

// HTTPS only: over plain HTTP the proxy server (or anyone after it) could forge where we exit.
const IPWHO: &str = "https://ipwho.is/";
const IPINFO: &str = "https://ipinfo.io/json";

/// Look up the public IP. `proxy` (e.g. `socks5h://127.0.0.1:10808`) routes the request through
/// the tunnel in Proxy mode; in TUN mode a plain request already goes through it.
pub async fn lookup(proxy: Option<&str>) -> Result<ExitInfo> {
    let mut b = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("v2baloot");
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(p)?),
        None => b.no_proxy(),
    };
    let client = b.build()?;
    let info = match fetch(&client, IPWHO).await.and_then(|v| parse_ipwho(&v)) {
        Ok(info) => info,
        Err(first) => fetch(&client, IPINFO)
            .await
            .and_then(|v| parse_ipinfo(&v))
            .with_context(|| format!("ipwho.is: {first:#}; ipinfo.io"))?,
    };
    sanitize(info)
}

/// Untrusted response: require a real IP and keep the text fields short and printable.
fn sanitize(mut i: ExitInfo) -> Result<ExitInfo> {
    let ip: std::net::IpAddr = i.ip.trim().parse().context("lookup returned no valid IP")?;
    i.ip = ip.to_string();
    i.country = crate::util::clean_text(&i.country, 60);
    i.city = crate::util::clean_text(&i.city, 60);
    i.isp = crate::util::clean_text(&i.isp, 80);
    i.country_code = i
        .country_code
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(2)
        .collect();
    Ok(i)
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Value> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let text = crate::subscription::read_capped(resp, 64 * 1024).await?;
    Ok(serde_json::from_str(&text)?)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn parse_ipwho(v: &Value) -> Result<ExitInfo> {
    if v.get("success").and_then(Value::as_bool) != Some(true) {
        bail!("lookup failed");
    }
    let isp = v.get("connection").map(|c| s(c, "isp")).unwrap_or_default();
    Ok(ExitInfo {
        ip: s(v, "ip"),
        country: s(v, "country"),
        country_code: s(v, "country_code"),
        city: s(v, "city"),
        isp,
    })
}

pub fn parse_ipinfo(v: &Value) -> Result<ExitInfo> {
    if v.get("ip").is_none() {
        bail!("lookup failed");
    }
    let code = s(v, "country");
    Ok(ExitInfo {
        ip: s(v, "ip"),
        // ipinfo only gives the code; the flag next to it says enough.
        country: code.clone(),
        country_code: code,
        city: s(v, "city"),
        isp: s(v, "org"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_providers_and_sanitizes() {
        let b = parse_ipwho(&json!({"success":true,"ip":"1.2.3.4","country":"Japan","country_code":"JP","city":"Tokyo","connection":{"isp":"X"}})).unwrap();
        assert_eq!((b.country.as_str(), b.isp.as_str()), ("Japan", "X"));
        assert!(parse_ipwho(&json!({"success":false})).is_err());
        let c = parse_ipinfo(&json!({"ip":"5.6.7.8","country":"DE","city":"Berlin","org":"AS1 X"}))
            .unwrap();
        assert_eq!(c.country_code, "DE");
        assert!(sanitize(ExitInfo {
            ip: "not an ip".into(),
            ..Default::default()
        })
        .is_err());
        let d = sanitize(ExitInfo {
            ip: " 1.2.3.4".into(),
            city: "a\u{1b}[2Jb".into(),
            country_code: "S\u{7}G".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!((d.city.as_str(), d.country_code.as_str()), ("a [2Jb", "SG"));
    }
}
