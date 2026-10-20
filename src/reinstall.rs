//! `aim reinstall <id>` — re-install an AppImage from the existing
//! cache, after a previous `aim remove` (without `--purge`).
//!
//! After a default `aim remove`, the cached AppImage binary survives
//! at `apps/<id@version>/AppImage`. `aim reinstall` wires that
//! surviving file back through the normal install pipeline, so:
//!
//! - The state record is recreated (integrated, per-app overrides
//!   start fresh — they were tied to the old record).
//! - The `.desktop` file and bin symlink are re-rendered.
//! - The icon is re-extracted (the file on disk is overwritten
//!   with a fresh copy from the AppImage's `.DirIcon`).
//! - Atomicity is preserved: `install::run` stages to a temp file
//!   and renames into place.
//!
//! Errors clearly when no cache is found — typically because the
//! user did `aim remove --purge` previously, or because the
//! target never had a cache (a hypothetical that's hard to reach).

use crate::error::{Error, Result};
use crate::install::{self, InstallRequest};
use crate::paths::{self, Layout};
use crate::state::Source;
use crate::staging::StagingDir;

#[derive(Debug, Clone, Default)]
pub struct ReinstallOpts {
    /// When true, skip rendering the `.desktop` file and bin
    /// symlink. The default mirrors `install`: integrate.
    pub no_integrate: bool,
}

/// Run `aim reinstall <target>`.
///
/// `target` is a state-record key (`id@version`) or a bare id; a
/// bare id is resolved against the existing cache (`apps/<id>@*/`).
pub fn run(target: &str, opts: ReinstallOpts, layout: &Layout) -> Result<()> {
    let key = resolve_cached_key(target, layout)?;
    let cache = layout.appimage_path(&key);
    if !cache.is_file() {
        return Err(Error::InvalidTarget {
            message: format!(
                "no cached AppImage for '{target}' (looked at {cache:?}); \
                 use `aim install <source>` with the original source path or URL"
            ),
        });
    }

    // Drive the install pipeline. `force` is implicit — the user
    // asked for this, so an existing state record (somehow left
    // over from a buggy run) should be overwritten rather than
    // rejected. `Source::Local` is the right tag because the
    // cached file is on this host.
    //
    // The cached file is always named `AppImage` (per install's
    // atomic cache layout). Identity parsing falls back to
    // filename stem when the AppImage has no embedded
    // `AppImage.yml`, so passing the cache path verbatim would
    // identify as `id="AppImage"`. Stage a copy with a meaningful
    // basename so identity falls back to the right id. When the
    // original key carries a version (`id@version`), include it
    // in the staged basename so identity can recover the version
    // via filename parsing — same shape as typical GitHub release
    // asset names. The `StagingDir` drops on return.
    let staging = StagingDir::new("aim-reinstall")?;
    let id = paths::id_for_key(&key);
    let version = key.strip_prefix(&format!("{id}@"));
    let staged_basename = match version {
        Some(v) if !v.is_empty() => format!("{id}-{v}.AppImage"),
        _ => format!("{id}.AppImage"),
    };
    let staged_source = staging.path().join(&staged_basename);
    std::fs::copy(&cache, &staged_source)?;

    install::run(
        InstallRequest {
            source: staged_source,
            source_kind: Source::Local {
                path: layout.appimage_path(&key).to_string_lossy().into_owned(),
            },
            name_override: None,
            no_integrate: opts.no_integrate,
            force: true,
        },
        layout,
    )?;
    Ok(())
}

