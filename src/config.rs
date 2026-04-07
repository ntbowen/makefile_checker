use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::i18n::Lang;

/// Per-package override rules (stored in config under [pkg_rules.<pkg_name>])
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PkgRule {
    /// Regex patterns — versions matching any of these are ignored
    #[serde(default)]
    pub ignore_regex: Vec<String>,
    /// Only consider versions >= this value
    pub min_version: Option<String>,
    /// Only consider versions <= this value
    pub max_version: Option<String>,
    /// Strip this prefix from tag before treating as version (e.g. "release-")
    pub strip_prefix: Option<String>,
    /// Strip this suffix from tag before treating as version
    pub strip_suffix: Option<String>,
    /// If true, also include pre-release versions for this package
    #[serde(default)]
    pub include_prerelease: bool,
    /// Override: fetch this URL and extract version with url_regex_pattern
    pub url_regex_url: Option<String>,
    /// Regex with capture group 1 for version extraction
    pub url_regex_pattern: Option<String>,
    /// Skip this package entirely (no upstream check)
    #[serde(default)]
    pub skip: bool,
    /// Override upstream: use GitHub API for "owner/repo"
    pub github: Option<String>,
    /// Override upstream: use GitLab API for "host:owner/repo" or "owner/repo" (default gitlab.com)
    pub gitlab: Option<String>,
    /// Override upstream: use Gitea API for "host:owner/repo"
    pub gitea: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub search_paths: Vec<String>,
    pub parallel_jobs: usize,
    pub timeout_secs: u64,
    /// Number of retries on transient HTTP errors / rate-limit responses
    #[serde(default = "default_retry_times")]
    pub retry_times: u32,
    pub output_path: Option<String>,
    pub output_format: OutputFormat,
    pub github_token: Option<String>,
    /// Directory path patterns to skip (e.g. "host/", "toolchain/")
    pub skip_patterns: Vec<String>,
    /// Exact package names to skip entirely
    #[serde(default)]
    pub skip_packages: Vec<String>,
    /// Per-package override rules keyed by PKG_NAME
    #[serde(default)]
    pub pkg_rules: HashMap<String, PkgRule>,
    /// Global switch: when true, pre-release versions are included for ALL packages
    /// (pkg_rules.include_prerelease=true on a specific package always overrides regardless)
    #[serde(default)]
    pub include_prerelease: bool,
    #[serde(default)]
    pub lang: Lang,
}

fn default_retry_times() -> u32 { 3 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Xlsx,
    Csv,
    Both,
    None,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Xlsx => write!(f, "xlsx"),
            OutputFormat::Csv => write!(f, "csv"),
            OutputFormat::Both => write!(f, "both"),
            OutputFormat::None => write!(f, "none"),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            search_paths: vec![".".to_string()],
            parallel_jobs: num_cpus(),
            timeout_secs: 30,
            retry_times: 3,
            output_path: None,
            output_format: OutputFormat::Xlsx,
            github_token: std::env::var("GITHUB_TOKEN").ok(),
            skip_patterns: vec![
                "host/".to_string(),
                "toolchain/".to_string(),
            ],
            skip_packages: vec![],
            pkg_rules: HashMap::new(),
            include_prerelease: false,
            lang: Lang::En,
        }
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(16))
        .unwrap_or(4)
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("makefile_checker")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            match std::fs::read_to_string(&path)
                .context("read config")
                .and_then(|s| toml::from_str(&s).context("parse config"))
            {
                Ok(cfg) => cfg,
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }
}
