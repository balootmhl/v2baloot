//! Data model: servers, subscriptions, connection status.

use serde::{Deserialize, Serialize};

use crate::util::fnv1a64;

fn default_none() -> String {
    "none".into()
}
fn default_auto() -> String {
    "auto".into()
}

/// Outbound protocol plus its credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Protocol {
    Vless {
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        flow: Option<String>,
        #[serde(default = "default_none")]
        encryption: String,
    },
    Vmess {
        uuid: String,
        #[serde(default)]
        alter_id: u32,
        #[serde(default = "default_auto")]
        cipher: String,
    },
    Trojan {
        password: String,
    },
    Shadowsocks {
        method: String,
        password: String,
    },
}

impl Protocol {
    pub fn name(&self) -> &'static str {
        match self {
            Protocol::Vless { .. } => "vless",
            Protocol::Vmess { .. } => "vmess",
            Protocol::Trojan { .. } => "trojan",
            Protocol::Shadowsocks { .. } => "ss",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    #[default]
    Tcp,
    Ws,
    Grpc,
    HttpUpgrade,
    Xhttp,
}

impl Network {
    pub fn name(&self) -> &'static str {
        match self {
            Network::Tcp => "tcp",
            Network::Ws => "ws",
            Network::Grpc => "grpc",
            Network::HttpUpgrade => "httpupgrade",
            Network::Xhttp => "xhttp",
        }
    }

    pub fn parse(s: &str) -> Option<Network> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "tcp" | "raw" => Network::Tcp,
            "ws" | "websocket" => Network::Ws,
            "grpc" | "gun" => Network::Grpc,
            "httpupgrade" => Network::HttpUpgrade,
            // HTTP/2 transport was removed from Xray and migrated to XHTTP stream-one.
            "xhttp" | "splithttp" | "h2" | "http" => Network::Xhttp,
            _ => return None,
        })
    }
}

/// Transport (stream) settings. Fields are shared between networks the same way share links do.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Transport {
    pub network: Network,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// gRPC service name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    /// gRPC authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    /// TCP header type (`http` obfuscation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_type: Option<String>,
    /// gRPC `multi`/`gun`, XHTTP `auto`/`packet-up`/`stream-up`/`stream-one`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityKind {
    #[default]
    None,
    Tls,
    Reality,
}

impl SecurityKind {
    pub fn name(&self) -> &'static str {
        match self {
            SecurityKind::None => "none",
            SecurityKind::Tls => "tls",
            SecurityKind::Reality => "reality",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Security {
    pub kind: SecurityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alpn: Vec<String>,
    /// `allowInsecure` from the link. Xray 26 removed it, so it is shown but not applied;
    /// `pinned_sha256` / `verify_name` are the replacements.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub allow_insecure: bool,
    /// `pcs`: pinnedPeerCertSha256.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_sha256: Option<String>,
    /// `vcn`: verifyPeerCertByName.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_name: Option<String>,
    /// REALITY public key (`pbk`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// REALITY short id (`sid`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_id: Option<String>,
    /// REALITY spiderX (`spx`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spider_x: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Latency {
    Ok {
        ms: u32,
    },
    Failed {
        #[serde(default)]
        error: String,
    },
}

impl Latency {
    pub fn ms(&self) -> Option<u32> {
        match self {
            Latency::Ok { ms } => Some(*ms),
            Latency::Failed { .. } => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Latency::Ok { ms } => format!("{ms} ms"),
            Latency::Failed { .. } => "timeout".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    /// Subscription id this server came from. `None` = added manually.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub address: String,
    pub port: u16,
    pub protocol: Protocol,
    #[serde(default)]
    pub transport: Transport,
    #[serde(default)]
    pub security: Security,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Latency>,
}

impl Server {
    pub fn new(
        name: String,
        address: String,
        port: u16,
        protocol: Protocol,
        transport: Transport,
        security: Security,
    ) -> Server {
        let mut s = Server {
            id: String::new(),
            name,
            group: None,
            address,
            port,
            protocol,
            transport,
            security,
            latency: None,
        };
        s.id = s.compute_id();
        s
    }

    /// Stable id derived from connection-relevant fields (not the name), so a server keeps its id
    /// across subscription refreshes.
    pub fn compute_id(&self) -> String {
        let key = serde_json::json!([
            self.address,
            self.port,
            self.protocol,
            self.transport,
            self.security
        ]);
        format!("{:016x}", fnv1a64(key.to_string().as_bytes()))
    }

    /// e.g. `vless · tcp · reality`
    pub fn kind_label(&self) -> String {
        let mut s = format!(
            "{} · {}",
            self.protocol.name(),
            self.transport.network.name()
        );
        if self.security.kind != SecurityKind::None {
            s.push_str(" · ");
            s.push_str(self.security.kind.name());
        }
        s
    }

    pub fn latency_ms(&self) -> Option<u32> {
        self.latency.as_ref().and_then(Latency::ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Proxy,
    Tun,
}

impl Mode {
    pub fn label(&self) -> &'static str {
        match self {
            Mode::Proxy => "Proxy",
            Mode::Tun => "TUN",
        }
    }
    pub fn other(&self) -> Mode {
        match self {
            Mode::Proxy => Mode::Tun,
            Mode::Tun => Mode::Proxy,
        }
    }
}

/// Traffic usage reported by the `Subscription-Userinfo` header.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// Unix timestamp, 0 = never.
    pub expire: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ConnState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub state: ConnState,
    pub mode: Mode,
    pub server_id: Option<String>,
    pub server_name: Option<String>,
    /// Bytes per second.
    pub up_speed: u64,
    pub down_speed: u64,
    pub total_up: u64,
    pub total_down: u64,
    pub testing: bool,
    pub socks_port: u16,
    pub http_port: u16,
    /// Unix time the current connection came up.
    pub connected_since: Option<i64>,
    /// Public IP / location as seen through the tunnel.
    pub exit: Option<ExitInfo>,
}

/// Where traffic leaves the tunnel, as reported by an IP lookup service.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExitInfo {
    pub ip: String,
    pub country: String,
    pub country_code: String,
    pub city: String,
    pub isp: String,
}

impl Status {
    pub fn is_connected(&self) -> bool {
        self.state == ConnState::Connected
    }
    pub fn is_active(&self) -> bool {
        matches!(self.state, ConnState::Connected | ConnState::Connecting)
    }
}