/// Resolve `target` to a state-record key (`id@version`).
///
/// - If `target` already contains `@`, it's used as-is. The lookup
///   is against the filesystem (a state record may or may not
///   exist — `aim reinstall` works without one).
/// - Otherwise, `target` is treated as a bare id and matched
///   against `apps/<id>@*/` directories. With one match, use it;
///   with multiple matches, error and ask the user to disambiguate
///   with the version; with no matches, error.
fn resolve_cached_key(target: &str, layout: &Layout) -> Result<String> {
    if target.contains('@') {
        return Ok(target.to_string());
    }
    if !layout.apps_dir.is_dir() {
        return Err(Error::InvalidTarget {
            message: format!("no cache for '{target}' (apps dir missing)"),
        });
    }

    // A bare id matches two forms: an `apps/<id>/` directory
    // (when the AppImage had no identifiable version) and any
    // `apps/<id>@<version>/` directories (when it did). With one
    // match we use it; with multiple we ask the user to
    // disambiguate.
    let mut matches: Vec<String> = Vec::new();
    if layout.apps_dir.join(target).is_dir() {
        matches.push(target.to_string());
    }
    let prefix = format!("{target}@");
    for entry in std::fs::read_dir(&layout.apps_dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) {
            matches.push(name);
        }
    }
    match matches.len() {
        0 => Err(Error::InvalidTarget {
            message: format!(
                "no cached AppImage for '{target}'; use \
                 `aim install <source>` with the original source"
            ),
        }),
        1 => Ok(matches.remove(0)),
        _ => Err(Error::InvalidTarget {
            message: format!(
                "ambiguous target '{target}' matches {matches:?}; \
                 disambiguate with the version, e.g. 'aim reinstall {prefix}…'"
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install;
    use crate::paths;
    use crate::state::{AppRecord, Source, State};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_layout() -> (Layout, std::path::PathBuf) {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("aim-reinstall-{nanos}"));
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
    fn resolve_cached_key_with_at_returns_verbatim() {
        let (layout, home) = fresh_layout();
        let key = resolve_cached_key("foo@1.0.0", &layout).unwrap();
        assert_eq!(key, "foo@1.0.0");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn resolve_cached_key_with_bare_id_finds_single_match() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", true);
        // Fake AppImage has no version, so install creates apps/Foo/
        // (no @version segment). Bare-id lookup should find it.
        let key = resolve_cached_key("Foo", &layout).unwrap();
        assert_eq!(key, "Foo");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn resolve_cached_key_with_versioned_match_prefers_at_form() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", true);
        // Manually plant a versioned entry to force the @ form.
        let v = layout.apps_dir.join("Foo@1.0.0");
        fs::create_dir_all(&v).unwrap();
        fs::write(v.join("AppImage"), b"x").unwrap();
        // Now both `Foo` and `Foo@1.0.0` exist → ambiguous.
        let err = resolve_cached_key("Foo", &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn resolve_cached_key_with_no_match_errors() {
        let (layout, home) = fresh_layout();
        let err = resolve_cached_key("nonexistent", &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn resolve_cached_key_with_multiple_matches_errors() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", true);
        // Plant a second matching cache dir to force ambiguity.
        let extra = layout.apps_dir.join("Foo@0.0.1");
        fs::create_dir_all(&extra).unwrap();
        let err = resolve_cached_key("Foo", &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn reinstall_after_default_remove_recovers_state() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", false);

        // Identify the record's key so we can verify reinstall
        // uses the same cache file.
        let state = State::load(&layout.state_file).unwrap();
        let (key, _record) = state.lookup("Foo").unwrap();
        let cache = layout.appimage_path(&key);
        let cache_bytes = fs::read(&cache).unwrap();

        // Simulate `aim remove <Foo>` (default, no --purge).
        crate::remove::run(&["Foo".to_string()], false, &layout).unwrap();
        assert!(cache.is_file(), "cache should survive default remove");
        let after_remove = State::load(&layout.state_file).unwrap();
        assert!(after_remove.apps.is_empty(), "state record should be gone");

        // Reinstall from the surviving cache.
        run("Foo", ReinstallOpts::default(), &layout).unwrap();

        let after = State::load(&layout.state_file).unwrap();
        let (_, _rec) = after.lookup("Foo").unwrap();
        assert_eq!(after.apps.len(), 1);
        // Cache still has the same content.
        assert_eq!(fs::read(&cache).unwrap(), cache_bytes);
        // And integration came back.
        assert!(layout.desktop_file(&key).is_file());
        assert!(layout.bin_link(&key).is_symlink());

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn reinstall_with_no_cache_errors_clearly() {
        let (layout, home) = fresh_layout();
        // No install done — apps dir exists but is empty.
        let err = run("never-installed", ReinstallOpts::default(), &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn reinstall_after_purge_fails() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", true);
        let state = State::load(&layout.state_file).unwrap();
        let (key, _) = state.lookup("Foo").unwrap();
        crate::remove::run(&["Foo".to_string()], true, &layout).unwrap();
        assert!(!layout.appimage_path(&key).exists());
        let err = run("Foo", ReinstallOpts::default(), &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    // Suppress an unused-import warning in `clippy --all`.
    #[allow(dead_code)]
    fn _ensure_paths_compiles() -> paths::Layout {
        paths::Layout::with_home(std::path::PathBuf::from("/tmp"))
    }

    // Reference AppRecord so the import stays put for future
    // expansion (e.g. preserving overrides across reinstall).
    #[allow(dead_code)]
    fn _app_record_marker() -> AppRecord {
        AppRecord {
            id: String::new(),
            version: None,
            sha256: String::new(),
            size_bytes: 0,
            integrated: false,
            source: Source::Unknown,
            icon_size_px: None,
            icon_path: None,
            env: Default::default(),
            extra_exec_args: vec![],
            comment: None,
            categories: vec![],
            mime_types: vec![],
            installed_at_secs: 0,
        }
    }
}