//! `aim integrate` / `aim unintegrate`.
//!
//! These two ops are the **toggle** for whether an already-installed
//! AppImage has a `.desktop` file (so it shows up in `rofi`/Gnome
//! search) and a bin symlink (so the user can type `foo` in a shell).
//! They never touch the cached AppImage: the file at
//! `~/.local/share/aim/apps/<id@version>/AppImage` stays put.
//!
//! Both ops are idempotent. Re-running `aim integrate <id>` when it's
//! already integrated rewrites the `.desktop` to pick up any
//! `info --edit` changes.

use crate::desktop;
use crate::error::{Error, Result};
use crate::paths::Layout;
use crate::state::State;
use crate::staging::{atomic_write, remove_any, symlink};

/// Toggle the integration flag on. Idempotent.
pub fn integrate(target: &str, layout: &Layout) -> Result<()> {
    let mut state = State::load(&layout.state_file)?;
    let (key, mut record) = state.lookup(target)?;

    // The bin link and the cache must point at each other through
    // `key`, not the *id*, because two installs of the same id at
    // different versions would collide otherwise.
    let cache_path = layout.appimage_path(&key);
    if !cache_path.exists() {
        // The cached AppImage vanished. Refuse rather than leave a
        // dangling bin symlink behind.
        return Err(Error::InvalidTarget {
            message: format!(
                "cannot integrate '{key}': cached AppImage is missing at {}",
                cache_path.display()
            ),
        });
    }

    let desktop_path = layout.desktop_file(&key);
    let bin_link = layout.bin_link(&key);

    // Flip the integration flag BEFORE rendering so the rendered
    // `.desktop` carries `AIM_INTEGRATED=1` in its env shim. The
    // renderer derives `AIM_INTEGRATED` from `integrated`.
    record.integrated = true;
    let body = desktop::render(&record, &cache_path);
    atomic_write(&desktop_path, body.as_bytes())?;

    symlink(&cache_path, &bin_link)?;

    state.apps.insert(key.clone(), record);
    state.save(&layout.state_file)?;
    Ok(())
}

/// Toggle the integration flag off. Idempotent. Removes `.desktop`
/// and the bin symlink but leaves the cache + state entry alone.
pub fn unintegrate(target: &str, layout: &Layout) -> Result<()> {
    let mut state = State::load(&layout.state_file)?;
    let (key, mut record) = state.lookup(target)?;

    remove_any(&layout.desktop_file(&key));
    remove_any(&layout.bin_link(&key));

    record.integrated = false;

    state.apps.insert(key.clone(), record);
    state.save(&layout.state_file)?;
    Ok(())
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
        let home = std::env::temp_dir().join(format!("aim-int-{nanos}"));
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

    #[test]
    fn integrate_then_unintegrate_round_trip() {
        let (layout, home) = fresh_layout();
        let src = home.join("Foo.AppImage");
        fake_appimage(&src);

        // Start unintegrated so we can re-integrate later.
        install::run(
            InstallRequest {
                source: src.clone(),
                source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
                name_override: None,
                no_integrate: true,
                force: false,
            },
            &layout,
        )
        .unwrap();

        // `unintegrated` initially.
        let state = State::load(&layout.state_file).unwrap();
        let (key, _) = state.lookup("Foo").unwrap();
        assert!(!state.apps[&key].integrated);
        assert!(!layout.desktop_file(&key).exists());

        // Integrate.
        integrate("Foo", &layout).unwrap();
        let state = State::load(&layout.state_file).unwrap();
        let rec = &state.apps[&key];
        assert!(rec.integrated);
        assert!(layout.desktop_file(&key).is_file());
        assert!(layout.bin_link(&key).is_symlink());

        // Unintegrate.
        unintegrate("Foo", &layout).unwrap();
        let state = State::load(&layout.state_file).unwrap();
        let rec = &state.apps[&key];
        assert!(!rec.integrated);
        assert!(!layout.desktop_file(&key).exists());
        assert!(!layout.bin_link(&key).exists());

        // Cache survived.
        assert!(layout.appimage_path(&key).is_file());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn integrate_idempotent() {
        let (layout, home) = fresh_layout();
        let src = home.join("Bar.AppImage");
        fake_appimage(&src);

        install::run(
            InstallRequest {
                source: src.clone(),
                source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
                name_override: None,
                no_integrate: true,
                force: false,
            },
            &layout,
        )
        .unwrap();

        integrate("Bar", &layout).unwrap();
        integrate("Bar", &layout).unwrap();
        integrate("Bar", &layout).unwrap();

        let state = State::load(&layout.state_file).unwrap();
        let (key, rec) = state.lookup("Bar").unwrap();
        assert!(rec.integrated);
        assert!(layout.bin_link(&key).is_symlink());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn integrate_unknown_target_errors() {
        let (layout, home) = fresh_layout();
        let err = integrate("nope", &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::not_installed");
        fs::remove_dir_all(&home).ok();
    }
}
