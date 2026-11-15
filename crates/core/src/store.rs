//! Persistent settings (`config.toml`) and data (`servers.json`).

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::model::*;
use crate::xray::XrayOptions;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Path or name of the xray binary.
    pub xray_path: String,
    pub listen: String,
    pub socks_port: u16,
    pub http_port: u16,
    pub api_port: u16,
    /// Set GNOME/Hyprland/systemd proxy variables while connected in Proxy mode.
    pub system_proxy: bool,
    /// Route LAN/private addresses directly.
    pub bypass_private: bool,
    pub dns: Vec<String>,
    /// DNS server assigned to the TUN interface (queries are hijacked by Xray anyway).
    pub tun_dns: String,
    pub log_level: String,
    /// Connect to the last server when the daemon starts.
    pub autoconnect: bool,
    /// Refresh subscriptions every N hours. 0 = never.
    pub sub_update_hours: u32,
    pub sub_user_agent: String,
    pub test_url: String,
    pub test_timeout_ms: u64,
    pub test_concurrency: usize,
    /// Show a StatusNotifierItem tray icon.
    pub tray: bool,
    pub notifications: bool,
    /// Command used by `v2baloot menu`; receives lines on stdin, prints the chosen line.
    pub menu_command: String,
    /// Show the server name next to the Waybar icon.
    pub waybar_show_name: bool,
    /// Idle timeout for proxied connections, seconds (V2Box uses 30).
    pub conn_idle_secs: u32,
    /// Look up the exit IP/country through the tunnel after connecting.
    pub show_exit_ip: bool,
    /// Bind proxy traffic to a specific network interface (e.g. `wlan0`); empty = system routing.
    pub outbound_interface: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            xray_path: "xray".into(),
            listen: "127.0.0.1".into(),
            socks_port: 10808,
            http_port: 10809,
            api_port: 10085,
            system_proxy: true,
            bypass_private: true,
            dns: vec!["1.1.1.1".into(), "8.8.8.8".into()],
            tun_dns: "1.1.1.1".into(),
            log_level: "warning".into(),
            autoconnect: false,
            sub_update_hours: 12,
            sub_user_agent: "v2rayN/7.0 v2baloot/0.1".into(),
            test_url: "https://www.gstatic.com/generate_204".into(),
            test_timeout_ms: 5000,
            test_concurrency: 16,
            tray: true,
            notifications: true,
            menu_command: "walker --dmenu -p 'v2baloot…'".into(),
            waybar_show_name: false,
            conn_idle_secs: 30,
            outbound_interface: None,
            show_exit_ip: true,
        }
    }
}

impl Settings {
    pub fn xray_options(&self, tun: bool) -> XrayOptions {
        XrayOptions {
            listen: self.listen.clone(),
            socks_port: self.socks_port,
            http_port: self.http_port,
            api_port: Some(self.api_port),
            log_level: self.log_level.clone(),
            dns: self.dns.clone(),
            bypass_private: self.bypass_private,
            tun,
            local_inbounds: true,
            conn_idle_secs: self.conn_idle_secs,
            outbound_interface: self.outbound_interface.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub selected: Option<String>,
    pub mode: Mode,
}

impl Data {
    pub fn server(&self, id: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    /// Add servers, skipping ids that already exist. Returns how many were added.
    pub fn add_servers(&mut self, servers: Vec<Server>) -> usize {
        let mut ids: std::collections::HashSet<String> =
            self.servers.iter().map(|s| s.id.clone()).collect();
        let mut n = 0;
        for s in servers {
            if ids.insert(s.id.clone()) {
                self.servers.push(s);
                n += 1;
            }
        }
        n
    }

    /// Replace all servers of a subscription, keeping order and previous latency results.
    /// Returns how many servers the group now has.
    pub fn replace_group(&mut self, group: &str, mut servers: Vec<Server>) -> usize {
        let old_latency: std::collections::HashMap<&str, &Option<crate::model::Latency>> = self
            .servers
            .iter()
            .map(|o| (o.id.as_str(), &o.latency))
            .collect();
        for s in &mut servers {
            s.group = Some(group.to_string());
            if let Some(l) = old_latency.get(s.id.as_str()) {
                s.latency = (*l).clone();
            }
        }
        let insert_at = self
            .servers
            .iter()
            .position(|s| s.group.as_deref() == Some(group))
            .unwrap_or(self.servers.len());
        self.servers.retain(|s| s.group.as_deref() != Some(group));
        let insert_at = insert_at.min(self.servers.len());
        // A server can't be both manual and in a group; drop duplicates from the new list.
        let others: std::collections::HashSet<&str> =
            self.servers.iter().map(|o| o.id.as_str()).collect();
        servers.retain(|s| !others.contains(s.id.as_str()));
        let n = servers.len();
        self.servers.splice(insert_at..insert_at, servers);
        n
    }

    pub fn remove_group(&mut self, group: &str) {
        self.servers.retain(|s| s.group.as_deref() != Some(group));
        self.subscriptions.retain(|s| s.id != group);
    }
}

pub fn load_settings(path: &Path) -> Result<Settings> {
    match fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn save_settings(path: &Path, s: &Settings) -> Result<()> {
    write_atomic(path, toml::to_string_pretty(s)?.as_bytes(), false)
}

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

pub fn load_data(path: &Path) -> Result<Data> {
    let mut d: Data = load_json(path)?;
    // Data written by older versions wasn't sanitized.
    for s in &mut d.servers {
        s.name = crate::util::clean_text(&s.name, 120);
    }
    for s in &mut d.subscriptions {
        s.name = crate::util::clean_text(&s.name, 60);
        s.last_error = s
            .last_error
            .as_deref()
            .map(|e| crate::util::clean_text(e, 300));
    }
    Ok(d)
}

pub fn save_data(path: &Path, d: &Data) -> Result<()> {
    // servers.json contains credentials: keep it private.
    write_atomic(path, &serde_json::to_vec_pretty(d)?, true)
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        #[cfg(unix)]
        if private {
            // `mode` only applies to new files; a leftover temp file may be wider.
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link;

    fn srv(link: &str) -> Server {
        link::parse(link).unwrap()
    }

    #[test]
    fn replace_group_keeps_latency_and_position() {
        let mut d = Data::default();
        d.add_servers(vec![srv("trojan://a@m.example.com:443#manual")]);
        let mut a = srv("trojan://a@a.example.com:443#a");
        a.latency = Some(Latency::Ok { ms: 120 });
        d.replace_group(
            "sub1",
            vec![a.clone(), srv("trojan://a@b.example.com:443#b")],
        );
        d.add_servers(vec![srv("trojan://a@z.example.com:443#after")]);
        assert_eq!(d.servers.len(), 4);

        let mut a2 = a.clone();
        a2.latency = None;
        a2.name = "a renamed".into();
        d.replace_group("sub1", vec![a2, srv("trojan://a@c.example.com:443#c")]);
        let names: Vec<_> = d.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["manual", "a renamed", "c", "after"]);
        assert_eq!(d.servers[1].latency, Some(Latency::Ok { ms: 120 }));
        assert_eq!(d.servers[1].group.as_deref(), Some("sub1"));
    }

    #[test]
    fn settings_defaults_fill_missing_keys() {
        let s: Settings = toml::from_str("socks_port = 2080\n").unwrap();
        assert_eq!(s.socks_port, 2080);
        assert_eq!(s.http_port, 10809);
    }
}
