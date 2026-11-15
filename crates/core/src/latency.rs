//! Real-delay latency test: start one temporary Xray with a SOCKS inbound per server and time an
//! HTTP request through each.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::stream::{self, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

use crate::model::{Latency, Server};
use crate::store::write_atomic;
use crate::xray::latency_config;

const BATCH: usize = 64;

#[derive(Debug, Clone)]
pub struct Tester {
    pub xray_path: String,
    pub work_dir: PathBuf,
    pub url: String,
    pub timeout: Duration,
    pub concurrency: usize,
}

impl Tester {
    /// Test all `servers`, calling `on_result` as each finishes.
    pub async fn run<F>(&self, servers: Vec<Server>, on_result: F) -> Result<()>
    where
        F: Fn(String, Latency) + Send + Sync,
    {
        for chunk in servers.chunks(BATCH) {
            self.run_batch(chunk, &on_result, true).await?;
        }
        Ok(())
    }

    async fn run_batch<F>(&self, servers: &[Server], on_result: &F, allow_split: bool) -> Result<()>
    where
        F: Fn(String, Latency) + Send + Sync,
    {
        let ports = free_ports(servers.len()).await?;
        let pairs: Vec<(&Server, u16)> = servers.iter().zip(ports.iter().copied()).collect();
        let cfg = latency_config(&pairs);
        let cfg_path =
            self.work_dir
                .join(format!("latency-{}-{}.json", std::process::id(), ports[0]));
        write_atomic(&cfg_path, cfg.to_string().as_bytes(), true)?;

        let started = self.start_xray(&cfg_path, ports[0]).await;
        let mut child = match started {
            Ok(c) => c,
            Err(e) => {
                let _ = std::fs::remove_file(&cfg_path);
                // One bad server makes Xray reject the whole config; isolate it.
                if allow_split && servers.len() > 1 {
                    for s in servers {
                        Box::pin(self.run_batch(std::slice::from_ref(s), on_result, false)).await?;
                    }
                    return Ok(());
                }
                for s in servers {
                    on_result(
                        s.id.clone(),
                        Latency::Failed {
                            error: format!("{e:#}"),
                        },
                    );
                }
                return Ok(());
            }
        };

        let jobs: Vec<(String, u16)> = pairs.iter().map(|(s, p)| (s.id.clone(), *p)).collect();
        let mut results = stream::iter(jobs)
            .map(|(id, port)| {
                let tester = self.clone();
                async move { (id, tester.measure(port).await) }
            })
            .buffer_unordered(self.concurrency.max(1));
        while let Some((id, lat)) = results.next().await {
            on_result(id, lat);
        }

        let _ = child.kill().await;
        let _ = std::fs::remove_file(&cfg_path);
        Ok(())
    }

    async fn start_xray(
        &self,
        cfg: &std::path::Path,
        probe_port: u16,
    ) -> Result<tokio::process::Child> {
        let mut child = Command::new(&self.xray_path)
            .arg("run")
            .arg("-c")
            .arg(cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", self.xray_path))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait()? {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = e.read_to_string(&mut err).await;
                }
                let last = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .to_string();
                bail!("xray exited ({status}): {last}");
            }
            if TcpStream::connect(("127.0.0.1", probe_port)).await.is_ok() {
                return Ok(child);
            }
            if Instant::now() > deadline {
                bail!("xray did not start in time");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn measure(&self, port: u16) -> Latency {
        match self.measure_inner(port).await {
            Ok(ms) => Latency::Ok { ms },
            Err(e) => Latency::Failed {
                error: format!("{e:#}"),
            },
        }
    }

    /// First request warms up the tunnel (handshakes); the second, on the same keep-alive
    /// connection, measures the round trip. Falls back to the first timing.
    async fn measure_inner(&self, port: u16) -> Result<u32> {
        let client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}"))?)
            .timeout(self.timeout)
            .build()?;
        let t0 = Instant::now();
        let r = client.get(&self.url).send().await?;
        if !(r.status().is_success()) {
            bail!("HTTP {}", r.status());
        }
        let _ = r.bytes().await;
        let first = t0.elapsed();
        let t1 = Instant::now();
        let second = match client.get(&self.url).send().await {
            Ok(r) if r.status().is_success() => Some(t1.elapsed()),
            _ => None,
        };
        let d = second.unwrap_or(first);
        Ok(d.as_millis().clamp(1, u32::MAX as u128) as u32)
    }
}

async fn free_ports(n: usize) -> Result<Vec<u16>> {
    // Hold all listeners until every port is chosen so we don't get duplicates.
    let mut listeners = Vec::with_capacity(n);
    for _ in 0..n {
        listeners.push(TcpListener::bind("127.0.0.1:0").await?);
    }
    listeners
        .iter()
        .map(|l| Ok(l.local_addr()?.port()))
        .collect()
}
