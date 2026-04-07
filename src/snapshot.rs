/// Version snapshot: persists last-seen upstream versions so future runs
/// can report only *changed* packages (nvchecker oldver/newver style).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Snapshot {
    /// pkg_name → last known upstream version
    pub versions: HashMap<String, String>,
}

impl Snapshot {
    pub fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("makefile_checker")
            .join("snapshot.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        if path.exists() {
            match std::fs::read_to_string(&path)
                .context("read snapshot")
                .and_then(|s| serde_json::from_str(&s).context("parse snapshot"))
            {
                Ok(s) => s,
                Err(_) => Self::default(),
            }
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Returns true if the upstream version has changed since last snapshot.
    pub fn has_changed(&self, pkg_name: &str, latest_version: &str) -> bool {
        match self.versions.get(pkg_name) {
            Some(prev) => prev != latest_version,
            None => true, // new package = changed
        }
    }

    /// Update a single entry.
    pub fn update(&mut self, pkg_name: &str, latest_version: &str) {
        self.versions.insert(pkg_name.to_string(), latest_version.to_string());
    }

    /// Bulk-update from check results, only for packages where we got a version.
    pub fn apply_results(&mut self, results: &[crate::reporter::CheckResult]) {
        for r in results {
            if let Some(v) = &r.upstream.latest_version {
                self.update(&r.upstream.pkg_name, v);
            }
        }
    }
}
