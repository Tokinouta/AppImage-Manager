//! Persistent state.
//!
//! The single source of truth lives at
//! `$XDG_DATA_HOME/aim/state.toml`. It enumerates every installed
//! AppImage along with its identity, install metadata, and per-app
//! overrides. The schema is versioned (`schema_version = 1`) so we can
//! refuse incompatible versions cleanly.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Top-level state document.
///
/// `BTreeMap` keeps iteration order deterministic and produces
/// pleasant diffs when the user hand-edits the file.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct State {
    /// Schema version. Bump this when the on-disk format changes.
    pub schema_version: u64,
    /// The install records, keyed by `id@version`.
    pub apps: BTreeMap<String, AppRecord>,
}

impl State {
    /// New, empty state at `schema_version = 1`.
    #[must_use]
    pub fn new() -> Self {
        Self { schema_version: 1, apps: BTreeMap::new() }
    }

    /// Load from `path`. Returns an empty state if the file does not
    /// exist yet.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let text = std::fs::read_to_string(path).map_err(|source| Error::Path {
            path: path.to_path_buf(),
            source,
        })?;
        let state: State = toml::from_str(&text).map_err(|e| Error::StateCorrupt {
            message: format!("{e}"),
        })?;
        if state.schema_version != 1 {
            return Err(Error::StateVersion {
                found: state.schema_version,
                required: 1,
            });
        }
        Ok(state)
    }

    /// Save atomically. Writes to a temp file in the same directory,
    /// then renames into place.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text =
            toml::to_string(self).map_err(|e| Error::StateCorrupt { message: format!("{e}") })?;
        crate::staging::atomic_write(path, text.as_bytes())
    }

    /// Resolve a user-supplied target string to the record it names.
    /// Accepts:
    ///
    /// - The literal key `id@version` (preferred).
    /// - A bare `id` when exactly one record matches; errors otherwise.
    pub fn lookup(&self, target: &str) -> Result<(String, AppRecord)> {
        if let Some(rec) = self.apps.get(target) {
            return Ok((target.to_string(), rec.clone()));
        }
        let matches: Vec<_> = self
            .apps
            .iter()
            .filter(|(k, _)| k.starts_with(target) && k.as_str() == target || {
                // bare-id match: the key either equals the target or
                // matches `target@<something>`.
                if let Some((id_part, _)) = k.split_once('@') {
                    id_part == target
                } else {
                    false
                }
            })
            .collect();

        match matches.len() {
            0 => Err(Error::NotInstalled { id: target.to_string() }),
            1 => {
                let (k, v) = matches[0];
                Ok((k.clone(), v.clone()))
            }
            n => Err(Error::InvalidTarget {
                message: format!(
                    "'{target}' matches {n} installs; pass a full id@version to disambiguate"
                ),
            }),
        }
    }
}

/// A single install record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppRecord {
    /// Stable id (`vikunja`).
    pub id: String,
    /// Version (`1.2.3`) or `None` if unknown.
    pub version: Option<String>,
    /// SHA-256 of the cached AppImage, hex-encoded. Used as the
    /// "same file?" fingerprint (re-runs of the same download hit a
    /// cache hit).
    pub sha256: String,
    /// Full size in bytes.
    pub size_bytes: u64,
    /// Whether `.desktop` and bin symlink are present.
    #[serde(default)]
    pub integrated: bool,
    /// Where the AppImage came from (originally).
    #[serde(default)]
    pub source: Source,
    /// Icon size in pixels that was extracted (if any).
    #[serde(default)]
    pub icon_size_px: Option<u32>,
    /// Path under `~/.local/share/icons/` (relative).
    #[serde(default)]
    pub icon_path: Option<String>,
    /// Per-app environment variable overrides applied at integration
    /// time. `KEY = "value"` lines land verbatim in the generated
    /// `.desktop` file's `Exec=` shim.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extra flags appended to `Exec=` after the cached AppImage path.
    /// E.g. `["--no-sandbox", "--disable-gpu"]`.
    #[serde(default)]
    pub extra_exec_args: Vec<String>,
    /// Comment override (taken from `.desktop`).
    #[serde(default)]
    pub comment: Option<String>,
    /// Categories override (taken from `.desktop`).
    #[serde(default)]
    pub categories: Vec<String>,
    /// Install timestamp (seconds since epoch).
    pub installed_at_secs: i64,
}

impl AppRecord {
    /// Canonical id@version key for the state map.
    #[must_use]
    pub fn key(&self) -> String {
        match &self.version {
            Some(v) => format!("{}@{}", self.id, v),
            None => self.id.clone(),
        }
    }
}

/// Provenance of an installed AppImage. Strings are kept open-ended
/// (they may be local paths, URLs, or `gh:owner/repo` style refs) so
/// `aim info` can show a useful "where from" without strict parsing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Already on disk at install time.
    Local { path: String },
    /// Future: a direct URL.
    Url { url: String },
    /// Future: a GitHub `owner/repo` latest release.
    Gh { owner: String, repo: String },
    /// Provenance lost or unknown.
    Unknown,
}

impl Default for Source {
    fn default() -> Self {
        Self::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_round_trips() {
        let s = State::new();
        let bytes = toml::to_string(&s).unwrap();
        let back: State = toml::from_str(&bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn populated_state_round_trips() {
        let mut s = State::new();
        let mut env = BTreeMap::new();
        env.insert("SDL_VIDEODRIVER".to_string(), "wayland".to_string());
        let rec = AppRecord {
            id: "foo".into(),
            version: Some("1.2.3".into()),
            sha256: "deadbeef".repeat(8),
            size_bytes: 42,
            integrated: false,
            source: Source::Local { path: "/x/Foo.AppImage".into() },
            icon_size_px: Some(512),
            icon_path: Some("hicolor/512x512/apps/aim-foo.png".into()),
            env,
            extra_exec_args: vec!["--no-sandbox".into()],
            comment: Some("A foo".into()),
            categories: vec!["Utility".into()],
            installed_at_secs: 1_700_000_000,
        };
        s.apps.insert(rec.key(), rec.clone());

        let bytes = toml::to_string(&s).unwrap();
        let back: State = toml::from_str(&bytes).unwrap();
        assert_eq!(back.apps.get("foo@1.2.3"), Some(&rec));
    }
}
