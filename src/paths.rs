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
#[allow(clippy::struct_excessive_bools)]
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
    /// `$XDG_DATA_HOME/applications/` (relative to `home` when
    /// constructed via `with_home`).
    pub apps_root_dir: PathBuf,
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
        // Icons must live in the *theme* search path, which is
        // `$XDG_DATA_HOME/icons/`, NOT inside our app-specific data
        // dir. Otherwise the icon theme lookup (hicolor, Adwaita,
        // etc.) will never find them.
        let icons_dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let state_file = data_dir.join("state.toml");
        let apps_root_dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

        Self {
            config_dir,
            data_dir,
            apps_dir,
            icons_dir,
            state_file,
            bin_dir,
            apps_root_dir,
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
        // Same XDG rationale as `resolve()`: icons live in
        // `$XDG_DATA_HOME/icons/`, sibling to our app data dir.
        let icons_dir = home.join(".local").join("share");
        let state_file = data_dir.join("state.toml");
        // The XDG apps dir sits next to `data_dir` in the `share/`
        // tree. `with_home` mirrors the exact same relative layout.
        let apps_root_dir = home.join(".local").join("share");

        Self {
            config_dir,
            data_dir,
            apps_dir,
            icons_dir,
            state_file,
            bin_dir,
            apps_root_dir,
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
        self.apps_root_dir
            .join("applications")
            .join(format!("aim-{}.desktop", sanitize_id(id_version)))
    }

    /// Bin symlink location.
    #[must_use]
    pub fn bin_link(&self, id_version: &str) -> PathBuf {
        self.bin_dir.join(sanitize_id(id_version))
    }

    /// Icon path for a specific theme size. The filename uses just
    /// the id (not the full `id@version` key) so that the
    /// `.desktop` file's `Icon=aim-<id>` reference resolves
    /// consistently via the hicolor theme lookup.
    #[must_use]
    pub fn icon_path(&self, id_version: &str, size: u32) -> PathBuf {
        let id = id_for_key(id_version);
        self.icons_dir
            .join("hicolor")
            .join(format!("{size}x{size}"))
            .join("apps")
            .join(format!("aim-{}.png", sanitize_id(id)))
    }
}

/// Return the id portion (before `@version`) of a state-record key.
/// When the key has no `@`, the whole string is the id.
#[must_use]
pub fn id_for_key(key: &str) -> &str {
    match key.split_once('@') {
        Some((id, _)) => id,
        None => key,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_for_key_strips_version() {
        assert_eq!(id_for_key("Readest@0.12.16"), "Readest");
        assert_eq!(id_for_key("marktext-linux"), "marktext-linux");
        assert_eq!(id_for_key("foo@1.0.0-beta.1"), "foo");
    }

    #[test]
    fn icon_path_uses_id_only() {
        let home = std::path::PathBuf::from("/tmp");
        let layout = Layout::with_home(home);
        // The .desktop file references `Icon=aim-Readest`; the
        // icon file must live at the same name. Regression for the
        // bug where the file was named `aim-Readest@0.12.16.png`.
        let p = layout.icon_path("Readest@0.12.16", 512);
        assert_eq!(
            p.file_name().unwrap().to_string_lossy(),
            "aim-Readest.png"
        );
    }

    #[test]
    fn icon_path_no_version_uses_full_key() {
        let home = std::path::PathBuf::from("/tmp");
        let layout = Layout::with_home(home);
        let p = layout.icon_path("marktext-linux", 512);
        assert_eq!(
            p.file_name().unwrap().to_string_lossy(),
            "aim-marktext-linux.png"
        );
    }
}
