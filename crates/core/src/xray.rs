//! Xray-core JSON config generation.

use serde_json::{json, Map, Value};

use crate::model::*;

/// Name of the TUN interface created by Xray's `tun` inbound.
pub const TUN_NAME: &str = "v2tun0";
/// SO_MARK put on proxy traffic in TUN mode so policy routing can send it out the real interface.
pub const TUN_MARK: u32 = 255;

pub const PRIVATE_CIDRS: &[&str] = &[
    "0.0.0.0/8",
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "224.0.0.0/4",
    "255.255.255.255/32",
    "::1/128",
    "fc00::/7",
    "fe80::/10",
    "ff00::/8",
];

#[derive(Debug, Clone)]
pub struct XrayOptions {
    pub listen: String,
    pub socks_port: u16,
    pub http_port: u16,
    /// Local gRPC API port used for traffic stats. `None` disables the API.
    pub api_port: Option<u16>,
    pub log_level: String,
    pub dns: Vec<String>,
    pub bypass_private: bool,
    /// Enable Xray's TUN inbound and mark outbound traffic (requires CAP_NET_ADMIN).
    pub tun: bool,
    /// Open the local SOCKS/HTTP inbounds. Off for the root-side TUN instance so it doesn't offer
    /// an unauthenticated proxy to every local user.
    pub local_inbounds: bool,
    /// Close connections idle for this long (seconds). V2Box uses 30: dead flows after a network
    /// blip or sleep are dropped quickly so apps reconnect instead of hanging.
    pub conn_idle_secs: u32,
    /// Bind proxy traffic to this network interface (`sockopt.interface`).
    pub outbound_interface: Option<String>,
}

impl Default for XrayOptions {
    fn default() -> Self {
        XrayOptions {
            listen: "127.0.0.1".into(),
            socks_port: 10808,
            http_port: 10809,
            api_port: Some(10085),
            log_level: "warning".into(),
            dns: vec!["1.1.1.1".into(), "8.8.8.8".into()],
            bypass_private: true,
            tun: false,
            local_inbounds: true,
            conn_idle_secs: 30,
            outbound_interface: None,
        }
    }
}

fn sniffing() -> Value {
    json!({ "enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": true })
}

