//! `vmess://base64(json)` (v2rayN format) plus the URL-style AEAD variant.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use url::Url;

use super::*;
use crate::util::b64encode;

fn field(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub fn parse(link: &str) -> Result<Server> {
    let body = &link[link.find("://").unwrap() + 3..];
    let (payload, fragment) = match body.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (body, None),
    };
    match b64decode_str(payload) {
        Some(json_text) if json_text.trim_start().starts_with('{') => {
            parse_json(&json_text, fragment)
        }
        _ if body.contains('@') => parse_url(link),
        _ => Err(anyhow!("invalid vmess link")),
    }
}

fn parse_json(text: &str, fragment: Option<&str>) -> Result<Server> {
    let v: Value = serde_json::from_str(text).context("invalid vmess JSON")?;
    let address = field(&v, "add").context("missing address")?;
    let port: u16 = field(&v, "port")
        .context("missing port")?
        .parse()
        .context("invalid port")?;
    let uuid = field(&v, "id").context("missing id")?;
    let alter_id = field(&v, "aid").and_then(|a| a.parse().ok()).unwrap_or(0);
    let cipher = field(&v, "scy").unwrap_or_else(|| "auto".into());

    let net_raw = field(&v, "net").unwrap_or_else(|| "tcp".into());
    let network =
        Network::parse(&net_raw).ok_or_else(|| anyhow!("unsupported transport `{net_raw}`"))?;
    let path = field(&v, "path");
    let host = field(&v, "host");
    let header_type = field(&v, "type").filter(|t| t != "none");
    let mut transport = Transport {
        network,
        ..Default::default()
    };
    match network {
        Network::Grpc => {
            // v2rayN puts the gRPC service name in `path`, and `type` holds gun/multi.
            transport.service_name = path;
            transport.authority = host;
            transport.mode = header_type;
        }
        Network::Tcp => {
            transport.path = path;
            transport.host = host;
            transport.header_type = header_type;
        }
        _ => {
            transport.path = path;
            transport.host = host;
            if network == Network::Xhttp {
                transport.mode =
                    field(&v, "mode").or(if matches!(net_raw.as_str(), "h2" | "http") {
                        Some("stream-one".into())
                    } else {
                        None
                    });
            }
        }
    }

    let kind = match field(&v, "tls").map(|t| t.to_ascii_lowercase()).as_deref() {
        Some("tls") => SecurityKind::Tls,
        Some("reality") => SecurityKind::Reality,
        _ => SecurityKind::None,
    };
    let security = Security {
        kind,
        sni: field(&v, "sni"),
        fingerprint: field(&v, "fp"),
        alpn: field(&v, "alpn")
            .map(|a| {
                a.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        allow_insecure: matches!(field(&v, "allowInsecure").as_deref(), Some("1" | "true")),
        pinned_sha256: field(&v, "pcs"),
        verify_name: field(&v, "vcn"),
        public_key: field(&v, "pbk"),
        short_id: field(&v, "sid"),
        spider_x: field(&v, "spx"),
    };

    let name = field(&v, "ps")
        .or_else(|| fragment.map(pct_decode).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| default_name(&address, port));
    Ok(Server::new(
        name,
        address,
        port,
        Protocol::Vmess {
            uuid,
            alter_id,
            cipher,
        },
        transport,
        security,
    ))
}

/// `vmess://uuid@host:port?type=ws&security=tls&...#name`
fn parse_url(link: &str) -> Result<Server> {
    let url = Url::parse(link).context("invalid vmess link")?;
    let uuid = userinfo(&url);
    let (address, port) = host_port(&url)?;
    let q = query_map(&url);
    let transport = transport_from_query(&q)?;
    let security = security_from_query(&q, SecurityKind::None)?;
    let cipher = nonempty(&q, "encryption").unwrap_or_else(|| "auto".into());
    let name = fragment_name(&url, &address, port);
    Ok(Server::new(
        name,
        address,
        port,
        Protocol::Vmess {
            uuid,
            alter_id: 0,
            cipher,
        },
        transport,
        security,
    ))
}

pub fn to_link(server: &Server) -> String {
    let Protocol::Vmess {
        uuid,
        alter_id,
        cipher,
    } = &server.protocol
    else {
        unreachable!()
    };
    let t = &server.transport;
    let s = &server.security;
    let (path, host, typ) = match t.network {
        Network::Grpc => (t.service_name.clone(), t.authority.clone(), t.mode.clone()),
        Network::Tcp => (t.path.clone(), t.host.clone(), t.header_type.clone()),
        _ => (t.path.clone(), t.host.clone(), None),
    };
    let mut v = json!({
        "v": "2",
        "ps": server.name,
        "add": server.address,
        "port": server.port.to_string(),
        "id": uuid,
        "aid": alter_id.to_string(),
        "scy": cipher,
        "net": t.network.name(),
        "type": typ.unwrap_or_else(|| "none".into()),
        "host": host.unwrap_or_default(),
        "path": path.unwrap_or_default(),
        "tls": if s.kind == SecurityKind::None { "" } else { s.kind.name() },
        "sni": s.sni.clone().unwrap_or_default(),
        "alpn": s.alpn.join(","),
        "fp": s.fingerprint.clone().unwrap_or_default(),
    });
    if t.network == Network::Xhttp {
        if let Some(m) = &t.mode {
            v["mode"] = json!(m);
        }
    }
    if s.kind == SecurityKind::Reality {
        v["pbk"] = json!(s.public_key.clone().unwrap_or_default());
        v["sid"] = json!(s.short_id.clone().unwrap_or_default());
        v["spx"] = json!(s.spider_x.clone().unwrap_or_default());
    }
    for (k, val) in [("pcs", &s.pinned_sha256), ("vcn", &s.verify_name)] {
        if let Some(val) = val {
            v[k] = json!(val);
        }
    }

    format!("vmess://{}", b64encode(v.to_string().as_bytes()))
}
