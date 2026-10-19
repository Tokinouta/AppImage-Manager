//! `aim gc` — garbage-collect orphan files left behind by other
//! commands. Always prints what it would do; in non-TTY environments
//! (CI, scripts) it requires `--yes` to actually delete.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crate::error::Result;
use crate::health::{self, IssueKind};
use crate::paths::Layout;

/// Options accepted by [`run`].
#[derive(Debug, Clone, Default)]
pub struct GcOpts {
    pub yes: bool,
    pub dry_run: bool,
    pub json: bool,
}

/// Run `aim gc`. Returns a count of items removed (0 if dry-run,
/// non-interactive, or empty).
pub fn run(opts: GcOpts, layout: &Layout) -> Result<usize> {
    let issues = health::scan(layout)?;
    let recoverable: Vec<_> = issues
        .into_iter()
        .filter(|i| i.kind.is_recoverable())
        .collect();

    if opts.json {
        // For JSON, include the cleanup paths so callers can act on
        // the same data.
        #[derive(serde::Serialize)]
        struct GcItem {
            kind: &'static str,
            id: Option<String>,
            path: String,
            cleanup_paths: Vec<String>,
        }
        let items: Vec<GcItem> = recoverable
            .iter()
            .map(|i| GcItem {
                kind: i.kind.label(),
                id: i.id.clone(),
                path: i.path.display().to_string(),
                cleanup_paths: i
                    .cleanup_paths()
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(0);
    }

    if recoverable.is_empty() {
        println!("aim gc: nothing to do");
        return Ok(0);
    }

    println!("aim gc: will remove {} item(s):", recoverable.len());
    for issue in &recoverable {
        let id = issue.id.as_deref().unwrap_or("?");
        let path = issue.path.display();
        let hint = if matches!(issue.kind, IssueKind::OrphanCache) {
            "  (+ parent dir)"
        } else {
            ""
        };
        println!("  [{:>17}]  {:<12}  {}{}", issue.kind.label(), id, path, hint);
    }

    if opts.dry_run {
        println!("(dry run; pass --yes to actually delete)");
        return Ok(0);
    }

    if !opts.yes {
        if !io::stdin().is_terminal() {
            println!();
            println!("refusing to delete in non-interactive mode without --yes");
            return Ok(0);
        }
        print!("\nProceed? [y/N] ");
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            println!("aborted");
            return Ok(0);
        }
    }

    // Walk all cleanup paths. We do per-issue removal because each
    // Issue knows its own list (file + parent dir for OrphanCache).
    let mut removed = 0usize;
    for issue in &recoverable {
        for p in issue.cleanup_paths() {
            if try_remove(p.as_path()) {
                removed += 1;
            }
        }
    }
    println!("aim gc: removed {removed} path(s)");
    Ok(removed)
}

fn try_remove(p: &Path) -> bool {
    if !p.exists() && !p.is_symlink() {
        return false;
    }
    let r = if p.is_dir() && !p.is_symlink() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    if let Err(e) = r {
        eprintln!("  warn: could not remove {}: {}", p.display(), e);
        return false;
    }
    true
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
        let home = std::env::temp_dir().join(format!("aim-gc-{nanos}"));
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
                no_integrate: false,
                force: false,
            },
            &layout,
        )
        .unwrap();
    }

    fn plant_orphan_cache(layout: &Layout) -> std::path::PathBuf {
        let orphan = layout.apps_dir.join("zombie");
        fs::create_dir_all(&orphan).unwrap();
        fake_appimage(&orphan.join("AppImage"));
        orphan
    }

    fn plant_orphan_desktop(layout: &Layout) -> std::path::PathBuf {
        let apps = layout.apps_root_dir.join("applications");
        fs::create_dir_all(&apps).unwrap();
        let p = apps.join("aim-zombie.desktop");
        fs::write(&p, b"").unwrap();
        p
    }

    #[test]
    fn clean_state_does_nothing() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        let n = run(GcOpts { yes: true, ..Default::default() }, &layout).unwrap();
        assert_eq!(n, 0);
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn yes_removes_orphan_cache() {
        let (layout, home) = fresh_layout();
        let orphan = plant_orphan_cache(&layout);
        let n = run(GcOpts { yes: true, ..Default::default() }, &layout).unwrap();
        assert!(n >= 2, "expected at least file + parent dir removed, got {n}");
        assert!(!orphan.exists(), "orphan dir should be gone");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn yes_removes_orphan_desktop() {
        let (layout, home) = fresh_layout();
        let desktop = plant_orphan_desktop(&layout);
        let n = run(GcOpts { yes: true, ..Default::default() }, &layout).unwrap();
        assert_eq!(n, 1);
        assert!(!desktop.exists());
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn dry_run_removes_nothing() {
        let (layout, home) = fresh_layout();
        let orphan = plant_orphan_cache(&layout);
        let n = run(GcOpts { dry_run: true, yes: false, json: false }, &layout).unwrap();
        assert_eq!(n, 0);
        assert!(orphan.exists(), "dry run must not delete");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn no_yes_in_non_tty_aborts() {
        // cargo test runs without a TTY on stdin, so this is the
        // exact "user ran `aim gc` from a script without --yes"
        // scenario.
        let (layout, home) = fresh_layout();
        let orphan = plant_orphan_cache(&layout);
        let n = run(GcOpts { yes: false, dry_run: false, json: false }, &layout).unwrap();
        assert_eq!(n, 0);
        assert!(orphan.exists(), "non-TTY without --yes must not delete");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn json_lists_recoverable_items() {
        let (layout, home) = fresh_layout();
        plant_orphan_cache(&layout);
        // We can't easily capture stdout here, but we can confirm
        // the JSON path doesn't error and serializes correctly.
        let _ = run(GcOpts { json: true, ..Default::default() }, &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }
}
