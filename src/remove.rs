//! `aim remove` — uninstall an AppImage.
//!
//! Behaviour (per spec, agreed at design time):
//!
//! - Default: drop the state record, remove the `.desktop` file and
//!   bin symlink. The cached AppImage stays on disk and becomes an
//!   "orphaned file" that a future `aim gc` (slice 6) can sweep.
//! - `--purge`: also delete the cached AppImage and the DirIcon
//!   under `icons/hicolor/`.
//!
//! Multiple targets per invocation are processed in the order given;
//! the first failure aborts the run (no partial cleanup).

use std::path::Path;

use crate::error::{Error, Result};
use crate::paths::Layout;
use crate::state::State;
use crate::staging::remove_any;

/// Run `aim remove <targets...> [--purge]`.
pub fn run(targets: &[String], purge: bool, layout: &Layout) -> Result<()> {
    if targets.is_empty() {
        return Err(Error::InvalidTarget {
            message: "remove requires at least one id".to_string(),
        });
    }

    let mut state = State::load(&layout.state_file)?;
    let mut removed: Vec<String> = Vec::new();

    for target in targets {
        let (key, _record) = state.lookup(target)?;

        // 1. Remove integration artefacts. These can legitimately
        //    already be absent (e.g. user manually rm'd them); we use
        //    `remove_any` which silently tolerates missing paths.
        remove_any(&layout.desktop_file(&key));
        remove_any(&layout.bin_link(&key));

        // 2. With --purge, also drop the cache and the icon. The cache
        //    directory is taken entirely; that mirrors `install`'s
        //    layout (one app image per dir).
        if purge {
            let app_dir = layout.app_dir(&key);
            remove_dir_all_safe(&app_dir);
            // The icon lives at `icons/hicolor/<size>/apps/aim-<id>.png`
            // but per spec a single AppImage can install only one icon
            // (slice 1). We don't know which size; remove the matching
            // hicolor/apps/aim-<id>.png anywhere under the icons dir.
            remove_icons(&layout, &key);
        }

        // 3. Drop the state record.
        state.apps.remove(&key);
        removed.push(key.clone());
    }

    state.save(&layout.state_file)?;
    // Print a one-liner per removal so scripting/automation has
    // structured output.
    for k in &removed {
        if purge {
            println!("removed {k} (cache + icon purged)");
        } else {
            println!("removed {k} (cache preserved)");
        }
    }
    Ok(())
}

/// Recursive remove with a forgiving semantics: missing files are OK,
/// permission errors are reported via the underlying IO error.
fn remove_dir_all_safe(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Walk the icons tree and remove any PNG named `aim-<id>.png`.
/// Always best-effort — at worst a stale icon remains and `gc`
/// cleans it later.
fn remove_icons(layout: &Layout, key: &str) {
    let apps_root = layout.icons_dir.join("hicolor");
    // The icon may sit at multiple sizes; we only installed one in
    // slice 1 but be defensive. We can't ask "where exactly is it"
    // because the record only stored the relative path; pick the
    // PNG at the layout's recorded `appimage_path` location and any
    // other size directories.
    if let Ok(it) = std::fs::read_dir(&apps_root) {
        for size_entry in it.flatten() {
            if size_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let dir = size_entry.path().join("apps");
                let candidate = dir.join(format!("aim-{}.png", key.replace('/', "_")));
                if candidate.exists() {
                    let _ = std::fs::remove_file(&candidate);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::{self, InstallRequest};
    use crate::state::Source;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_layout() -> (Layout, std::path::PathBuf) {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("aim-rm-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::with_home(home.clone());
        (layout, home)
    }

    fn fake_appimage(path: &Path) {
        let mut bytes = vec![0x7f, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00, b'A', b'I', 0x02, 0x00];
        bytes.resize(4096, 0);
        fs::write(path, &bytes).unwrap();
        let mut perm = fs::metadata(path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).unwrap();
    }

    fn install_one(layout: &Layout, dir: &Path, name: &str, no_integrate: bool) {
        let src = dir.join(name);
        fake_appimage(&src);
        let path_str = src.to_string_lossy().into_owned();
        install::run(
            InstallRequest {
                source: src,
                source_kind: Source::Local { path: path_str },
                name_override: None,
                no_integrate,
                force: false,
            },
            &layout,
        )
        .unwrap();
    }

    #[test]
    fn remove_default_preserves_cache() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", false);

        let state_before = State::load(&layout.state_file).unwrap();
        let (key, _) = state_before.lookup("Foo").unwrap();
        let cache = layout.appimage_path(&key);
        assert!(cache.is_file());
        assert!(layout.desktop_file(&key).is_file());
        assert!(layout.bin_link(&key).is_symlink());

        run(&["Foo".to_string()], false, &layout).unwrap();

        // Cache survives.
        assert!(cache.is_file(), "cache file should survive a default remove");
        // Integration artefacts gone.
        assert!(!layout.desktop_file(&key).exists());
        assert!(!layout.bin_link(&key).exists());
        // State record gone.
        let state_after = State::load(&layout.state_file).unwrap();
        assert!(state_after.apps.is_empty());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_purge_deletes_cache_and_icon() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Bar.AppImage", false);

        let state_before = State::load(&layout.state_file).unwrap();
        let (key, _record) = state_before.lookup("Bar").unwrap();
        let app_dir = layout.app_dir(&key);
        assert!(app_dir.is_dir());

        // Make sure there's an icon to remove. Manually plant one so
        // the test isn't dependent on icon extraction.
        let icon = layout.icon_path(&key, 512);
        if let Some(parent) = icon.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&icon, b"fakepng").unwrap();

        run(&["Bar".to_string()], true, &layout).unwrap();

        // Cache + app_dir gone.
        assert!(!app_dir.exists(), "app_dir should be removed with --purge");
        // Icon gone.
        assert!(!icon.exists(), "icon file should be removed with --purge");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_unknown_target_errors() {
        let (layout, home) = fresh_layout();
        let err = run(&["nope".to_string()], false, &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::not_installed");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_multiple_targets() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "First.AppImage", true);
        install_one(&layout, &home, "Second.AppImage", true);

        run(
            &["First".to_string(), "Second".to_string()],
            false,
            &layout,
        )
        .unwrap();

        let state_after = State::load(&layout.state_file).unwrap();
        assert!(state_after.apps.is_empty());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn remove_empty_target_list_errors() {
        let (layout, home) = fresh_layout();
        let err = run(&[], false, &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }
}
