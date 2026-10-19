//! XDG path resolution for `aim`'s on-disk locations.
//!
//! All paths follow the XDG Base Directory spec:
//!
//! - Config:        `$XDG_CONFIG_HOME/aim/`      (default `~/.config/aim/`)
//! - Data:          `$XDG_DATA_HOME/aim/`        (default `~/.local/share/aim/`)
//!   - apps/<id>/<id>.AppImage          — cached AppImages
//!   - icons/hicolor/<size>/apps/<id>.png — extracted DirIcons
//! - State file:    `$XDG_DATA_HOME/aim/state.toml`
//! - Bin symlink:   `$XDG_DATA_HOME/../bin/<id>` (default `~/.local/bin/<id>`)
//! - Desktop file:  `$XDG_DATA_HOME/applications/aim-<id>.desktop`
//!                  (default `~/.local/share/applications/aim-<id>.desktop`)

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Resolved on-disk layout. All paths are absolute.
#[derive(Debug, Clone)]
pub struct Layout {
    /// `~/.config/aim/`
    pub config_dir: PathBuf,
    /// `~/.local/share/aim/`
    pub data_dir: PathBuf,
    /// `~/.local/share/aim/apps/`
    pub apps_dir: PathBuf,
    /// `~/.local/share/aim/icons/`
    pub icons_dir: PathBuf,
    /// `~/.local/share/aim/state.toml`
    pub state_file: PathBuf,
    /// `~/.local/bin/`
    pub bin_dir: PathBuf,
}

impl Layout {
    /// Resolve against the real user's home. Honours `$XDG_*_HOME`
    /// overrides where present, falling back to the XDG defaults.
    #[must_use]
    pub fn resolve() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("aim");
        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("aim");
        // XDG doesn't define ~/.local/bin; it's convention. We resolve
        // it as the sibling of data_dir's parent.
        let bin_dir = dirs::data_dir()
            .and_then(|d| d.parent().map(|p| p.join("bin")))
            .unwrap_or_else(|| PathBuf::from("/tmp/bin"));

        let apps_dir = data_dir.join("apps");
        let icons_dir = data_dir.join("icons");
        let state_file = data_dir.join("state.toml");

        Self {
            config_dir,
            data_dir,
            apps_dir,
            icons_dir,
            state_file,
            bin_dir,
        }
    }

    /// Construct a layout rooted at an arbitrary home directory. Used
    /// by integration tests to keep state isolated in a tempdir.
    #[must_use]
    pub fn with_home(home: PathBuf) -> Self {
        let config_dir = home.join(".config").join("aim");
        let data_dir = home.join(".local").join("share").join("aim");
        let bin_dir = home.join(".local").join("bin");

        let apps_dir = data_dir.join("apps");
        let icons_dir = data_dir.join("icons");
        let state_file = data_dir.join("state.toml");

        Self {
            config_dir,
            data_dir,
            apps_dir,
            icons_dir,
            state_file,
            bin_dir,
        }
    }

    /// Path to the AppImage cache directory for `id@version`.
    #[must_use]
    pub fn app_dir(&self, id_version: &str) -> PathBuf {
        self.apps_dir.join(sanitize_id(id_version))
    }

    /// Path to the cached AppImage file inside its cache directory.
    /// The on-disk file always keeps the literal name `AppImage` so
    /// `.desktop` `Exec=` lines reference a stable, non-renaming path.
    #[must_use]
    pub fn appimage_path(&self, id_version: &str) -> PathBuf {
        self.app_dir(id_version).join("AppImage")
    }

    /// Desktop file location (under `$XDG_DATA_HOME/applications/`).
    #[must_use]
    pub fn desktop_file(&self, id_version: &str) -> PathBuf {
        let apps_root = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        apps_root
            .join("applications")
            .join(format!("aim-{}.desktop", sanitize_id(id_version)))
    }

    /// Bin symlink location.
    #[must_use]
    pub fn bin_link(&self, id_version: &str) -> PathBuf {
        self.bin_dir.join(sanitize_id(id_version))
    }

    /// Icon path for a specific theme size.
    #[must_use]
    pub fn icon_path(&self, id_version: &str, size: u32) -> PathBuf {
        self.icons_dir
            .join("hicolor")
            .join(format!("{size}x{size}"))
            .join("apps")
            .join(format!("aim-{}.png", sanitize_id(id_version)))
    }
}

/// Replace filename-hostile characters in an id-derived string.
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' | '\0' => '_',
            c if c.is_ascii_control() => '_',
            c => c,
        })
        .collect()
}

/// `mkdir -p`-equivalent that maps I/O errors through [`Error::Path`].
pub fn ensure_dir(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(path).map_err(|source| Error::Path {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}
