//! Validates generated configs with the real `xray run -test`. Skipped when xray is not installed.

use std::process::Command;

use v2baloot_core::link;
use v2baloot_core::xray::{build_config, latency_config, XrayOptions};

const LINKS: &[&str] = &[
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@203.0.113.7:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=www.microsoft.com&fp=chrome&pbk=Z84J2IelR9ch3k8VtlVhhs5ycBUlXA7wHBWcBrjqnAw&sid=6ba85179e30d4fc2&type=tcp#reality",
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@cdn.example.com:443?type=ws&security=tls&path=%2Fws&host=cdn.example.com#ws",
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@g.example.com:443?type=grpc&serviceName=g&mode=multi&security=tls#grpc",
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@x.example.com:443?type=xhttp&path=%2Fx&mode=auto&security=tls#xhttp",
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@x.example.com:443?type=h2&path=%2Fh&security=tls#h2",
    "vless://b831381d-6324-4d53-ad4f-8cda48b30811@x.example.com:443?type=httpupgrade&security=tls&path=%2Fu&host=x.example.com#hu",
    "vmess://b831381d-6324-4d53-ad4f-8cda48b30811@x.example.com:80?type=tcp&headerType=http&path=%2F&host=a.com,b.com#tcphttp",
    "trojan://pw@t.example.com:443?sni=t.example.com#trojan",
    "trojan://pw@t.example.com:443?sni=t.example.com&allowInsecure=1#trojan-insecure",
    "trojan://pw@t.example.com:443?sni=t.example.com&pcs=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855&vcn=real.example.com#trojan-pinned",
    "ss://Y2hhY2hhMjAtaWV0Zi1wb2x5MTMwNTpzZWNyZXQ@198.51.100.2:8388#ss",
    "ss://2022-blake3-aes-128-gcm:YWJjZGVmZ2hpamtsbW5vcA%3D%3D@ss.example.com:443#ss2022",
];

fn xray_ok(cfg: &serde_json::Value) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("v2baloot-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "{:x}.json",
        v2baloot_core::util::fnv1a64(cfg.to_string().as_bytes())
    ));
    std::fs::write(&path, cfg.to_string()).unwrap();
    let out = Command::new("xray")
        .args(["run", "-test", "-c"])
        .arg(&path)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() && text.contains("Configuration OK") {
        Ok(())
    } else {
        Err(text)
    }
}

#[test]
fn xray_accepts_generated_configs() {
    if Command::new("xray").arg("version").output().is_err() {
        eprintln!("xray not installed; skipping");
        return;
    }
    let servers: Vec<_> = LINKS.iter().map(|l| link::parse(l).unwrap()).collect();
    for s in &servers {
        for tun in [false, true] {
            let cfg = build_config(
                s,
                &XrayOptions {
                    tun,
                    local_inbounds: !tun,
                    ..Default::default()
                },
            );
            xray_ok(&cfg)
                .unwrap_or_else(|e| panic!("{} (tun={tun}) rejected:\n{e}\n{cfg:#}", s.name));
        }
    }
    // Interface binding (used to bypass another VPN, or pin to wlan0).
    let bound = build_config(
        &servers[0],
        &XrayOptions {
            outbound_interface: Some("en0".into()),
            conn_idle_secs: 30,
            ..Default::default()
        },
    );
    xray_ok(&bound).unwrap_or_else(|e| panic!("interface-bound config rejected:\n{e}"));
    let pairs: Vec<_> = servers
        .iter()
        .enumerate()
        .map(|(i, s)| (s, 30000 + i as u16))
        .collect();
    xray_ok(&latency_config(&pairs)).unwrap();
}
