//! Shared scanner: cross-check `state.toml` against the filesystem
//! to find inconsistencies, orphan files, and dangling symlinks.
//!
//! Used by both `aim gc` (which deletes the recoverable subset) and
//! `aim doctor` (which only reports).

use std::path::PathBuf;

use crate::error::Result;
use crate::paths::Layout;
use crate::state::State;

/// Categories of issues the scanner can detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "SCREAMING-KEBAB-CASE")]
pub enum IssueKind {
    /// `state.toml` references a cache file that no longer exists
    /// on disk. Recoverable: `aim install <source>` again.
    MissingCache,
    /// `state.toml` says `integrated = true` but the `.desktop`
    /// file is absent. Re-fix with `aim integrate <id>`.
    MissingDesktop,
    /// `state.toml` says `integrated = true` but the bin symlink is
    /// absent. Re-fix with `aim integrate <id>`.
    MissingBinLink,
    /// Bin symlink points at a non-existent target. Recoverable: gc
    /// can drop the symlink.
    DanglingBinLink,
    /// A `apps/<key>/AppImage` file exists on disk but no state
    /// record references it. Recoverable: gc can drop it (and the
    /// parent directory).
    OrphanCache,
    /// An `aim-<id>.desktop` file exists with no state record.
    /// Recoverable: gc can drop it.
    OrphanDesktop,
    /// A bin symlink under `bin_dir` points at `apps_dir` but the
    /// target's directory has no state record. Recoverable: gc can
    /// drop the symlink.
    OrphanBinLink,
}

impl IssueKind {
    /// True iff this kind represents something a healthy system
    /// shouldn't have on disk; the gc command is allowed to remove
    /// it without further confirmation beyond the global `--yes`.
    pub fn is_recoverable(self) -> bool {
        matches!(
            self,
            IssueKind::DanglingBinLink
                | IssueKind::OrphanCache
                | IssueKind::OrphanDesktop
                | IssueKind::OrphanBinLink
        )
    }

    /// Short upper-case label for the human-readable report.
    pub fn label(self) -> &'static str {
        match self {
            IssueKind::MissingCache => "MISSING-CACHE",
            IssueKind::MissingDesktop => "MISSING-DESKTOP",
            IssueKind::MissingBinLink => "MISSING-BIN-LINK",
            IssueKind::DanglingBinLink => "DANGLING-BIN-LINK",
            IssueKind::OrphanCache => "ORPHAN-CACHE",
            IssueKind::OrphanDesktop => "ORPHAN-DESKTOP",
            IssueKind::OrphanBinLink => "ORPHAN-BIN-LINK",
        }
    }
}

/// A single inconsistency the scanner found.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Issue {
    pub kind: IssueKind,
    /// Path to display in the report. For `OrphanCache` this is the
    /// `AppImage` file inside the orphan directory; gc will delete
    /// the parent directory in addition to this file.
    pub path: PathBuf,
    /// State-side id/key when known. Used for filtering & display.
    pub id: Option<String>,
    /// Free-form detail (e.g. "state sha256=abc… file sha256=def…").
    /// Currently empty; reserved for bit-rot detection (slice 10).
    pub detail: Option<String>,
}

