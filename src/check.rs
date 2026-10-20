//! `aim check` — list installed AppImages with upstream-update
//! availability.
//!
//! For each record in `state.toml`:
//! - **Gh sources**: re-resolve the latest release via `gh::resolve_release`
//!   and compare versions. Status is one of `up-to-date`,
//!   `upgrade available`, `ahead of upstream`, or `unknown`.
//! - **Local / Url / Unknown sources**: marked `not trackable`.
//!
//! On GitHub resolution failure the entry's status reports the error
//! inline so a single bad network call doesn't fail the whole run.

use std::cmp::Ordering;

use serde::Serialize;

use crate::error::Result;
use crate::gh;
use crate::paths::Layout;
use crate::state::{Source, State};

/// Options for [`run`].
#[derive(Debug, Clone, Default)]
pub struct CheckOpts {
    /// Restrict to entries where the upstream is newer.
    pub outdated_only: bool,
    pub json: bool,
}

/// One row of the check report.
#[derive(Debug, Clone, Serialize)]
pub struct CheckEntry {
    pub key: String,
    /// Local version (from the state record), or `None` if unknown.
    pub local: Option<String>,
    /// Upstream version (from the latest GitHub release), or `None`
    /// for non-trackable sources / unresolved networks.
    pub upstream: Option<String>,
    /// One of: `up-to-date`, `upgrade available`, `ahead of upstream`,
    /// `unknown`, `not trackable`, or `upstream check failed: <why>`.
    pub status: String,
}

impl CheckEntry {
    /// Status string for the various source types.
    fn not_trackable(key: &str, local: Option<String>) -> Self {
        Self {
            key: key.to_string(),
            local,
            upstream: None,
            status: "not trackable".to_string(),
        }
    }
}

pub fn run(opts: CheckOpts, layout: &Layout) -> Result<()> {
    let state = State::load(&layout.state_file)?;
    let mut entries: Vec<CheckEntry> = state
        .apps
        .iter()
        .map(|(key, rec)| check_record(key, rec))
        .collect();

    if opts.outdated_only {
        entries.retain(|e| e.status == "upgrade available");
    }

    if opts.json {
        let json = serde_json::to_string_pretty(&entries)?;
        println!("{json}");
        return Ok(());
    }

    if entries.is_empty() {
        if opts.outdated_only {
            println!("aim: all installed AppImages are up-to-date");
        } else {
            println!("aim: no AppImages installed");
        }
        return Ok(());
    }

    let key_width = entries.iter().map(|e| e.key.len()).max().unwrap_or(8).max(8);
    println!(
        "{:<key_width$}  {:<10}  {:<10}  {}",
        "KEY", "LOCAL", "UPSTREAM", "STATUS"
    );
    for e in &entries {
        let local = e.local.as_deref().unwrap_or("—");
        let upstream = e.upstream.as_deref().unwrap_or("—");
        println!(
            "{:<key_width$}  {:<10}  {:<10}  {}",
            e.key, local, upstream, e.status
        );
    }
    Ok(())
}

fn check_record(key: &str, record: &crate::state::AppRecord) -> CheckEntry {
    match &record.source {
        Source::Local { .. } | Source::Url { .. } | Source::Unknown => {
            CheckEntry::not_trackable(key, record.version.clone())
        }
        Source::Gh { owner, repo } => {
            let gh_ref = gh::GhRef {
                owner: owner.clone(),
                repo: repo.clone(),
                version: None,
            };
            match gh::resolve_release(&gh_ref) {
                Ok((upstream_tag, _asset)) => {
                    let local_v = parse_version(record.version.as_deref().unwrap_or(""));
                    let upstream_v = parse_version(&upstream_tag);
                    let status = match (local_v, upstream_v) {
                        (Some(l), Some(u)) => match compare(&l, &u) {
                            Ordering::Equal => "up-to-date",
                            Ordering::Less => "upgrade available",
                            Ordering::Greater => "ahead of upstream",
                        },
                        _ => "unknown",
                    };
                    CheckEntry {
                        key: key.to_string(),
                        local: record.version.clone(),
                        upstream: Some(upstream_tag),
                        status: status.to_string(),
                    }
                }
                Err(e) => CheckEntry {
                    key: key.to_string(),
                    local: record.version.clone(),
                    upstream: None,
                    status: format!("upstream check failed: {e}"),
                },
            }
        }
    }
}

