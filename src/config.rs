use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub jellyfin_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_key: Option<String>,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_scan_interval")]
    pub scan_interval_minutes: u64,
    #[serde(default = "default_true")]
    pub clean_deleted: bool,
    #[serde(default = "default_true")]
    pub include_movies: bool,
    #[serde(default = "default_true")]
    pub include_series: bool,
    #[serde(default)]
    pub library_filter: Vec<String>,
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("/media")
}

fn default_scan_interval() -> u64 {
    360 // 6 hours
}

fn default_true() -> bool {
    true
}

impl Config {
    pub fn load<P: AsRef<Path>>(config_path: Option<P>) -> anyhow::Result<Self> {
        let mut cfg = if let Some(path) = config_path {
            if path.as_ref().exists() {
                let content = std::fs::read_to_string(path)?;
                serde_yaml::from_str::<Config>(&content)?
            } else {
                Self::from_env()?
            }
        } else if Path::new("config.yaml").exists() {
            let content = std::fs::read_to_string("config.yaml")?;
            serde_yaml::from_str::<Config>(&content)?
        } else {
            Self::from_env()?
        };

        // Environment variable overrides
        if let Ok(v) = env::var("JELLYFIN_URL") {
            if !v.trim().is_empty() {
                cfg.jellyfin_url = v.trim().trim_end_matches('/').to_string();
            }
        }
        if let Ok(v) = env::var("JELLYFIN_USERNAME") {
            if !v.trim().is_empty() {
                cfg.username = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = env::var("JELLYFIN_PASSWORD") {
            if !v.trim().is_empty() {
                cfg.password = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = env::var("JELLYFIN_API_KEY") {
            if !v.trim().is_empty() {
                cfg.api_key = Some(v.trim().to_string());
            }
        }
        if let Ok(v) = env::var("OUTPUT_DIR") {
            if !v.trim().is_empty() {
                cfg.output_dir = PathBuf::from(v.trim());
            }
        }
        if let Ok(v) = env::var("SCAN_INTERVAL_MINUTES") {
            if let Ok(n) = v.trim().parse::<u64>() {
                cfg.scan_interval_minutes = n;
            }
        }
        if let Ok(v) = env::var("CLEAN_DELETED") {
            cfg.clean_deleted = v.trim().eq_ignore_ascii_case("true") || v.trim() == "1";
        }
        if let Ok(v) = env::var("INCLUDE_MOVIES") {
            cfg.include_movies = v.trim().eq_ignore_ascii_case("true") || v.trim() == "1";
        }
        if let Ok(v) = env::var("INCLUDE_SERIES") {
            cfg.include_series = v.trim().eq_ignore_ascii_case("true") || v.trim() == "1";
        }

        // Clean URL
        cfg.jellyfin_url = cfg.jellyfin_url.trim_end_matches('/').to_string();

        if cfg.jellyfin_url.is_empty() {
            anyhow::bail!("JELLYFIN_URL is required (set in config.yaml or env JELLYFIN_URL)");
        }
        if cfg.api_key.is_none() && cfg.username.is_none() {
            anyhow::bail!("Either JELLYFIN_API_KEY or JELLYFIN_USERNAME & JELLYFIN_PASSWORD must be provided");
        }

        Ok(cfg)
    }

    fn from_env() -> anyhow::Result<Self> {
        let jellyfin_url = env::var("JELLYFIN_URL").unwrap_or_default();
        let username = env::var("JELLYFIN_USERNAME").ok();
        let password = env::var("JELLYFIN_PASSWORD").ok();
        let api_key = env::var("JELLYFIN_API_KEY").ok();
        let output_dir = env::var("OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_output_dir());
        let scan_interval_minutes = env::var("SCAN_INTERVAL_MINUTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(default_scan_interval);
        let clean_deleted = env::var("CLEAN_DELETED")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        let include_movies = env::var("INCLUDE_MOVIES")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);
        let include_series = env::var("INCLUDE_SERIES")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(true);

        Ok(Self {
            jellyfin_url,
            username,
            password,
            api_key,
            output_dir,
            scan_interval_minutes,
            clean_deleted,
            include_movies,
            include_series,
            library_filter: Vec::new(),
        })
    }
}
