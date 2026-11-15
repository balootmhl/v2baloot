use anyhow::{Context, Result};
use url::Url;

use super::*;

pub fn parse(link: &str) -> Result<Server> {
    let url = Url::parse(link).context("invalid trojan link")?;
    let password = userinfo(&url);
    if password.is_empty() {
        anyhow::bail!("missing password");
    }
    let (address, port) = host_port(&url)?;
    let q = query_map(&url);
    let transport = transport_from_query(&q)?;
    // Trojan is TLS unless stated otherwise.
    let security = security_from_query(&q, SecurityKind::Tls)?;
    let name = fragment_name(&url, &address, port);
    Ok(Server::new(
        name,
        address,
        port,
        Protocol::Trojan { password },
        transport,
        security,
    ))
}

pub fn to_link(server: &Server) -> String {
    let Protocol::Trojan { password } = &server.protocol else {
        unreachable!()
    };
    format!(
        "trojan://{}@{}:{}?{}#{}",
        encode_component(password),
        url_host(&server.address),
        server.port,
        common_query(server, &[]),
        encode_component(&server.name)
    )
}
