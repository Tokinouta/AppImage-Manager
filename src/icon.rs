//! DirIcon extraction.
//!
//! AppImages embed their own icons inside the SquashFS. We extract
//! the full `--appimage-extract` payload into a tempdir, then pick
//! an icon by walking a precedence chain.
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
//!
//! Slice 11 lesson (from `siyuan-3.8.7` picking a 358 KB tutorial
//! image instead of the 5 KB M logo):
//!
//! - "Largest PNG wins" picks tutorial/guide assets over real icons
//!   because the tutorial screenshots dwarf the logo in bytes.
//!   Real AppImages ship icons in the freedesktop-canonical
//!   locations: a `.DirIcon` symlink at the squashfs root, plus a
//!   hicolor tree at `usr/share/icons/hicolor/<size>x<size>/apps/<id>.png`.
//!   Following `.DirIcon` first (and falling back through the
//!   hicolor tree + filename heuristics) gets the right icon.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::paths::{id_for_key, Layout};
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

    let id = id_for_key(id_version);

    // Walk the precedence chain. First match wins.
    let chosen = pick_best_icon(&sqroot, id);

    let Some((size_px, src)) = chosen else {
        return Ok(None);
    };

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

/// Pick the best icon to use, returning `(size_px, source_path)`.
///
/// Walks a precedence chain:
///
/// 1. `.DirIcon` (the freedesktop canonical name for a directory
///    icon, used by AppImage runtime and many AppImages).
/// 2. The hicolor theme tree
///    (`usr/share/icons/hicolor/<size>x<size>/apps/<id>.png`),
///    preferring the largest size present.
/// 3. A few conventional top-level names: `<id>.png`, `icon.png`,
///    `logo.png`, `icon-large.png`.
/// 4. Fallback: the largest PNG under the tree that isn't inside a
///    tutorial/guide/doc/help directory. This handles quirky
///    AppImages that ship icons in unexpected places.
///
/// Returns `None` when no PNGs are found at all.
fn pick_best_icon(sqroot: &Path, id: &str) -> Option<(u32, PathBuf)> {
    // 1. .DirIcon (canonical). Usually a symlink, which `is_file()`
    //    follows.
    let diricon = sqroot.join(".DirIcon");
    if diricon.is_file() {
        return Some((512, diricon));
    }

    // 2. hicolor theme tree, largest size first.
    for size_px in [512_u32, 256, 128, 64, 48, 32, 16] {
        let p = sqroot
            .join("usr")
            .join("share")
            .join("icons")
            .join("hicolor")
            .join(format!("{size_px}x{size_px}"))
            .join("apps")
            .join(format!("{id}.png"));
        if p.is_file() {
            return Some((size_px, p));
        }
    }

    // 3. Conventional top-level filenames.
    for name in [
        format!("{id}.png"),
        format!("{id}.svg"),
        "icon.png".to_string(),
        "icon.svg".to_string(),
        "logo.png".to_string(),
        "logo.svg".to_string(),
        "icon-large.png".to_string(),
    ] {
        let p = sqroot.join(&name);
        if p.is_file() {
            return Some((512, p));
        }
    }

    // 4. Fallback: largest PNG that isn't inside a tutorial/guide/
    //    doc/help subtree.
    let pngs = find_pngs(sqroot);
    let mut best: Option<(u64, PathBuf)> = None;
    for p in pngs {
        if is_in_guide_dir(&p) {
            continue;
        }
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
            best = Some((size, p));
        }
    }
    best.map(|(_, p)| (512, p))
}

/// Heuristic: is `path` inside a directory whose name suggests
/// tutorial / guide / documentation content?
fn is_in_guide_dir(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_lowercase();
    for marker in ["/guide/", "/guides/", "/tutorial/", "/tutorials/",
                   "/help/", "/docs/", "/doc/", "/screenshots/"] {
        if lower.contains(marker) {
            return true;
        }
    }
    false
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

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, body: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn fresh_squashfs() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("aim-icon-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn picks_diricon_first() {
        let sq = fresh_squashfs();
        // The actual M icon (small).
        let icon = sq.join("usr/share/icons/hicolor/512x512/apps/siyuan.png");
        touch(&icon, b"fake-m-icon");
        // A .DirIcon symlink at the root, pointing to the M icon.
        std::os::unix::fs::symlink(&icon, sq.join(".DirIcon")).unwrap();
        // A huge tutorial asset (would otherwise win by size).
        let tutorial = sq.join("resources/guide/big.png");
        touch(&tutorial, &vec![0u8; 100_000]);
        // A random top-level PNG.
        touch(&sq.join("siyuan.png"), b"top-level");

        let chosen = pick_best_icon(&sq, "siyuan").unwrap();
        // .DirIcon resolves to the M icon.
        assert_eq!(chosen.1.canonicalize().unwrap(), icon.canonicalize().unwrap());
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn picks_hicolor_when_no_diricon() {
        let sq = fresh_squashfs();
        let icon = sq.join("usr/share/icons/hicolor/512x512/apps/foo.png");
        touch(&icon, b"foo-512");
        // No .DirIcon. Tutorial files must NOT win.
        let tutorial = sq.join("resources/guide/big.png");
        touch(&tutorial, &vec![0u8; 100_000]);

        let chosen = pick_best_icon(&sq, "foo").unwrap();
        assert_eq!(chosen.1.canonicalize().unwrap(), icon.canonicalize().unwrap());
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn picks_largest_hicolor_size_available() {
        let sq = fresh_squashfs();
        // 256x256 only — no 512.
        let icon = sq.join("usr/share/icons/hicolor/256x256/apps/foo.png");
        touch(&icon, b"foo-256");

        let chosen = pick_best_icon(&sq, "foo").unwrap();
        assert_eq!(chosen.0, 256);
        assert_eq!(chosen.1.canonicalize().unwrap(), icon.canonicalize().unwrap());
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn picks_top_level_when_no_diricon_or_hicolor() {
        let sq = fresh_squashfs();
        touch(&sq.join("foo.png"), b"top-level-foo");

        let chosen = pick_best_icon(&sq, "foo").unwrap();
        assert_eq!(
            chosen.1.canonicalize().unwrap(),
            sq.join("foo.png").canonicalize().unwrap()
        );
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn fallback_skips_tutorial_directory() {
        let sq = fresh_squashfs();
        // A big tutorial asset.
        let tutorial = sq.join("resources/guide/big.png");
        touch(&tutorial, &vec![0u8; 100_000]);
        // A smaller real icon in an "icons" subdir.
        let icon = sq.join("usr/share/icons/foo.png");
        touch(&icon, b"real-icon");

        let chosen = pick_best_icon(&sq, "foo").unwrap();
        assert_eq!(chosen.1.canonicalize().unwrap(), icon.canonicalize().unwrap());
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn returns_none_when_no_pngs() {
        let sq = fresh_squashfs();
        touch(&sq.join("AppRun"), b"#!/bin/sh");
        assert!(pick_best_icon(&sq, "foo").is_none());
        std::fs::remove_dir_all(&sq).ok();
    }

    #[test]
    fn is_in_guide_dir_recognises_markers() {
        for marker in &["/guide/", "/guides/", "/tutorial/", "/tutorials/",
                        "/help/", "/docs/", "/doc/", "/screenshots/"] {
            let s = format!("/x{marker}foo.png");
            let path = Path::new(&s);
            assert!(is_in_guide_dir(path), "marker {marker} should be recognised");
        }
        let outside = Path::new("/usr/share/icons/hicolor/512x512/apps/foo.png");
        assert!(!is_in_guide_dir(outside));
    }
}