//! Configuration: a small TOML file, with env-var and flag overrides.
//!
//! Resolution precedence for every value is: **flag > environment > config file**.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Config {
    /// Callsign used for the (read-only) APRS-IS login.
    pub callsign: Option<String>,
    /// Default home location as "lat,lon".
    pub home: Option<String>,
    /// Optional aprs.fi API key (only used by `last`).
    pub aprsfi_key: Option<String>,
    /// Override the APRS-IS server (host:port).
    pub server: Option<String>,
}

impl Config {
    /// Path to the config file (e.g. `~/.config/claprs/config.toml`).
    pub fn path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "claprs")
            .context("could not determine a config directory for this platform")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Load the config file, or a default (empty) config if none exists yet.
    pub fn load() -> Result<Config> {
        let p = Self::path()?;
        if p.exists() {
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("reading {}", p.display()))?;
            toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))
        } else {
            Ok(Config::default())
        }
    }

    /// Write the config back to disk, creating parent directories as needed.
    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))
    }

    pub fn resolve_callsign(&self, flag: Option<String>) -> String {
        flag.or_else(|| std::env::var("CLAPRS_CALLSIGN").ok())
            .or_else(|| self.callsign.clone())
            .unwrap_or_else(|| "N0CALL".to_string())
    }

    pub fn resolve_server(&self, flag: Option<String>) -> String {
        flag.or_else(|| std::env::var("CLAPRS_SERVER").ok())
            .or_else(|| self.server.clone())
            .unwrap_or_else(|| "rotate.aprs2.net:14580".to_string())
    }

    pub fn resolve_aprsfi_key(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| std::env::var("APRSFI_API_KEY").ok())
            .or_else(|| self.aprsfi_key.clone())
    }

    pub fn resolve_home(&self, flag: Option<String>) -> Option<String> {
        flag.or_else(|| std::env::var("CLAPRS_HOME").ok())
            .or_else(|| self.home.clone())
    }
}
