//! Traffic counters via `xray api statsquery`.

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::process::Command;

/// Total (uplink, downlink) bytes of the `proxy` outbound.
pub async fn query(xray: &str, api_port: u16) -> Result<(u64, u64)> {
    let out = Command::new(xray)
        .args(["api", "statsquery"])
        .arg(format!("--server=127.0.0.1:{api_port}"))
        .args(["-pattern", "outbound>>>proxy>>>traffic"])
        .output()
        .await
        .context("running xray api")?;
    if !out.status.success() {
        anyhow::bail!(
            "xray api failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

fn parse(text: &str) -> Result<(u64, u64)> {
    let v: Value = serde_json::from_str(text).context("parsing stats")?;
    let mut up = 0;
    let mut down = 0;
    for stat in v
        .get("stat")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = stat.get("name").and_then(Value::as_str).unwrap_or("");
        // int64 is encoded as a string in protobuf JSON; absent when zero.
        let value = match stat.get("value") {
            Some(Value::String(s)) => s.parse().unwrap_or(0),
            Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
            _ => 0,
        };
        if name.ends_with(">>>uplink") {
            up = value;
        } else if name.ends_with(">>>downlink") {
            down = value;
        }
    }
    Ok((up, down))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_stats() {
        let t = r#"{"stat":[{"name":"outbound>>>proxy>>>traffic>>>downlink","value":"2048"},
                   {"name":"outbound>>>proxy>>>traffic>>>uplink","value":512}]}"#;
        assert_eq!(super::parse(t).unwrap(), (512, 2048));
        assert_eq!(
            super::parse(r#"{"stat":[{"name":"outbound>>>proxy>>>traffic>>>uplink"}]}"#).unwrap(),
            (0, 0)
        );
        assert_eq!(super::parse("{}").unwrap(), (0, 0));
    }
}
