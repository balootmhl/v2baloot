mod daemon;
mod icons;
mod launch;
mod menu;
mod notify;
mod sysproxy;
mod tray;
mod tui;
mod tun;
mod waybar;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use v2baloot_core::ipc::{Client, Event, Reply, Request};
use v2baloot_core::util::{format_date, human_bytes, human_speed};
use v2baloot_core::{ConnState, Mode, Server};

#[derive(Parser)]
#[command(name = "v2baloot", version, about = "Xray client for Omarchy")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Terminal UI (default)
    Tui,
    /// Run the background daemon (normally started by systemd)
    Daemon,
    /// Open the GTK window
    Gui,
    /// Show connection status
    Status {
        #[arg(long)]
        json: bool,
    },
    /// List servers
    #[command(alias = "ls")]
    List,
    /// Connect (to a server id, list number or name; default: last used)
    Connect {
        server: Option<String>,
    },
    Disconnect,
    /// Connect if disconnected, otherwise disconnect
    Toggle,
    /// Set Proxy or TUN mode (no argument: switch)
    Mode {
        mode: Option<ModeArg>,
    },
    /// Import share links (no argument: from clipboard)
    Import {
        links: Vec<String>,
    },
    /// Manage subscriptions
    #[command(subcommand)]
    Sub(SubCmd),
    /// Latency test (no argument: all servers)
    Test {
        servers: Vec<String>,
    },
    /// Remove a server
    #[command(alias = "rm")]
    Remove {
        server: String,
    },
    /// Show recent xray log lines
    Logs,
    /// Waybar custom module output (streams JSON)
    Waybar,
    /// Server picker through Walker (dmenu mode)
    Menu,
    /// Root TUN helper (run by v2baloot-tun@.service)
    #[command(hide = true)]
    TunRun {
        #[arg(long)]
        user: String,
        #[arg(long)]
        cleanup: bool,
    },
}

#[derive(Subcommand)]
enum SubCmd {
    /// Add a subscription URL and fetch it
    Add {
        url: String,
        #[arg(long, short)]
        name: Option<String>,
    },
    /// Refresh one (id) or all subscriptions
    Update { id: Option<String> },
    #[command(alias = "ls")]
    List,
    #[command(alias = "rm")]
    Remove { id: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Proxy,
    Tun,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(
        cli.cmd,
        Some(
            Cmd::Status { .. } | Cmd::List | Cmd::Logs | Cmd::Test { .. } | Cmd::Sub(SubCmd::List)
        )
    ) {
        // Print-only commands behave like normal CLIs when piped into `head` etc. Everything else
        // keeps ignoring SIGPIPE: a write to a vanished daemon socket or picker must not kill us.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    }
    let level = match &cli.cmd {
        Some(Cmd::Daemon | Cmd::TunRun { .. }) => "info",
        _ => "warn",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into()),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
    let client = Client::default();
    match cli.cmd.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run().await?,
        Cmd::Daemon => daemon::run().await?,
        Cmd::Gui => launch::gui().context("starting v2baloot-gui")?,
        Cmd::Waybar => waybar::run().await?,
        Cmd::Menu => menu::run().await?,
        Cmd::TunRun { user, cleanup } => tun::run_helper(&user, cleanup).await?,
        Cmd::Status { json } => {
            let st = client.status().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
            } else {
                let state = match &st.state {
                    ConnState::Connected => "connected".to_string(),
                    ConnState::Connecting => "connecting".to_string(),
                    ConnState::Disconnected => "disconnected".to_string(),
                    ConnState::Error { message } => format!("error: {message}"),
                };
                println!("State:  {state}");
                println!("Mode:   {}", st.mode.label());
                println!("Server: {}", st.server_name.as_deref().unwrap_or("-"));
                if st.is_connected() {
                    println!(
                        "Speed:  ↑ {}  ↓ {}   (total ↑ {} ↓ {})",
                        human_speed(st.up_speed),
                        human_speed(st.down_speed),
                        human_bytes(st.total_up),
                        human_bytes(st.total_down)
                    );
                    if st.mode == Mode::Proxy {
                        println!(
                            "Proxy:  socks5://127.0.0.1:{}  http://127.0.0.1:{}",
                            st.socks_port, st.http_port
                        );
                    }
                }
            }
        }
        Cmd::List => {
            let (servers, selected) = client.servers().await?;
            let subs = client.subscriptions().await?;
            if servers.is_empty() {
                println!("No servers. Add some with `v2baloot import <link>` or `v2baloot sub add <url>`.");
            }
            for (i, s) in servers.iter().enumerate() {
                let group = s
                    .group
                    .as_ref()
                    .and_then(|g| subs.iter().find(|x| &x.id == g))
                    .map(|x| x.name.as_str())
                    .unwrap_or("manual");
                let lat = s
                    .latency
                    .as_ref()
                    .map(|l| l.label())
                    .unwrap_or_else(|| "-".into());
                let mark = if selected.as_ref() == Some(&s.id) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{mark}{:>3}  {:<36} {:<24} {:>8}  [{group}]  {}",
                    i + 1,
                    truncate(&s.name, 36),
                    s.kind_label(),
                    lat,
                    s.id
                );
            }
        }
        Cmd::Connect { server } => {
            let id = match server {
                Some(q) => Some(resolve(&client, &q).await?.id),
                None => None,
            };
            client.call(&Request::Connect { id }).await?;
            print_short_status(&client).await?;
        }
        Cmd::Disconnect => {
            client.call(&Request::Disconnect).await?;
        }
        Cmd::Toggle => {
            client.call(&Request::Toggle).await?;
            print_short_status(&client).await?;
        }
        Cmd::Mode { mode } => {
            let mode = match mode {
                Some(ModeArg::Proxy) => Mode::Proxy,
                Some(ModeArg::Tun) => Mode::Tun,
                None => client.status().await?.mode.other(),
            };
            client.call(&Request::SetMode { mode }).await?;
            println!("Mode: {}", mode.label());
        }
        Cmd::Import { links } => {
            let text = if links.is_empty() {
                launch::clipboard().context("clipboard is empty (pass links as arguments)")?
            } else {
                links.join("\n")
            };
            print_imported(client.call(&Request::Import { text }).await?);
        }
        Cmd::Sub(SubCmd::Add { url, name }) => {
            print_imported(
                client
                    .call(&Request::AddSubscription {
                        name: name.unwrap_or_default(),
                        url,
                    })
                    .await?,
            );
        }
        Cmd::Sub(SubCmd::Update { id }) => {
            print_imported(client.call(&Request::UpdateSubscriptions { id }).await?);
        }
        Cmd::Sub(SubCmd::Remove { id }) => {
            client.call(&Request::RemoveSubscription { id }).await?;
        }
        Cmd::Sub(SubCmd::List) => {
            let (servers, _) = client.servers().await?;
            for s in client.subscriptions().await? {
                let n = servers
                    .iter()
                    .filter(|x| x.group.as_ref() == Some(&s.id))
                    .count();
                println!("{}  {}  ({n} servers)", s.id, s.name);
                println!("    {}", s.url);
                if let Some(u) = &s.usage {
                    let mut line = format!(
                        "    used {} / {}",
                        human_bytes(u.upload + u.download),
                        if u.total > 0 {
                            human_bytes(u.total)
                        } else {
                            "∞".into()
                        }
                    );
                    if u.expire > 0 {
                        line.push_str(&format!(", expires {}", format_date(u.expire)));
                    }
                    println!("{line}");
                }
                if let Some(e) = &s.last_error {
                    println!("    last error: {e}");
                }
            }
        }
        Cmd::Test { servers } => test(&client, servers).await?,
        Cmd::Remove { server } => {
            let s = resolve(&client, &server).await?;
            client.call(&Request::RemoveServer { id: s.id }).await?;
            println!("Removed {}", s.name);
        }
        Cmd::Logs => {
            if let Reply::Logs { lines } = client.call(&Request::Logs).await? {
                for l in lines {
                    println!("{l}");
                }
            }
        }
    }
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

