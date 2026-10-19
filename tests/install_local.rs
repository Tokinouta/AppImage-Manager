//! Integration tests for `aim install` against a local path.
//!
//! These tests don't shell out to a real binary; they call the
//! library's `install::run` directly with a `Layout::with_home` so the
//! user's real `$HOME` is never touched.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use aim::install::{self, InstallRequest, InstallResult};
use aim::paths::Layout;
use aim::state::{Source, State};

fn fixture_dir() -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    dir.push(format!("aim-test-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a synthetic Type-2 AppImage: ELF magic + AI\x02 marker + a
/// trailing marker comment so `--appimage-extract` has a chance of
/// being meaningful (though it'll fail; that's fine — we just need
/// detect_kind to recognise the file).
fn fake_type2_appimage(path: &std::path::Path) {
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00];
    bytes.extend_from_slice(b"AI\x02");
    bytes.resize(4096, 0);
    fs::write(path, &bytes).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perm = fs::metadata(path).unwrap().permissions();
    perm.set_mode(0o755);
    fs::set_permissions(path, perm).unwrap();
}

#[test]
fn install_local_path_end_to_end() {
    let home = fixture_dir();
    let layout = Layout::with_home(home.clone());

    let src = home.join("vikunja-test.AppImage");
    fake_type2_appimage(&src);

    let res: InstallResult = install::run(
        InstallRequest {
            source: src.clone(),
            source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
            name_override: None,
            no_integrate: false,
            force: false,
        },
        &layout,
    )
    .expect("install should succeed");

    // The cache file exists.
    let cache = layout.appimage_path(&res.record.key());
    assert!(cache.is_file(), "cached AppImage should exist at {cache:?}");

    // The original source file is preserved (we copy, not move).
    assert!(src.is_file(), "source file must not be moved");

    // The state file has one entry.
    let state_after = State::load(&layout.state_file).unwrap();
    let state_applied = state_after.apps.get(&res.record.key()).expect("state should contain the record");
    assert_eq!(state_applied.id, res.record.id);
    assert_eq!(state_applied.sha256, res.record.sha256);

    // The .desktop file matches the record.
    let desktop = layout.desktop_file(&res.record.key());
    assert!(desktop.is_file(), ".desktop file should exist at {desktop:?}");
    let body = fs::read_to_string(&desktop).unwrap();
    assert!(body.contains("[Desktop Entry]"));
    assert!(body.contains(&format!("Name={}", res.record.id)));
    assert!(body.contains(&cache.to_string_lossy().to_string()));

    // The bin symlink points at the cache.
    let link = layout.bin_link(&res.record.key());
    assert!(link.is_symlink(), "bin symlink should be a symlink");
    let target = fs::read_link(&link).unwrap();
    assert_eq!(target, cache);

    // The install result reports `integrated = true` because we did
    // not pass `--no-integrate`.
    assert!(res.integrated);

    fs::remove_dir_all(&home).ok();
}

#[test]
fn install_already_present_errors_without_force() {
    let home = fixture_dir();
    let layout = Layout::with_home(home.clone());

    let src = home.join("first.AppImage");
    fake_type2_appimage(&src);

    let req = InstallRequest {
        source: src.clone(),
        source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
        name_override: None,
        no_integrate: true, // skip integrate to speed up + avoid desktop-write flakiness
        force: false,
    };
    install::run(req.clone(), &layout).expect("first install");

    let err = install::run(req, &layout).unwrap_err();
    assert_eq!(err.code(), "aim::error::already_installed");

    fs::remove_dir_all(&home).ok();
}

#[test]
fn install_force_overwrites_existing() {
    let home = fixture_dir();
    let layout = Layout::with_home(home.clone());

    let src = home.join("dup.AppImage");
    fake_type2_appimage(&src);

    let req = || InstallRequest {
        source: src.clone(),
        source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
        name_override: None,
        no_integrate: true,
        force: false,
    };
    install::run(req(), &layout).unwrap();
    let req = || InstallRequest {
        source: src.clone(),
        source_kind: Source::Local { path: src.to_string_lossy().into_owned() },
        name_override: None,
        no_integrate: true,
        force: true,
    };
    install::run(req(), &layout).expect("force should succeed");

    fs::remove_dir_all(&home).ok();
}

#[test]
fn install_no_integrate_skips_desktop_and_symlink() {
    let home = fixture_dir();
    let layout = Layout::with_home(home.clone());

    let src = home.join("plain.AppImage");
    fake_type2_appimage(&src);

    let res = install::run(
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

    let desktop = layout.desktop_file(&res.record.key());
    assert!(!desktop.exists(), ".desktop should NOT exist when --no-integrate");
    let link = layout.bin_link(&res.record.key());
    assert!(!link.exists(), "bin symlink should NOT exist when --no-integrate");

    fs::remove_dir_all(&home).ok();
}
