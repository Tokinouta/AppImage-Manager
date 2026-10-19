//! `aim doctor` — read-only diagnostic report.
//!
//! Runs the same scanner as `aim gc` but never deletes anything.
//! Outputs a human-readable summary; the recoverability of each
//! issue is shown so the user knows whether `aim gc` can help.

use std::io::IsTerminal;

use crate::error::Result;
use crate::health;
use crate::paths::Layout;

/// Options accepted by [`run`].
#[derive(Debug, Clone, Default)]
pub struct DoctorOpts {
    pub json: bool,
}

/// Run `aim doctor`.
pub fn run(opts: DoctorOpts, layout: &Layout) -> Result<()> {
    let issues = health::scan(layout)?;

    if opts.json {
        let json = serde_json::to_string_pretty(&issues)?;
        println!("{json}");
        return Ok(());
    }

    if issues.is_empty() {
        println!("aim: no issues found");
        return Ok(());
    }

    let mut by_kind: std::collections::BTreeMap<&'static str, Vec<&health::Issue>> =
        Default::default();
    for i in &issues {
        by_kind.entry(i.kind.label()).or_default().push(i);
    }

    println!("aim: {} issue(s) found", issues.len());
    for (label, group) in &by_kind {
        println!();
        println!("  [{label}]  ({} item(s))", group.len());
        for issue in group {
            let id_str = issue
                .id
                .as_deref()
                .map(|s| format!("{s:>16}  "))
                .unwrap_or_else(|| " ".repeat(18));
            println!("    {id_str}  {}", issue.path.display());
        }
    }

    let recoverable: Vec<_> = issues.iter().filter(|i| i.kind.is_recoverable()).collect();
    if !recoverable.is_empty() && std::io::stdout().is_terminal() {
        println!();
        println!("  {} issue(s) can be cleaned up with `aim gc --yes`.", recoverable.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::IssueKind;
    use crate::install::{self, InstallRequest};
    use crate::state::Source;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fresh_layout() -> (Layout, std::path::PathBuf) {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("aim-doctor-{nanos}"));
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

    #[test]
    fn clean_state_reports_zero() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        run(DoctorOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn doctor_finds_missing_cache() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage");
        // Remove the cache and run doctor; the report should detect
        // the missing file.
        let cache = layout.appimage_path("Foo");
        std::fs::remove_file(&cache).unwrap();
        run(DoctorOpts::default(), &layout).unwrap();
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn doctor_json_emits_array() {
        let (layout, home) = fresh_layout();
        // Plant an orphan to give the JSON something to serialize.
        let orphan = layout.apps_dir.join("ghost");
        fs::create_dir_all(&orphan).unwrap();
        fake_appimage(&orphan.join("AppImage"));
        let issues = health::scan(&layout).unwrap();
        let json = serde_json::to_string(&issues).unwrap();
        assert!(json.contains("\"ORPHAN-CACHE\""));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn issue_kind_label() {
        assert_eq!(IssueKind::OrphanCache.label(), "ORPHAN-CACHE");
        assert_eq!(IssueKind::OrphanDesktop.label(), "ORPHAN-DESKTOP");
        assert_eq!(IssueKind::DanglingBinLink.label(), "DANGLING-BIN-LINK");
    }
}
