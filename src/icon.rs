//! DirIcon extraction.
//!
//! AppImages embed their own icons inside the SquashFS. We extract
//! the full `--appimage-extract` payload into a tempdir, then pick
//! the largest PNG and copy it into the hicolor icon theme tree so
//! file managers and the desktop environment pick it up.
//!
//! Slice 1 lessons (from a 30-minute debugging session against
//! `marktext-x86_64.AppImage`):
//!
//! - Pattern-limited `--appimage-extract '*.png'` only creates
//!   top-level symlinks; the real files inside `squashfs-root/usr/`
//!   are *not* extracted. Following those symlinks later ENOENTs.
//!   The fix is to extract the whole tree and recurse.
//! - `std::fs::copy` follows symlinks at read time, so symlinks
//!   created by the runtime work as long as their targets are real
//!   files. With full extraction they always are.
//! - `std::fs::copy` does not preserve the executable bit. The
//!   install op owns the chmod for the cached AppImage itself; we
//!   don't need to do anything special for icons.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::paths::Layout;
use crate::staging::StagingDir;

/// Result of an icon extraction attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedIcon {
    /// Pixel size that was extracted (e.g. `512`). We currently report
    /// a constant 512 — see `pick_best_icon` for the rationale.
    pub size_px: u32,
    /// Where the icon now lives on disk (absolute path).
    pub on_disk: PathBuf,
}

/// Attempt to extract and install the AppImage's DirIcon for `id_version`.
///
/// Returns `Ok(None)` when the AppImage has no embedded icon (some
/// don't). The caller is responsible for storing the result in the
/// `AppRecord` for later reference.
pub fn extract(id_version: &str, appimage_path: &Path, layout: &Layout) -> Result<Option<ExtractedIcon>> {
    let staging = StagingDir::new("aim-icon")?;

    let out = std::process::Command::new(appimage_path)
        .arg("--appimage-extract")
        .current_dir(staging.path())
        .output();

    let out = match out {
        Ok(o) => o,
        Err(_) => return Ok(None),
    };
    if !out.status.success() {
        return Ok(None);
    }

    let sqroot = staging.path().join("squashfs-root");
    if !sqroot.is_dir() {
        return Ok(None);
    }
    let pngs = find_pngs(&sqroot);
    if pngs.is_empty() {
        return Ok(None);
    }

    let (size_px, src) = pick_best_icon(&pngs);

    let dest = layout.icon_path(id_version, size_px);
    if let Some(parent) = dest.parent() {
        crate::paths::ensure_dir(parent)?;
    }
    if std::fs::copy(&src, &dest).is_err() {
        // Don't fail the install over an icon.
        return Ok(None);
    }

    Ok(Some(ExtractedIcon { size_px, on_disk: dest }))
}

/// Recursively collect PNGs under `root`. We descend into anything
/// `is_dir()` agrees is a directory (which follows symlinks — the
/// runtime creates a flat hierarchy of symlinks at `squashfs-root/`
/// pointing into `squashfs-root/usr/...`).
fn find_pngs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|s| s.to_str()) == Some("png") {
                out.push(p);
            }
        }
    }
    out
}

/// Pick the biggest PNG on disk. We don't decode the image header —
/// file size correlates well with resolution for the standard AppImage
/// icon set, and a PNG decoder would be heavy for this gain.
fn pick_best_icon(pngs: &[PathBuf]) -> (u32, PathBuf) {
    let mut best: Option<(u64, PathBuf)> = None;
    for p in pngs {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
            best = Some((size, p.clone()));
        }
    }
    let (_, path) = best.expect("non-empty by caller contract");
    // Reported size is informational — the icon-theme lookup uses
    // `aim-<id>` and finds whatever size's PNG is registered.
    (512, path)
}
