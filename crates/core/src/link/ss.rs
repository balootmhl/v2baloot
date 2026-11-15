//! Shadowsocks links: SIP002 (`ss://base64(method:pass)@host:port#name`), plain userinfo
//! (`ss://method:pass@host:port`, common for 2022 ciphers) and legacy (`ss://base64(all)#name`).

use anyhow::{anyhow, bail, Context, Result};

use super::*;
use crate::util::b64encode_url_nopad;

pub fn parse(link: &str) -> Result<Server> {
    let body = &link[link.find("://").unwrap() + 3..];
    let (body, fragment) = match body.split_once('#') {
        Some((b, f)) => (b, Some(pct_decode(f))),
        None => (body, None),
    };
    // Standard base64 and raw 2022 keys may contain `/`, so the path/query (`/?plugin=...`) is
    // only searched for after the last `@`.
    let (userinfo, host_part) = match body.rfind('@') {
        Some(i) => (Some(&body[..i]), &body[i + 1..]),
        None => (None, body),
    };
    let (host_main, query) = match host_part.find(['/', '?']) {
        Some(i) => (&host_part[..i], Some(&host_part[i..])),
        None => (host_part, None),
    };
    if let Some(q) = query {
        if q.contains("plugin=") {
            bail!("shadowsocks plugins are not supported");
        }
    }

    let (userinfo, hostport) = match userinfo {
        Some(u) => (decode_userinfo(u)?, host_main.to_string()),
        None => {
            // Legacy: base64(method:password@host:port). Base64 never contains `?` or `@`.
            let encoded = body.split('?').next().unwrap_or(body);
            let all = b64decode_str(encoded).context("invalid ss link")?;
            let (u, h) = all
                .rsplit_once('@')
                .ok_or_else(|| anyhow!("invalid ss link"))?;
            (u.to_string(), h.trim_end_matches('/').to_string())
        }
    };
    let (method, password) = userinfo
        .split_once(':')
        .map(|(m, p)| (m.trim().to_ascii_lowercase(), p.to_string()))
        .ok_or_else(|| anyhow!("missing method:password"))?;
    let (address, port) = split_host_port(&hostport)?;
    let name = fragment
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| default_name(&address, port));
    Ok(Server::new(
        name,
        address,
        port,
        Protocol::Shadowsocks { method, password },
        Transport::default(),
        Security::default(),
    ))
}

fn decode_userinfo(u: &str) -> Result<String> {
    let decoded = pct_decode(u);
    if decoded.contains(':') {
        return Ok(decoded);
    }
    b64decode_str(&decoded)
        .filter(|s| s.contains(':'))
        .context("invalid ss userinfo")
}

fn split_host_port(s: &str) -> Result<(String, u16)> {
    let (host, port) = s.rsplit_once(':').ok_or_else(|| anyhow!("missing port"))?;
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = port.trim().parse().context("invalid port")?;
    if host.is_empty() {
        bail!("missing host");
    }
    Ok((host, port))
}

pub fn to_link(server: &Server) -> String {
    let Protocol::Shadowsocks { method, password } = &server.protocol else {
        unreachable!()
    };
    format!(
        "ss://{}@{}:{}#{}",
        b64encode_url_nopad(format!("{method}:{password}").as_bytes()),
        url_host(&server.address),
        server.port,
        encode_component(&server.name)
    )
}