impl Issue {
    pub fn new(kind: IssueKind, path: PathBuf) -> Self {
        Self { kind, path, id: None, detail: None }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Filesystem paths to remove when `gc` acts on this issue. For
    /// `OrphanCache` we also include the parent directory so the
    /// leftover `apps/<key>/` doesn't sit empty.
    pub fn cleanup_paths(&self) -> Vec<PathBuf> {
        if matches!(self.kind, IssueKind::OrphanCache) {
            if let Some(parent) = self.path.parent() {
                return vec![self.path.clone(), parent.to_path_buf()];
            }
        }
        vec![self.path.clone()]
    }
}

/// Cross-check the on-disk state against `state.toml`. Returns a
/// vector of issues (possibly empty). The order is roughly:
/// state-driven issues first, then filesystem-driven orphans.
pub fn scan(layout: &Layout) -> Result<Vec<Issue>> {
    let state = State::load(&layout.state_file)?;
    let mut issues = Vec::new();

    // 1. State-driven: for each record, check the files it claims
    //    exist.
    for (key, record) in &state.apps {
        let cache = layout.appimage_path(key);
        if !cache.exists() {
            issues.push(
                Issue::new(IssueKind::MissingCache, cache)
                    .with_id(key.clone()),
            );
        }

        if record.integrated {
            let desktop = layout.desktop_file(key);
            if !desktop.exists() {
                issues.push(
                    Issue::new(IssueKind::MissingDesktop, desktop)
                        .with_id(key.clone()),
                );
            }
            let bin = layout.bin_link(key);
            if bin.is_symlink() {
                // Resolve and check the target exists.
                match std::fs::read_link(&bin) {
                    Ok(target) if !target.exists() => {
                        issues.push(
                            Issue::new(IssueKind::DanglingBinLink, bin)
                                .with_id(key.clone()),
                        );
                    }
                    Ok(_) => {} // symlink and target both present: fine
                    Err(_) => {
                        issues.push(
                            Issue::new(IssueKind::DanglingBinLink, bin)
                                .with_id(key.clone()),
                        );
                    }
                }
            } else if !bin.exists() {
                issues.push(
                    Issue::new(IssueKind::MissingBinLink, bin)
                        .with_id(key.clone()),
                );
            }
        }
    }

    // 2. Filesystem-driven: walk apps/ and find directories not
    //    referenced by state.
    if let Ok(rd) = std::fs::read_dir(&layout.apps_dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            if state.apps.contains_key(&dir_name) {
                continue;
            }
            let appimage = path.join("AppImage");
            if appimage.is_file() {
                issues.push(
                    Issue::new(IssueKind::OrphanCache, appimage)
                        .with_id(dir_name),
                );
            } else if path.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false) {
                // Empty directory; cheap to clean.
                issues.push(
                    Issue::new(IssueKind::OrphanCache, path.clone())
                        .with_id(dir_name),
                );
            }
        }
    }

    // 3. Walk applications/ for orphan .desktop files.
    let apps_dir = layout.apps_root_dir.join("applications");
    if let Ok(rd) = std::fs::read_dir(&apps_dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name
                .strip_prefix("aim-")
                .and_then(|n| n.strip_suffix(".desktop"))
            {
                if !state.apps.contains_key(id) {
                    issues.push(
                        Issue::new(IssueKind::OrphanDesktop, entry.path())
                            .with_id(id.to_string()),
                    );
                }
            }
        }
    }

    // 4. Walk bin_dir for orphan / dangling symlinks whose target
    //    is under our apps_dir.
    if let Ok(rd) = std::fs::read_dir(&layout.bin_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_symlink() {
                continue;
            }
            let target = match std::fs::read_link(&p) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if !target.starts_with(&layout.apps_dir) {
                continue;
            }
            let dir_name = target
                .parent()
                .and_then(|parent| parent.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if dir_name.is_empty() {
                continue;
            }
            if !target.exists() {
                issues.push(
                    Issue::new(IssueKind::DanglingBinLink, p.clone())
                        .with_id(dir_name),
                );
            } else if !state.apps.contains_key(&dir_name) {
                issues.push(
                    Issue::new(IssueKind::OrphanBinLink, p).with_id(dir_name),
                );
            }
        }
    }

    Ok(issues)
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
        let home = std::env::temp_dir().join(format!("aim-health-{nanos}"));
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
    fn clean_state_yields_no_issues() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", false);
        let issues = scan(&layout).unwrap();
        assert!(issues.is_empty(), "expected no issues, got: {issues:#?}");
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn missing_cache_detected() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", true);
        let cache = layout.appimage_path("Foo");
        std::fs::remove_file(&cache).unwrap();
        let issues = scan(&layout).unwrap();
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&IssueKind::MissingCache));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn orphan_cache_detected() {
        let (layout, home) = fresh_layout();
        // Plant a directory in apps/ that state has no record of.
        let orphan_dir = layout.apps_dir.join("zombie");
        fs::create_dir_all(&orphan_dir).unwrap();
        fake_appimage(&orphan_dir.join("AppImage"));
        let issues = scan(&layout).unwrap();
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&IssueKind::OrphanCache));
        let issue = issues.iter().find(|i| i.kind == IssueKind::OrphanCache).unwrap();
        assert_eq!(issue.id.as_deref(), Some("zombie"));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn orphan_desktop_detected() {
        let (layout, home) = fresh_layout();
        let apps = layout.apps_root_dir.join("applications");
        fs::create_dir_all(&apps).unwrap();
        fs::write(apps.join("aim-zombie.desktop"), b"").unwrap();
        let issues = scan(&layout).unwrap();
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&IssueKind::OrphanDesktop));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn dangling_bin_link_detected() {
        let (layout, home) = fresh_layout();
        install_one(&layout, &home, "Foo.AppImage", false);
        // Break the cache under the live state's feet.
        let cache = layout.appimage_path("Foo");
        std::fs::remove_file(&cache).unwrap();
        let issues = scan(&layout).unwrap();
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert!(
            kinds.contains(&IssueKind::DanglingBinLink),
            "expected DanglingBinLink among: {kinds:?}"
        );
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn orphan_bin_link_detected() {
        let (layout, home) = fresh_layout();
        // Plant a bin symlink whose target is under apps/ but
        // points at a directory that isn't in state.
        let zombie_dir = layout.apps_dir.join("zombie");
        fs::create_dir_all(&zombie_dir).unwrap();
        fake_appimage(&zombie_dir.join("AppImage"));
        let link = layout.bin_dir.join("zombie");
        // `Layout::with_home` does not create bin_dir; make sure it
        // exists so the symlink syscall can succeed.
        fs::create_dir_all(&layout.bin_dir).unwrap();
        std::os::unix::fs::symlink(zombie_dir.join("AppImage"), &link).unwrap();
        let issues = scan(&layout).unwrap();
        let kinds: Vec<_> = issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&IssueKind::OrphanBinLink));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn cleanup_paths_for_orphan_cache_includes_parent() {
        let (layout, home) = fresh_layout();
        let orphan_dir = layout.apps_dir.join("zombie");
        fs::create_dir_all(&orphan_dir).unwrap();
        fake_appimage(&orphan_dir.join("AppImage"));
        let issues = scan(&layout).unwrap();
        let issue = issues.iter().find(|i| i.kind == IssueKind::OrphanCache).unwrap();
        let paths = issue.cleanup_paths();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p.is_dir()));
        assert!(paths.iter().any(|p| p.is_file()));
        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn recoverability_classification() {
        assert!(IssueKind::OrphanCache.is_recoverable());
        assert!(IssueKind::OrphanDesktop.is_recoverable());
        assert!(IssueKind::OrphanBinLink.is_recoverable());
        assert!(IssueKind::DanglingBinLink.is_recoverable());
        assert!(!IssueKind::MissingCache.is_recoverable());
        assert!(!IssueKind::MissingDesktop.is_recoverable());
        assert!(!IssueKind::MissingBinLink.is_recoverable());
    }
}
