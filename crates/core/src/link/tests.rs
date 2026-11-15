use super::*;
use crate::util::b64encode;

const UUID: &str = "b831381d-6324-4d53-ad4f-8cda48b30811";

fn roundtrip(s: &Server) {
    let link = to_link(s);
    let back = parse(&link).unwrap_or_else(|e| panic!("reparse {link}: {e:#}"));
    assert_eq!(&back, s, "round trip via {link}");
}

#[test]
fn vless_reality_vision() {
    let link = format!(
        "vless://{UUID}@203.0.113.7:443?encryption=none&flow=xtls-rprx-vision&security=reality\
         &sni=www.microsoft.com&fp=chrome&pbk=Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw&sid=6ba85179e30d4fc2\
         &spx=%2F&type=tcp&headerType=none#%F0%9F%87%B8%F0%9F%87%AC%20SG%20Reality"
    );
    let s = parse(&link).unwrap();
    assert_eq!(s.name, "🇸🇬 SG Reality");
    assert_eq!(s.address, "203.0.113.7");
    assert_eq!(s.port, 443);
    assert_eq!(
        s.protocol,
        Protocol::Vless {
            uuid: UUID.into(),
            flow: Some("xtls-rprx-vision".into()),
            encryption: "none".into()
        }
    );
    assert_eq!(s.transport.network, Network::Tcp);
    assert_eq!(s.transport.header_type, None);
    assert_eq!(s.security.kind, SecurityKind::Reality);
    assert_eq!(s.security.sni.as_deref(), Some("www.microsoft.com"));
    assert_eq!(s.security.short_id.as_deref(), Some("6ba85179e30d4fc2"));
    assert_eq!(s.security.spider_x.as_deref(), Some("/"));
    roundtrip(&s);
}

#[test]
fn vless_ws_tls_ipv6() {
    let link = format!(
        "vless://{UUID}@[2001:db8::1]:8443?type=ws&security=tls&path=%2Fray%3Fed%3D2048&host=cdn.example.com\
         &sni=cdn.example.com&alpn=h2%2Chttp%2F1.1&allowInsecure=1#ws"
    );
    let s = parse(&link).unwrap();
    assert_eq!(s.address, "2001:db8::1");
    assert_eq!(s.transport.network, Network::Ws);
    assert_eq!(s.transport.path.as_deref(), Some("/ray?ed=2048"));
    assert_eq!(s.security.alpn, vec!["h2", "http/1.1"]);
    assert!(s.security.allow_insecure);
    roundtrip(&s);
}

#[test]
fn vless_grpc_and_xhttp() {
    let s = parse(&format!(
        "vless://{UUID}@g.example.com:443?type=grpc&serviceName=gsvc&mode=multi&security=tls&sni=g.example.com"
    ))
    .unwrap();
    assert_eq!(s.transport.network, Network::Grpc);
    assert_eq!(s.transport.service_name.as_deref(), Some("gsvc"));
    assert_eq!(s.name, "g.example.com:443");
    roundtrip(&s);

    let s = parse(&format!(
        "vless://{UUID}@x.example.com:443?type=splithttp&path=%2Fx&mode=auto&security=tls"
    ))
    .unwrap();
    assert_eq!(s.transport.network, Network::Xhttp);
    roundtrip(&s);

    // HTTP/2 is migrated to xhttp stream-one
    let s = parse(&format!(
        "vless://{UUID}@x.example.com:443?type=h2&path=%2Fh&security=tls"
    ))
    .unwrap();
    assert_eq!(s.transport.network, Network::Xhttp);
    assert_eq!(s.transport.mode.as_deref(), Some("stream-one"));
}

#[test]
fn vmess_json() {
    let json = serde_json::json!({
        "v": "2", "ps": "JP vmess", "add": "jp.example.com", "port": 443, "id": UUID,
        "aid": "0", "scy": "auto", "net": "ws", "type": "none", "host": "jp.example.com",
        "path": "/vm", "tls": "tls", "sni": "jp.example.com", "alpn": "", "fp": "chrome"
    });
    let link = format!("vmess://{}", b64encode(json.to_string().as_bytes()));
    let s = parse(&link).unwrap();
    assert_eq!(s.name, "JP vmess");
    assert_eq!(s.port, 443);
    assert_eq!(s.transport.network, Network::Ws);
    assert_eq!(s.transport.path.as_deref(), Some("/vm"));
    assert_eq!(s.security.kind, SecurityKind::Tls);
    assert_eq!(s.security.fingerprint.as_deref(), Some("chrome"));
    roundtrip(&s);
}

#[test]
fn vmess_grpc_json_and_url_style() {
    let json = serde_json::json!({
        "add": "1.2.3.4", "port": "2053", "id": UUID, "aid": 0, "net": "grpc", "path": "svc", "type": "multi", "tls": "tls"
    });
    let s = parse(&format!(
        "vmess://{}",
        b64encode(json.to_string().as_bytes())
    ))
    .unwrap();
    assert_eq!(s.transport.service_name.as_deref(), Some("svc"));
    assert_eq!(s.transport.mode.as_deref(), Some("multi"));
    roundtrip(&s);

    let s = parse(&format!("vmess://{UUID}@1.2.3.4:80?type=tcp#plain")).unwrap();
    assert_eq!(s.name, "plain");
    assert!(matches!(s.protocol, Protocol::Vmess { .. }));
}