/// Parse a version string into a list of numeric components. Tolerant
/// of `v`-prefixes, `release-` prefixes, and a final non-numeric
/// pre-release segment.
#[must_use]
pub fn parse_version(s: &str) -> Option<Vec<u32>> {
    let s = s.trim().trim_start_matches('v').trim_start_matches("release-");
    if s.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in s.split(|c: char| c == '.' || c == '-' || c == '+') {
        if part.is_empty() {
            continue;
        }
        // Extract the leading numeric prefix; the rest of the
        // segment is treated as a pre-release / build tag we ignore.
        let numeric_end = part.find(|c: char| !c.is_ascii_digit()).unwrap_or(part.len());
        let numeric = &part[..numeric_end];
        if numeric.is_empty() {
            // Non-numeric segment like "beta" or "rc": stop parsing
            // (the prior zero-or-more components are still valid).
            break;
        }
        match numeric.parse::<u32>() {
            Ok(n) => out.push(n),
            Err(_) => break,
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Lexicographic compare on numeric version components, with the
/// shorter list considered equal if its trailing components are
/// all zero (semver rule: `1.0 == 1.0.0`).
pub fn compare(a: &[u32], b: &[u32]) -> Ordering {
    for (x, y) in a.iter().zip(b.iter()) {
        match x.cmp(y) {
            Ordering::Equal => continue,
            non_eq => return non_eq,
        }
    }
    // All zipped components are equal; the rest of the longer
    // list must be all-zero for the versions to be equal.
    let rest: &[u32] = if a.len() > b.len() { &a[b.len()..] } else { &b[a.len()..] };
    for x in rest {
        if *x != 0 {
            return if a.len() > b.len() { Ordering::Greater } else { Ordering::Less };
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_versions() {
        assert_eq!(parse_version("1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("0.22.0"), Some(vec![0, 22, 0]));
    }

    #[test]
    fn parse_handles_release_prefix() {
        assert_eq!(parse_version("release-1.2.3"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn parse_stops_at_pre_release() {
        assert_eq!(parse_version("1.0.0-beta.1"), Some(vec![1, 0, 0]));
        assert_eq!(parse_version("2.0.0rc1"), Some(vec![2, 0, 0]));
    }

    #[test]
    fn parse_short_versions() {
        assert_eq!(parse_version("1.0"), Some(vec![1, 0]));
        assert_eq!(parse_version("1"), Some(vec![1]));
    }

    #[test]
    fn parse_invalid() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version("v"), None);
    }

    #[test]
    fn compare_equal_with_diff_lengths() {
        assert_eq!(compare(&[1, 0, 0], &[1, 0]), Ordering::Equal);
        assert_eq!(compare(&[1, 0], &[1, 0, 0]), Ordering::Equal);
    }

    #[test]
    fn compare_less() {
        assert_eq!(compare(&[0, 22, 0], &[0, 22, 1]), Ordering::Less);
        assert_eq!(compare(&[1, 0, 0], &[1, 0, 1]), Ordering::Less);
    }

    #[test]
    fn compare_greater() {
        assert_eq!(compare(&[1, 1, 0], &[1, 0, 5]), Ordering::Greater);
    }

    #[test]
    fn non_trackable_for_local_source() {
        // No network needed; this path never calls gh::resolve.
        let rec = crate::state::AppRecord {
            id: "foo".to_string(),
            version: Some("1.0.0".to_string()),
            sha256: "x".to_string(),
            size_bytes: 0,
            integrated: false,
            source: Source::Local { path: "/tmp/x".to_string() },
            icon_size_px: None,
            icon_path: None,
            env: Default::default(),
            extra_exec_args: vec![],
            comment: None,
            categories: vec![],
            mime_types: vec![],
            installed_at_secs: 0,
        };
        let entry = check_record("foo@1.0.0", &rec);
        assert_eq!(entry.status, "not trackable");
        assert_eq!(entry.upstream, None);
        assert_eq!(entry.local.as_deref(), Some("1.0.0"));
    }
}
