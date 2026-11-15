use anyhow::{Context, Result};
use url::Url;

use super::*;

pub fn parse(link: &str) -> Result<Server> {
    let url = Url::parse(link).context("invalid vless link")?;
    let uuid = userinfo(&url);
    if uuid.is_empty() {
        anyhow::bail!("missing uuid");
    }
    let (address, port) = host_port(&url)?;
    let q = query_map(&url);
    let transport = transport_from_query(&q)?;
    let security = security_from_query(&q, SecurityKind::None)?;
    let protocol = Protocol::Vless {
        uuid,
        flow: nonempty(&q, "flow"),
        encryption: nonempty(&q, "encryption").unwrap_or_else(|| "none".into()),
    };
    let name = fragment_name(&url, &address, port);
    Ok(Server::new(
        name, address, port, protocol, transport, security,
    ))
}

pub fn to_link(server: &Server) -> String {
    let Protocol::Vless {
        uuid,
        flow,
        encryption,
    } = &server.protocol
    else {
        unreachable!()
    };
    let mut extra = vec![("encryption", encryption.clone())];
    if let Some(f) = flow {
        extra.push(("flow", f.clone()));
    }
    format!(
        "vless://{}@{}:{}?{}#{}",
        encode_component(uuid),
        url_host(&server.address),
        server.port,
        common_query(server, &extra),
        encode_component(&server.name)
    )
}
