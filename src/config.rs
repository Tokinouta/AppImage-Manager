//! System-level configuration (`~/.config/aim/config.toml`).
//!
//! Slice 1 only reads the file; the editor UX (`aim info <id> --edit`)
//! lands with the `info` slice.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Defaults applied to every install. Per-app overrides in
/// `state.toml` take precedence.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SystemConfig {
    /// Schema version (always 1 for now).
    pub schema_version: u64,
    /// Default exec args appended to every AppImage's `Exec=` line.
    #[serde(default)]
    pub default_exec_args: Vec<String>,
    /// Default env vars applied to every AppImage.
    #[serde(default)]
    pub default_env: BTreeMap<String, String>,
    /// Default categories merged with `.desktop` categories.
    #[serde(default)]
    pub default_categories: Vec<String>,
}

impl SystemConfig {
    /// Load from disk. Returns an empty default if the file is
    /// missing.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self { schema_version: 1, ..Self::default() });
        }
        let text = std::fs::read_to_string(path).map_err(|source| Error::Path {
            path: path.to_path_buf(),
            source,
        })?;
        let cfg: Self = toml::from_str(&text).map_err(|e| Error::StateCorrupt {
            message: format!("{e}"),
        })?;
        Ok(cfg)
    }
}