/// Full client config for one server.
pub fn build_config(server: &Server, opts: &XrayOptions) -> Value {
    let mark = opts.tun.then_some(TUN_MARK);
    let mut inbounds = vec![];
    if opts.local_inbounds {
        inbounds.extend([
            json!({
                "tag": "socks",
                "listen": opts.listen,
                "port": opts.socks_port,
                "protocol": "socks",
                "settings": { "auth": "noauth", "udp": true },
                "sniffing": sniffing(),
            }),
            json!({
                "tag": "http",
                "listen": opts.listen,
                "port": opts.http_port,
                "protocol": "http",
                "settings": {},
                "sniffing": sniffing(),
            }),
        ]);
    }
    if opts.tun {
        inbounds.push(json!({
            "tag": "tun",
            "protocol": "tun",
            "settings": { "name": TUN_NAME, "MTU": 1500, "userLevel": 0 },
            "sniffing": sniffing(),
        }));
    }

    let mut rules = vec![];
    let mut out = Map::new();
    if let Some(api_port) = opts.api_port {
        inbounds.push(json!({
            "tag": "api",
            "listen": "127.0.0.1",
            "port": api_port,
            "protocol": "dokodemo-door",
            "settings": { "address": "127.0.0.1" },
        }));
        rules.push(json!({ "type": "field", "inboundTag": ["api"], "outboundTag": "api" }));
        out.insert(
            "api".into(),
            json!({ "tag": "api", "services": ["StatsService"] }),
        );
        out.insert("stats".into(), json!({}));
    }
    // Same timeouts as V2Box: short idle/half-close timeouts so stalled flows die fast.
    let mut policy = json!({
        "levels": {
            "0": {
                "handshake": 4,
                "connIdle": opts.conn_idle_secs.clamp(5, 3600),
                "uplinkOnly": 1,
                "downlinkOnly": 1,
            }
        }
    });
    if opts.api_port.is_some() {
        policy["system"] = json!({ "statsOutboundUplink": true, "statsOutboundDownlink": true });
    }
    out.insert("policy".into(), policy);
    if opts.tun {
        // DNS queries arriving on the TUN go through Xray's DNS module (which uses the proxy).
        rules.push(json!({ "type": "field", "inboundTag": ["tun"], "port": "53", "network": "udp", "outboundTag": "dns-out" }));
    }
    if opts.bypass_private {
        rules.push(json!({ "type": "field", "ip": PRIVATE_CIDRS, "outboundTag": "direct" }));
    }

    let direct_stream = match mark {
        Some(m) => json!({ "sockopt": { "mark": m } }),
        None => json!({}),
    };
    let mut outbounds = vec![
        {
            let mut proxy = outbound(server, "proxy", mark);
            if let Some(iface) = opts.outbound_interface.as_deref().filter(|i| !i.is_empty()) {
                proxy["streamSettings"]["sockopt"]["interface"] = json!(iface);
            }
            proxy
        },
        json!({ "tag": "direct", "protocol": "freedom", "streamSettings": direct_stream }),
        json!({ "tag": "block", "protocol": "blackhole" }),
    ];
    if opts.tun {
        outbounds
            .push(json!({ "tag": "dns-out", "protocol": "dns", "streamSettings": direct_stream }));
    }

    // No access log: it would record every visited domain.
    out.insert(
        "log".into(),
        json!({ "loglevel": opts.log_level, "access": "none" }),
    );
    out.insert(
        "dns".into(),
        json!({
            "servers": opts.dns,
            // In TUN mode only hand out IPv4 so apps don't stall on IPv6 the server may not have.
            "queryStrategy": if opts.tun { "UseIPv4" } else { "UseIP" },
            "tag": "dns-internal",
        }),
    );
    // DNS module queries go through the proxy so they are not poisoned locally.
    rules.insert(
        0,
        json!({ "type": "field", "inboundTag": ["dns-internal"], "outboundTag": "proxy" }),
    );
    out.insert("inbounds".into(), Value::Array(inbounds));
    out.insert("outbounds".into(), Value::Array(outbounds));
    out.insert(
        "routing".into(),
        json!({ "domainStrategy": "AsIs", "rules": rules }),
    );
    Value::Object(out)
}

