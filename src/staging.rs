//! Staging directories for atomic filesystem operations.
//!
//! Every install / remove op uses a `StagingDir` so that we either
//! commit the full result or none of it — no half-renamed AppImages,
//! no half-written `.desktop` files, no orphans.
//!
//! The directory lives under `tempfile::temp_dir()` and is removed on
//! drop. Every entry inside it is supposed to land at its final
//! destination via [`install_into`] which performs an atomic rename.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// RAII guard for a unique staging directory under the system tempdir.
#[derive(Debug)]
pub struct StagingDir {
    path: PathBuf,
}

impl StagingDir {
    /// Create a new staging directory named `aim-<prefix>-<rand>`.
    pub fn new(prefix: &str) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix(&format!("aim-{prefix}-"))
            .tempdir()
            .map_err(|source| Error::Path {
                path: std::env::temp_dir(),
                source,
            })?;
        Ok(Self {
            path: dir.keep(),
        })
    }

    /// Absolute path to the staging root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        // Best-effort cleanup. If remove_dir_all fails (a child
        // process still has a file open) we leave it for the OS's
        // temp cleaner.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Recursively copy `src` to `dst`. Used to stage a freshly downloaded
/// AppImage into the staging dir before atomic-rename into place.
pub fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Path {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::copy(src, dst).map_err(|source| Error::Path {
            path: dst.to_path_buf(),
            source,
        })?;
        return Ok(());
    }
    Err(Error::InvalidTarget {
        message: format!("expected file at {}", src.display()),
    })
}

/// Copy `src` → `dst` while preserving atomicity: readers of `dst`
/// see either the previous version or the full new one, never a
/// partial copy. We copy into a temp file in the same directory as
/// `dst`, then `persist` (which is `rename` on POSIX).
///
/// **Unlike `atomic_swap`, the source file is not consumed.** It is
/// not safe to assume the caller will re-use `src`.
pub fn copy_into_place(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let parent = dst.parent().ok_or_else(|| Error::InvalidTarget {
        message: format!("no parent for {}", dst.display()),
    })?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".aim-cache-")
        .suffix(".part")
        .tempfile_in(parent)
        .map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    std::io::copy(&mut std::fs::File::open(src).map_err(|source| Error::Path {
        path: src.to_path_buf(),
        source,
    })?, &mut tmp)
    .map_err(|source| Error::Path {
        path: tmp.path().to_path_buf(),
        source,
    })?;
    tmp.flush().map_err(|source| Error::Path {
        path: tmp.path().to_path_buf(),
        source,
    })?;
    match tmp.persist(dst) {
        Ok(_) => Ok(()),
        Err(e) => Err(Error::Path {
            path: dst.to_path_buf(),
            source: e.error,
        }),
    }
}

/// Atomically move `src` → `dst`. Both paths are expected to be on the
/// same filesystem; otherwise this will fall back to copy+remove.
///
/// On failure, `src` is left untouched so the caller can retry.
pub fn atomic_swap(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    // `rename` is atomic on POSIX when both paths are on the same
    // filesystem. If they're not (e.g. cross-device), fall back to
    // copy + remove.
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(src, dst).map_err(|source| Error::Path {
                path: dst.to_path_buf(),
                source,
            })?;
            std::fs::remove_file(src).map_err(|source| Error::Path {
                path: src.to_path_buf(),
                source,
            })?;
            Ok(())
        }
    }
}

/// Write `bytes` to `dst` via a temp file in the same directory, then
/// atomically rename into place. Prevents readers from observing
/// partially-written files.
pub fn atomic_write(dst: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let parent = dst.parent().ok_or_else(|| Error::InvalidTarget {
        message: format!("no parent for {}", dst.display()),
    })?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".aim-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    use std::io::Write;
    tmp.write_all(bytes).map_err(|source| Error::Path {
        path: tmp.path().to_path_buf(),
        source,
    })?;
    tmp.flush().map_err(|source| Error::Path {
        path: tmp.path().to_path_buf(),
        source,
    })?;
    match tmp.persist(dst) {
        Ok(_) => Ok(()),
        Err(e) => Err(Error::Path {
            path: dst.to_path_buf(),
            source: e.error,
        }),
    }
}

/// Create a symlink at `link` pointing to `target`. Removes any
/// pre-existing entry at `link` first (so re-running `integrate` is
/// idempotent).
pub fn symlink(target: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::Path {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if link.exists() || link.is_symlink() {
        let _ = std::fs::remove_file(link);
    }
    std::os::unix::fs::symlink(target, link).map_err(|source| Error::Path {
        path: link.to_path_buf(),
        source,
    })?;
    Ok(())
}

/// Remove a file, link, or empty directory. Missing paths are
/// tolerated so callers can use this in garbage-collection loops.
pub fn remove_any(path: &Path) {
    if path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    } else if path.exists() || path.is_symlink() {
        let _ = std::fs::remove_file(path);
    }
}
