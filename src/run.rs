//! `aim run` — launch an installed AppImage, optionally forwarding
//! arguments to it.
//!
//! The CLI uses Unix `exec()` so the current process *becomes* the
//! AppImage: signals (Ctrl-C, SIGTERM) propagate naturally, exit
//! codes pass through without interpretation. The library `run`
//! function forks + waits instead, returning the exit code so unit
//! tests don't replace themselves.

use std::io;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

use crate::error::{Error, Result};
use crate::paths::Layout;
use crate::state::State;

/// Run an installed AppImage and return its exit code.
///
/// Spawns the cached AppImage, inherits the terminal stdio, and
/// waits for it to complete. Returns `Ok(exit_code)`; the exit code
/// of the child is preserved (with `1` substituted when the OS
/// reports a non-numeric status, which is rare on Unix).
pub fn run(target: &str, app_args: &[String], layout: &Layout) -> Result<i32> {
    let cache = cache_path(target, layout)?;
    let status = run_command(&cache, app_args)?;
    Ok(status_code(&status))
}

/// Resolve `target` against the on-disk state and return the cache
/// AppImage path. Errors if the record is missing or the file isn't
/// there.
pub fn cache_path(target: &str, layout: &Layout) -> Result<PathBuf> {
    let state = State::load(&layout.state_file)?;
    let (key, _record) = state.lookup(target)?;
    let cache = layout.appimage_path(&key);
    if !cache.exists() {
        return Err(Error::InvalidTarget {
            message: format!(
                "cached AppImage for '{target}' is missing at {}",
                cache.display()
            ),
        });
    }
    Ok(cache)
}

fn run_command(cache: &std::path::Path, app_args: &[String]) -> Result<ExitStatus> {
    let mut cmd = Command::new(cache);
    cmd.args(app_args);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    cmd.status().map_err(|source| Error::Path {
        path: cache.to_path_buf(),
        source,
    })
}

fn status_code(s: &ExitStatus) -> i32 {
    s.code().unwrap_or(1)
}

/// Variants of [`Command`] that the binary CLI uses when it wants
/// `exec()` semantics — running the cached AppImage *as* the current
/// process, replacing `aim` so signals and exit codes propagate
/// without an interpreter in between.
pub fn exec_replace(target: &str, app_args: &[String], layout: &Layout) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let cache = cache_path(target, layout)?;
    let mut cmd = Command::new(&cache);
    cmd.args(app_args);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());
    // `exec()` only returns when the binary couldn't be exec'd. All
    // other "errors" (ENOENT etc.) are reported by the OS in the
    // parent process before exec runs.
    let err = cmd.exec();
    Err(io::Error::from(err).into())
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
        let home = std::env::temp_dir().join(format!("aim-run-{nanos}"));
        fs::create_dir_all(&home).unwrap();
        let layout = Layout::with_home(home.clone());
        (layout, home)
    }

    /// Build an AppImage-like file that exits with `code` when run.
    /// Uses a shell-script-style file with the Type-2 magic bytes so
    /// `aim install` accepts it.
    fn fake_executable(path: &Path, exit_code: u8) {
        let mut bytes = vec![0x7f, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00, b'A', b'I', 0x02, 0x00];
        bytes.resize(4096, 0);
        // Overwrite the body with a `#!/bin/sh\n` script that runs,
        // since the test isn't really an ELF. We need a real
        // executable shell script.
        let body = format!("#!/bin/sh\nexit {exit_code}\n");
        let mut bytes = body.into_bytes();
        // Pad to a non-trivial size so SHA-256 is unique per test.
        while bytes.len() < 64 {
            bytes.push(b'\n');
        }
        fs::write(path, &bytes).unwrap();
        let mut perm = fs::metadata(path).unwrap().permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).unwrap();
    }

    fn install_one(layout: &Layout, dir: &Path, name: &str, exit_code: u8) {
        let src = dir.join(name);
        fake_executable(&src, exit_code);
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
    fn run_propagates_zero_exit() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Good.AppImage", 0);
        let code = run("Good", &[], &layout).unwrap();
        assert_eq!(code, 0);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn run_propagates_nonzero_exit() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Bad.AppImage", 42);
        let code = run("Bad", &[], &layout).unwrap();
        assert_eq!(code, 42);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn run_unknown_target_errors() {
        let (layout, home) = fresh_layout();
        let err = run("nope", &[], &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::not_installed");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn run_cache_missing_errors_with_invalid_target() {
        // Install and then delete the cached file behind the state's
        // back.
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Ghost.AppImage", 0);
        // Wipe the cache.
        let cache = cache_path("Ghost", &layout).unwrap();
        std::fs::remove_file(&cache).unwrap();

        let err = run("Ghost", &[], &layout).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        fs::remove_dir_all(&home).ok();
    }
}
