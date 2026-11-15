//! Exit IP / location lookup through the tunnel.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::model::ExitInfo;

const IP_API: &str = "http://ip-api.com/json/?fields=status,query,country,countryCode,city,isp";
const IPWHO: &str = "https://ipwho.is/";

/// Look up the public IP. `proxy` (e.g. `socks5h://127.0.0.1:10808`) routes the request through
/// the tunnel in Proxy mode; in TUN mode a plain request already goes through it.
pub async fn lookup(proxy: Option<&str>) -> Result<ExitInfo> {
    let mut b = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .user_agent("v2baloot");
    b = match proxy {
        Some(p) => b.proxy(reqwest::Proxy::all(p)?),
        None => b.no_proxy(),
    };
    let client = b.build()?;
    match fetch(&client, IP_API).await.and_then(|v| parse_ip_api(&v)) {
        Ok(info) => Ok(info),
        Err(first) => fetch(&client, IPWHO)
            .await
            .and_then(|v| parse_ipwho(&v))
            .with_context(|| format!("ip-api: {first:#}; ipwho.is")),
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Value> {
    let text = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&text)?)
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn parse_ip_api(v: &Value) -> Result<ExitInfo> {
    if s(v, "status") != "success" {
        bail!("lookup failed");
    }
    Ok(ExitInfo {
        ip: s(v, "query"),
        country: s(v, "country"),
        country_code: s(v, "countryCode"),
        city: s(v, "city"),
        isp: s(v, "isp"),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_providers() {
        let a = parse_ip_api(&json!({"status":"success","query":"52.221.1.2","country":"Singapore","countryCode":"SG","city":"Singapore","isp":"Amazon"})).unwrap();
        assert_eq!(
            (a.ip.as_str(), a.country_code.as_str()),
            ("52.221.1.2", "SG")
        );
        assert!(parse_ip_api(&json!({"status":"fail"})).is_err());
        let b = parse_ipwho(&json!({"success":true,"ip":"1.2.3.4","country":"Japan","country_code":"JP","city":"Tokyo","connection":{"isp":"X"}})).unwrap();
        assert_eq!((b.country.as_str(), b.isp.as_str()), ("Japan", "X"));
    }
}
