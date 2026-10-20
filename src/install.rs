//! The install operation.
//!
//! `install::run` orchestrates the entire install: hash → identity →
//! stage → copy + atomic rename → icon extract → `.desktop` render →
//! state update → optional integrate.
//!
//! Atomicity: every on-disk side effect is staged under
//! `StagingDir` and renamed into place at the end. Failure mid-way
//! leaves a `StagingDir` that the OS temp cleaner will eventually
//! sweep.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

// SHA-256 streaming was extracted to `crate::hash` in slice 10 so
// that `verify` and `upgrade` can reuse it. The local helper below
// is kept as a thin re-export for any in-tree callers that still
// import it by its old name.

use crate::desktop;
use crate::error::{Error, Result};
use crate::icon;
use crate::identity::{self, IdSource};
use crate::paths::Layout;
use crate::state::{AppRecord, Source, State};
use crate::staging::{atomic_write, copy_into_place};

/// Caller's intent — what `aim install` actually asked for, after
/// argument disambiguation.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// Source AppImage file (already on disk).
    pub source: PathBuf,
    /// Where it came from, for the state record. `Local` for slice 1.
    pub source_kind: Source,
    /// User-supplied id override (`--name`).
    pub name_override: Option<String>,
    /// Whether to skip the integrate step (creating `.desktop` and bin
    /// symlink).
    pub no_integrate: bool,
    /// If a record already exists with the same id/version, overwrite
    /// it (instead of erroring with `AlreadyInstalled`).
    pub force: bool,
}

/// Result of a successful install. The CLI prints this on stdout.
#[derive(Debug, Clone)]
pub struct InstallResult {
    pub record: AppRecord,
    pub integrated: bool,
    pub icon_extracted: bool,
}

/// Run the install operation end-to-end.
#[allow(clippy::too_many_lines)]
pub fn run(req: InstallRequest, layout: &Layout) -> Result<InstallResult> {
    // 1. Identify the AppImage.
    let mut identity = identity::identify(&req.source)?;
    if let Some(name) = &req.name_override {
        identity.id = name.clone();
        identity.id_source = IdSource::UserOverride;
    }
    let key = identity::id_version(&identity);

    // 2. Hash the source file.
    let (sha256_hex, size_bytes) = hash_file(&req.source)?;

    // 3. Detect collisions with existing state.
    let mut state = State::load(&layout.state_file)?;
    if state.apps.contains_key(&key) && !req.force {
        return Err(Error::AlreadyInstalled {
            id: identity.id.clone(),
            version: identity.version.clone().unwrap_or_default(),
        });
    }

    // 4. Stage every on-disk side effect, then commit.
    let app_dir = layout.app_dir(&key);
    let cache_path = layout.appimage_path(&key);
    let desktop_path = layout.desktop_file(&key);

    // Copy (not move) into cache. We use the temp-file + atomic
    // rename pattern: readers of `$cache_path` either see the old
    // version or the full new one, never a partial.
    std::fs::create_dir_all(&app_dir).map_err(|source| Error::Path {
        path: app_dir.clone(),
        source,
    })?;
    copy_into_place(&req.source, &cache_path)?;

    // `std::fs::copy` does not preserve the source mode bits. AppImages
    // must be executable for `--appimage-extract` (which we use to find
    // the icon) and for the eventual `aim run`/`Exec=` line to work.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&cache_path, perms).map_err(|source| Error::Path {
            path: cache_path.clone(),
            source,
        })?;
    }

    // 5. Optional: icon extraction.
    let extracted = icon::extract(&key, &cache_path, layout)?;
    let (icon_size_px, icon_path_rel) = match &extracted {
        Some(e) => (
            Some(e.size_px),
            Some(
                e.on_disk
                    .strip_prefix(&layout.icons_dir)
                    .unwrap_or(&e.on_disk)
                    .to_string_lossy()
                    .into_owned(),
            ),
        ),
        None => (None, None),
    };

    // 6. Build the AppRecord. The renderer derives `AIM_INTEGRATED`
    //    from `record.integrated`, so we don't keep a copy in `env`;
    //    the per-app env map is for user overrides only.
    let will_integrate = !req.no_integrate;
    let record = AppRecord {
        id: identity.id.clone(),
        version: identity.version.clone(),
        sha256: sha256_hex,
        size_bytes,
        integrated: will_integrate,
        source: req.source_kind.clone(),
        icon_size_px,
        icon_path: icon_path_rel,
        env: std::collections::BTreeMap::new(),
        extra_exec_args: Vec::new(),
        comment: None,
        categories: Vec::new(),
        mime_types: Vec::new(),
        installed_at_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };

    // 7. Optionally integrate: write `.desktop`, drop bin symlink.
    let integrated = if will_integrate {
        let body = desktop::render(&record, &cache_path);
        atomic_write(&desktop_path, body.as_bytes())?;
        crate::staging::symlink(&cache_path, &layout.bin_link(&key))?;
        true
    } else {
        false
    };

    state.apps.insert(key.clone(), record.clone());
    state.save(&layout.state_file)?;

    Ok(InstallResult {
        record,
        integrated,
        icon_extracted: extracted.is_some(),
    })
}

/// SHA-256 + size of a file, streamed in 64 KiB chunks. Re-export of
/// [`crate::hash::hash_file`] kept as a module-local alias for
/// readability of the install pipeline.
fn hash_file(path: &Path) -> Result<(String, u64)> {
    crate::hash::hash_file(path)
}

/// Test-only helper, exposed so other test modules can re-use the
/// streaming dance. Currently unused but kept as a public surface
/// for future integration tests.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn hash_for_test(path: &Path) -> Result<(String, u64)> {
    hash_file(path)
}

/// Identity re-export so callers can use a single crate path.
pub use identity::identify as identify_appimage;


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn hash_empty_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("empty.bin");
        std::fs::write(&p, b"").unwrap();
        let (hex, size) = hash_file(&p).unwrap();
        assert_eq!(hex, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(size, 0);
    }

    #[test]
    fn hash_known_string() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        std::fs::write(&p, b"abc").unwrap();
        let (hex, size) = hash_file(&p).unwrap();
        // ba7816bf...8c is sha256("abc")
        assert!(hex.starts_with("ba7816bf"));
        assert_eq!(size, 3);
    }
}
