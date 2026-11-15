//! XDG paths. Implemented by hand so the root TUN helper can compute another user's paths.

use std::path::{Path, PathBuf};

pub const APP: &str = "v2baloot";

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub cache_dir: PathBuf,
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

impl Paths {
    /// Paths of the current user, honoring XDG_* variables.
    pub fn current() -> Paths {
        let home = env_dir("HOME").unwrap_or_else(|| PathBuf::from("/tmp"));
        let runtime_dir = match env_dir("XDG_RUNTIME_DIR") {
            Some(r) => r.join(APP),
            // No per-user runtime dir: use a uid-specific one (ownership is checked by the daemon).
            None => std::env::temp_dir().join(format!("{APP}-{}", unsafe_uid())),
        };
        Paths {
            config_dir: env_dir("XDG_CONFIG_HOME")
                .unwrap_or_else(|| home.join(".config"))
                .join(APP),
            data_dir: env_dir("XDG_DATA_HOME")
                .unwrap_or_else(|| home.join(".local/share"))
                .join(APP),
            state_dir: env_dir("XDG_STATE_HOME")
                .unwrap_or_else(|| home.join(".local/state"))
                .join(APP),
            runtime_dir,
            cache_dir: env_dir("XDG_CACHE_HOME")
                .unwrap_or_else(|| home.join(".cache"))
                .join(APP),
        }
    }

    /// Default XDG layout for an arbitrary home directory (used by the root TUN helper).
    pub fn for_home(home: &Path) -> Paths {
        Paths {
            config_dir: home.join(".config").join(APP),
            data_dir: home.join(".local/share").join(APP),
            state_dir: home.join(".local/state").join(APP),
            runtime_dir: PathBuf::from("/run").join(APP),
            cache_dir: home.join(".cache").join(APP),
        }
    }

    /// State icons for the Waybar module (referenced by absolute path from Waybar's CSS).
    pub fn waybar_icon(&self, state: &str) -> PathBuf {
        self.cache_dir.join("waybar").join(format!("{state}.png"))
    }
    pub fn notify_icon(&self) -> PathBuf {
        self.runtime_dir.join("icon.png")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }
    pub fn data_file(&self) -> PathBuf {
        self.data_dir.join("servers.json")
    }
    pub fn socket(&self) -> PathBuf {
        self.runtime_dir.join("daemon.sock")
    }
    pub fn xray_config(&self) -> PathBuf {
        self.runtime_dir.join("xray.json")
    }
    /// Server + options handed to the root TUN helper.
    pub fn tun_request(&self) -> PathBuf {
        self.state_dir.join("tun-request.json")
    }
}

#[cfg(unix)]
fn unsafe_uid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn unsafe_uid() -> u32 {
    0
}
