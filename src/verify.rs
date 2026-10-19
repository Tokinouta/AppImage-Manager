//! `aim verify` — re-hash the cached AppImage and compare against
//! the hash recorded in `state.toml`. Detects bit-rot, partial
//! copies, and tampering.

use serde::Serialize;

use crate::error::{Error, Result};
use crate::hash;
use crate::paths::Layout;
use crate::state::State;

#[derive(Debug, Clone, Default)]
pub struct VerifyOpts {
    pub json: bool,
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub key: String,
    pub recorded_sha256: String,
    pub actual_sha256: String,
    pub recorded_size_bytes: u64,
    pub actual_size_bytes: u64,
    pub matched: bool,
}

pub fn run(target: &str, opts: VerifyOpts, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let (key, record) = state.lookup(target)?;
    let cache = layout.appimage_path(&key);
    if !cache.exists() {
        return Err(Error::InvalidTarget {
            message: format!("cached AppImage missing at {}", cache.display()),
        });
    }

    let (actual, actual_size) = hash::hash_file(&cache)?;
    let matched = actual == record.sha256;

    let report = VerifyReport {
        key: key.clone(),
        recorded_sha256: record.sha256.clone(),
        actual_sha256: actual,
        recorded_size_bytes: record.size_bytes,
        actual_size_bytes: actual_size,
        matched,
    };

    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if matched {
        println!("OK: {} (sha256 matches)", key);
    } else {
        println!("FAILED: {} hash mismatch", key);
        println!("  recorded: {}", report.recorded_sha256);
        println!("  actual:   {}", report.actual_sha256);
    }

    if matched {
        Ok(())
    } else {
        Err(Error::InvalidTarget {
            message: format!("hash mismatch for {key}"),
        })
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
        let home = std::env::temp_dir().join(format!("aim-verify-{nanos}"));
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

    fn install_one(layout: &Layout, dir: &Path, name: &str) {
        let src = dir.join(name);
        fake_appimage(&src);
        let path_str = src.to_string_lossy().into_owned();
        install::run(
            InstallRequest {
                source: src,
                source_kind: Source::Local { path: path_str },
                name_override: None,
                no_integrate: true,
                force: false,
            },
            &layout,
        )
        .unwrap();
    }

    #[test]
    fn verify_passes_on_fresh_install() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        // Fresh install should pass verify.
        run("Foo", VerifyOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn verify_detects_tampered_cache() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        // Tamper with the cached file: append some bytes.
        let cache = layout.appimage_path("Foo");
        let mut f = std::fs::OpenOptions::new().append(true).open(&cache).unwrap();
        use std::io::Write;
        f.write_all(b"tampered").unwrap();
        drop(f);

        let err = run("Foo", VerifyOpts::default(), &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn verify_errors_when_cache_missing() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        let cache = layout.appimage_path("Foo");
        std::fs::remove_file(&cache).unwrap();
        let err = run("Foo", VerifyOpts::default(), &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn verify_json_shape() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        // We don't capture stdout here; instead, build the same
        // report struct directly and assert the fields exist.
        let state = State::load(&layout.state_file).unwrap();
        let (key, rec) = state.lookup("Foo").unwrap();
        let (actual, _size) = hash::hash_file(&layout.appimage_path(&key)).unwrap();
        let report = VerifyReport {
            key: key.clone(),
            recorded_sha256: rec.sha256.clone(),
            actual_sha256: actual,
            recorded_size_bytes: rec.size_bytes,
            actual_size_bytes: rec.size_bytes,
            matched: true,
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("\"matched\":true"));
        fs::remove_dir_all(&home).ok();
    }
}