fn print_imported(r: Reply) {
    if let Reply::Imported { added, errors } = r {
        println!("{added} server(s) added/updated");
        for e in errors {
            eprintln!("  skipped: {e}");
        }
    }
}

async fn print_short_status(client: &Client) -> Result<()> {
    let st = client.status().await?;
    match st.state {
        ConnState::Connected => println!(
            "Connected to {} ({})",
            st.server_name.unwrap_or_default(),
            st.mode.label()
        ),
        ConnState::Disconnected => println!("Disconnected"),
        ConnState::Connecting => println!("Connecting…"),
        ConnState::Error { message } => bail!(message),
    }
    Ok(())
}

/// Find a server by id, 1-based list number, or (unique) name substring.
async fn resolve(client: &Client, q: &str) -> Result<Server> {
    let (servers, _) = client.servers().await?;
    if let Some(s) = servers.iter().find(|s| s.id == q) {
        return Ok(s.clone());
    }
    if let Ok(n) = q.parse::<usize>() {
        if n >= 1 && n <= servers.len() {
            return Ok(servers[n - 1].clone());
        }
    }
    let ql = q.to_lowercase();
    let hits: Vec<_> = servers
        .iter()
        .filter(|s| s.name.to_lowercase().contains(&ql))
        .collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => bail!("no server matches `{q}`"),
        n => bail!(
            "`{q}` matches {n} servers; be more specific or use the number from `v2baloot list`"
        ),
    }
}

async fn test(client: &Client, queries: Vec<String>) -> Result<()> {
    let mut ids = vec![];
    for q in &queries {
        ids.push(resolve(client, q).await?.id);
    }
    let (servers, _) = client.servers().await?;
    let mut events = client.subscribe().await?;
    client
        .call(&Request::TestLatency {
            ids: if ids.is_empty() { None } else { Some(ids) },
        })
        .await?;
    while let Some(ev) = events.next().await? {
        match ev {
            Event::Latency { id, latency } => {
                let name = servers
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.name.as_str())
                    .unwrap_or(&id);
                println!("{:>8}  {name}", latency.label());
            }
            Event::TestFinished => break,
            _ => {}
        }
    }
    Ok(())
}