#[test]
fn trojan_defaults_to_tls() {
    let s = parse("trojan://p%40ss@t.example.com:443?sni=t.example.com#Trojan%20HK").unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Trojan {
            password: "p@ss".into()
        }
    );
    assert_eq!(s.security.kind, SecurityKind::Tls);
    assert_eq!(s.name, "Trojan HK");
    roundtrip(&s);

    let s = parse("trojan://pw@t.example.com:443?security=none&type=ws&path=%2Ft").unwrap();
    assert_eq!(s.security.kind, SecurityKind::None);
}

#[test]
fn shadowsocks_variants() {
    // SIP002
    let ui = crate::util::b64encode_url_nopad(b"chacha20-ietf-poly1305:secret");
    let s = parse(&format!("ss://{ui}@198.51.100.2:8388#SS%201")).unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Shadowsocks {
            method: "chacha20-ietf-poly1305".into(),
            password: "secret".into()
        }
    );
    assert_eq!(s.name, "SS 1");
    roundtrip(&s);

    // 2022 cipher with plain percent-encoded userinfo
    let s = parse("ss://2022-blake3-aes-128-gcm:YWJjZGVmZ2hpamtsbW5vcA%3D%3D@ss.example.com:443")
        .unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Shadowsocks {
            method: "2022-blake3-aes-128-gcm".into(),
            password: "YWJjZGVmZ2hpamtsbW5vcA==".into()
        }
    );

    // Legacy: whole thing base64
    let all = b64encode(b"aes-256-gcm:pw@10.0.0.1:443");
    let s = parse(&format!("ss://{all}#legacy")).unwrap();
    assert_eq!(s.address, "10.0.0.1");
    assert_eq!(s.port, 443);

    assert!(parse(&format!(
        "ss://{ui}@1.2.3.4:80/?plugin=obfs-local%3Bobfs%3Dhttp"
    ))
    .is_err());
}

#[test]
fn shadowsocks_slash_in_base64() {
    // Standard-alphabet base64 userinfo containing `/` and `+`.
    let ui = b64encode(b"aes-256-gcm:p?>>?/x");
    assert!(ui.contains('/') || ui.contains('+'), "{ui}");
    let s = parse(&format!("ss://{ui}@1.2.3.4:8388/?#std")).unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Shadowsocks {
            method: "aes-256-gcm".into(),
            password: "p?>>?/x".into()
        }
    );
    assert_eq!(s.name, "std");

    // Raw (unencoded) 2022 key with `/`, `+` and `=`.
    let s = parse("ss://2022-blake3-aes-256-gcm:YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=@1.2.3.4:8888#raw").unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Shadowsocks {
            method: "2022-blake3-aes-256-gcm".into(),
            password: "YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=".into()
        }
    );
    assert_eq!(s.port, 8888);

    // Legacy whole-link base64 containing `/`.
    let all = b64encode(b"aes-256-gcm:a??>/b@10.0.0.1:443");
    assert!(all.contains('/'), "{all}");
    let s = parse(&format!("ss://{all}#legacy")).unwrap();
    assert_eq!(s.address, "10.0.0.1");
    assert_eq!(
        s.protocol,
        Protocol::Shadowsocks {
            method: "aes-256-gcm".into(),
            password: "a??>/b".into()
        }
    );
}

#[test]
fn passwords_with_colon_and_vmess_tls_case() {
    let s = parse("trojan://pa:ss@t.example.com:443#t").unwrap();
    assert_eq!(
        s.protocol,
        Protocol::Trojan {
            password: "pa:ss".into()
        }
    );
    roundtrip(&s);

    let json = serde_json::json!({ "add": "1.2.3.4", "port": 443, "id": UUID, "net": "tcp", "tls": "TLS" });
    let s = parse(&format!(
        "vmess://{}",
        b64encode(json.to_string().as_bytes())
    ))
    .unwrap();
    assert_eq!(s.security.kind, SecurityKind::Tls);
}

#[test]
fn names_with_spaces_survive_parse_many() {
    let out = parse_many("trojan://pw@a.example.com:443#My Server One\ntrojan://pw@b.example.com:443#B trojan://pw@c.example.com:443#C");
    let names: Vec<_> = out.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["My Server One", "B", "C"]);

    let out =
        parse_many("trojan://pw@a.example.com:443#One A\rtrojan://pw@b.example.com:443#Two B\r");
    let names: Vec<_> = out.servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["One A", "Two B"]);
}

#[test]
fn parse_many_plain_and_base64() {
    let text = format!(
        "vless://{UUID}@a.example.com:443?security=tls#A\n\
         trojan://pw@b.example.com:443#B\n\
         hysteria2://x@c:1#C\n\
         vless://{UUID}@a.example.com:443?security=tls#A-duplicate\n"
    );
    let out = parse_many(&text);
    assert_eq!(out.servers.len(), 2);
    assert_eq!(out.errors.len(), 1);

    let out = parse_many(&b64encode(text.as_bytes()));
    assert_eq!(out.servers.len(), 2);
}

#[test]
fn id_ignores_name() {
    let a = parse(&format!("vless://{UUID}@a.example.com:443#One")).unwrap();
    let b = parse(&format!("vless://{UUID}@a.example.com:443#Two")).unwrap();
    assert_eq!(a.id, b.id);
}

#[test]
fn rejects_garbage() {
    assert!(parse("http://example.com").is_err());
    assert!(parse("vless://@host:1").is_err());
    assert!(parse("vless://uuid@host").is_err());
    assert!(parse("vmess://notbase64!!").is_err());
}