/// Config used for latency testing: one SOCKS inbound per server, each routed to its own outbound.
pub fn latency_config(servers: &[(&Server, u16)]) -> Value {
    let mut inbounds = vec![];
    let mut outbounds = vec![];
    let mut rules = vec![];
    for (i, (server, port)) in servers.iter().enumerate() {
        let in_tag = format!("in-{i}");
        let out_tag = format!("out-{i}");
        inbounds.push(json!({
            "tag": in_tag,
            "listen": "127.0.0.1",
            "port": port,
            "protocol": "socks",
            "settings": { "auth": "noauth", "udp": false },
        }));
        outbounds.push(outbound(server, &out_tag, None));
        rules.push(json!({ "type": "field", "inboundTag": [in_tag], "outboundTag": out_tag }));
    }
    json!({
        "log": { "loglevel": "none" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "routing": { "rules": rules },
    })
}

/// A single outbound object for `server`.
pub fn outbound(server: &Server, tag: &str, mark: Option<u32>) -> Value {
    let addr = &server.address;
    let port = server.port;
    let (protocol, settings) = match &server.protocol {
        Protocol::Vless {
            uuid,
            flow,
            encryption,
        } => {
            let mut user = json!({ "id": uuid, "encryption": encryption });
            if let Some(f) = flow {
                user["flow"] = json!(f);
            }
            (
                "vless",
                json!({ "vnext": [{ "address": addr, "port": port, "users": [user] }] }),
            )
        }
        Protocol::Vmess {
            uuid,
            alter_id,
            cipher,
        } => (
            "vmess",
            json!({ "vnext": [{ "address": addr, "port": port,
                "users": [{ "id": uuid, "alterId": alter_id, "security": cipher }] }] }),
        ),
        Protocol::Trojan { password } => (
            "trojan",
            json!({ "servers": [{ "address": addr, "port": port, "password": password }] }),
        ),
        Protocol::Shadowsocks { method, password } => (
            "shadowsocks",
            json!({ "servers": [{ "address": addr, "port": port, "method": method, "password": password }] }),
        ),
    };
    json!({
        "tag": tag,
        "protocol": protocol,
        "settings": settings,
        "streamSettings": stream_settings(server, mark),
    })
}

fn stream_settings(server: &Server, mark: Option<u32>) -> Value {
    let t = &server.transport;
    let s = &server.security;
    let mut ss = Map::new();
    ss.insert("network".into(), json!(t.network.name()));

    match t.network {
        Network::Tcp => {
            if t.header_type.as_deref() == Some("http") {
                let mut req = json!({ "path": [t.path.clone().unwrap_or_else(|| "/".into())] });
                if let Some(h) = &t.host {
                    req["headers"] =
                        json!({ "Host": h.split(',').map(str::trim).collect::<Vec<_>>() });
                }
                ss.insert(
                    "tcpSettings".into(),
                    json!({ "header": { "type": "http", "request": req } }),
                );
            }
        }
        Network::Ws => {
            let mut ws = json!({ "path": t.path.clone().unwrap_or_else(|| "/".into()) });
            if let Some(h) = &t.host {
                ws["host"] = json!(h);
            }
            ss.insert("wsSettings".into(), ws);
        }
        Network::HttpUpgrade => {
            let mut hu = json!({ "path": t.path.clone().unwrap_or_else(|| "/".into()) });
            if let Some(h) = &t.host {
                hu["host"] = json!(h);
            }
            ss.insert("httpupgradeSettings".into(), hu);
        }
        Network::Xhttp => {
            let mut xh = json!({ "path": t.path.clone().unwrap_or_else(|| "/".into()) });
            if let Some(h) = &t.host {
                xh["host"] = json!(h);
            }
            if let Some(m) = &t.mode {
                xh["mode"] = json!(m);
            }
            ss.insert("xhttpSettings".into(), xh);
        }
        Network::Grpc => {
            let mut g = json!({
                "serviceName": t.service_name.clone().or_else(|| t.path.clone()).unwrap_or_default(),
                "multiMode": t.mode.as_deref() == Some("multi"),
            });
            if let Some(a) = &t.authority {
                g["authority"] = json!(a);
            }
            ss.insert("grpcSettings".into(), g);
        }
    }

    ss.insert("security".into(), json!(s.kind.name()));
    let sni = s
        .sni
        .clone()
        .or_else(|| t.host.clone().filter(|h| !h.contains(',')));
    match s.kind {
        SecurityKind::None => {}
        SecurityKind::Tls => {
            let mut tls = Map::new();
            if let Some(n) = &sni {
                tls.insert("serverName".into(), json!(n));
            }
            if !s.alpn.is_empty() {
                tls.insert("alpn".into(), json!(s.alpn));
            }
            tls.insert(
                "fingerprint".into(),
                json!(s.fingerprint.clone().unwrap_or_else(|| "chrome".into())),
            );
            // Xray 26 removed `allowInsecure` (it now fails the whole config); certificate
            // pinning / name override are the supported replacements.
            if let Some(p) = &s.pinned_sha256 {
                tls.insert("pinnedPeerCertSha256".into(), json!(p));
            }
            if let Some(n) = &s.verify_name {
                tls.insert("verifyPeerCertByName".into(), json!(n));
            }
            ss.insert("tlsSettings".into(), Value::Object(tls));
        }
        SecurityKind::Reality => {
            let mut r = Map::new();
            if let Some(n) = &sni {
                r.insert("serverName".into(), json!(n));
            }
            r.insert(
                "fingerprint".into(),
                json!(s.fingerprint.clone().unwrap_or_else(|| "chrome".into())),
            );
            r.insert(
                "publicKey".into(),
                json!(s.public_key.clone().unwrap_or_default()),
            );
            r.insert(
                "shortId".into(),
                json!(s.short_id.clone().unwrap_or_default()),
            );
            if let Some(x) = &s.spider_x {
                r.insert("spiderX".into(), json!(x));
            }
            ss.insert("realitySettings".into(), Value::Object(r));
        }
    }

    if let Some(m) = mark {
        ss.insert("sockopt".into(), json!({ "mark": m }));
    }
    Value::Object(ss)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link;

    fn reality() -> Server {
        link::parse(
            "vless://b831381d-6324-4d53-ad4f-8cda48b30811@203.0.113.7:443?encryption=none&flow=xtls-rprx-vision\
             &security=reality&sni=www.microsoft.com&fp=chrome&pbk=Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw\
             &sid=6ba85179e30d4fc2&type=tcp#r",
        )
        .unwrap()
    }

    #[test]
    fn reality_outbound_shape() {
        let o = outbound(&reality(), "proxy", None);
        assert_eq!(o["protocol"], "vless");
        assert_eq!(
            o["settings"]["vnext"][0]["users"][0]["flow"],
            "xtls-rprx-vision"
        );
        assert_eq!(o["streamSettings"]["security"], "reality");
        assert_eq!(
            o["streamSettings"]["realitySettings"]["serverName"],
            "www.microsoft.com"
        );
        assert!(o["streamSettings"].get("sockopt").is_none());
    }

    #[test]
    fn tun_config_marks_traffic() {
        let cfg = build_config(
            &reality(),
            &XrayOptions {
                tun: true,
                local_inbounds: false,
                ..Default::default()
            },
        );
        let inbounds = cfg["inbounds"].as_array().unwrap();
        assert!(inbounds
            .iter()
            .all(|i| i["protocol"] != "socks" && i["protocol"] != "http"));
        assert!(inbounds
            .iter()
            .any(|i| i["protocol"] == "tun" && i["settings"]["name"] == TUN_NAME));
        let outbounds = cfg["outbounds"].as_array().unwrap();
        for o in outbounds.iter().filter(|o| o["protocol"] != "blackhole") {
            assert_eq!(o["streamSettings"]["sockopt"]["mark"], TUN_MARK, "{o}");
        }
    }

    #[test]
    fn policy_matches_v2box_and_interface_binding() {
        let cfg = build_config(
            &reality(),
            &XrayOptions {
                outbound_interface: Some("en0".into()),
                ..Default::default()
            },
        );
        let lvl = &cfg["policy"]["levels"]["0"];
        assert_eq!(lvl["handshake"], 4);
        assert_eq!(lvl["connIdle"], 30);
        assert_eq!(lvl["uplinkOnly"], 1);
        assert_eq!(lvl["downlinkOnly"], 1);
        assert_eq!(cfg["policy"]["system"]["statsOutboundUplink"], true);
        assert_eq!(
            cfg["outbounds"][0]["streamSettings"]["sockopt"]["interface"],
            "en0"
        );
    }

    #[test]
    fn ws_sni_falls_back_to_host() {
        let s = link::parse(
            "vless://u@1.2.3.4:443?type=ws&security=tls&host=cdn.example.com&path=%2Fw",
        )
        .unwrap();
        let o = outbound(&s, "proxy", None);
        assert_eq!(
            o["streamSettings"]["tlsSettings"]["serverName"],
            "cdn.example.com"
        );
        assert_eq!(o["streamSettings"]["wsSettings"]["host"], "cdn.example.com");
    }

    #[test]
    fn latency_config_routes_each_inbound() {
        let a = reality();
        let b = link::parse("trojan://pw@t.example.com:443").unwrap();
        let cfg = latency_config(&[(&a, 20001), (&b, 20002)]);
        assert_eq!(cfg["inbounds"][1]["port"], 20002);
        assert_eq!(cfg["routing"]["rules"][1]["outboundTag"], "out-1");
        assert_eq!(cfg["outbounds"][1]["protocol"], "trojan");
    }
}
