//! `gh:owner/repo` resolution.
//!
//! Two paths to find the AppImage asset URL in a GitHub release:
//! 1. The `gh` CLI, if installed and authenticated — fastest, works
//!    for private repos, no token management.
//! 2. The GitHub REST API (`/repos/{owner}/{repo}/releases/latest`),
//!    optionally authenticated via `gh auth token`.
//!
//! Both paths produce the same normalised [`Asset`] list, then
//! [`pick_appimage`] chooses the right one.

use serde::Deserialize;

use crate::error::{Error, Result};

/// A parsed `gh:owner/repo` reference. `version` is `None` for "the
/// latest release"; for `slice 8` we only support latest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhRef {
    pub owner: String,
    pub repo: String,
    pub version: Option<String>,
}

/// A single downloadable file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// Direct download URL.
    pub url: String,
    pub size_bytes: u64,
}

/// Parse a `gh:owner/repo` or `gh:owner/repo@version` reference.
#[must_use]
pub fn parse(s: &str) -> Result<GhRef> {
    if !s.starts_with("gh:") {
        return Err(Error::InvalidTarget {
            message: format!("not a gh: ref: {s}"),
        });
    }
    let body = &s[3..];
    let (path, version) = match body.split_once('@') {
        Some((p, v)) => (p, Some(v.to_string())),
        None => (body, None),
    };
    let (owner, repo) = path.split_once('/').ok_or_else(|| Error::InvalidTarget {
        message: format!("gh: ref must be owner/repo, got: {s}"),
    })?;
    if owner.is_empty() || repo.is_empty() {
        return Err(Error::InvalidTarget {
            message: format!("gh: ref must be owner/repo, got: {s}"),
        });
    }
    // Repo names can include ".git" or trailing slashes from sloppy
    // copying; trim.
    let repo = repo.trim_end_matches('/').trim_end_matches(".git").to_string();
    if repo.is_empty() {
        return Err(Error::InvalidTarget {
            message: format!("gh: ref must be owner/repo, got: {s}"),
        });
    }
    Ok(GhRef {
        owner: owner.to_string(),
        repo,
        version,
    })
}

/// Choose the AppImage asset to download. Heuristic:
/// 1. Filter to assets whose name ends in `.appimage` (case-insensitive).
/// 2. If exactly one, return it.
/// 3. If multiple, return the largest by `size_bytes`.
/// 4. Otherwise error.
#[must_use]
pub fn pick_appimage(assets: &[Asset]) -> Result<&Asset> {
    let candidates: Vec<&Asset> = assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".appimage"))
        .collect();
    match candidates.len() {
        0 => Err(Error::InvalidTarget {
            message: "no .AppImage asset in the latest release".to_string(),
        }),
        1 => Ok(candidates[0]),
        _ => Ok(candidates
            .into_iter()
            .max_by_key(|a| a.size_bytes)
            .expect("candidates is non-empty (just checked len > 1)")),
    }
}

/// Resolve a `GhRef` to an AppImage asset URL. Tries the `gh` CLI
/// first, then the GitHub REST API. Returns the first resolvable
/// release and the chosen AppImage asset.
pub fn resolve_release(r: &GhRef) -> Result<(String, Asset)> {
    // Try gh CLI
    if let Ok(assets) = try_gh_cli(r) {
        if let Ok(asset) = pick_appimage(&assets) {
            let tag = "latest".to_string();
            return Ok((tag, asset.clone()));
        }
    }
    // Fall back to REST
    let (tag, assets) = try_rest(r)?;
    let asset = pick_appimage(&assets)?.clone();
    Ok((tag, asset))
}

// ─── gh CLI path ─────────────────────────────────────────────────────

fn try_gh_cli(r: &GhRef) -> Result<Vec<Asset>> {
    let repo = format!("{}/{}", r.owner, r.repo);
    let output = std::process::Command::new("gh")
        .args([
            "release",
            "view",
            "--repo",
            &repo,
            "--json",
            "tagName,assets",
        ])
        .output()
        .map_err(|e| Error::Download {
            url: format!("gh CLI invocation for {repo}"),
            message: format!("failed to spawn `gh`: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::Download {
            url: format!("gh CLI invocation for {repo}"),
            message: if stderr.is_empty() {
                format!("`gh release view` exited with status {}", output.status)
            } else {
                stderr
            },
        });
    }

    let parsed: GhCliRelease = serde_json::from_slice(&output.stdout).map_err(|e| {
        Error::Download {
            url: format!("gh CLI invocation for {repo}"),
            message: format!("could not parse gh output: {e}"),
        }
    })?;

    let assets = parsed
        .assets
        .into_iter()
        .map(|a| Asset {
            name: a.name,
            url: a.browser_download_url,
            size_bytes: a.size as u64,
        })
        .collect();
    Ok(assets)
}

#[derive(Debug, Deserialize)]
struct GhCliRelease {
    #[allow(dead_code)]
    tag_name: Option<String>,
    assets: Vec<GhCliAsset>,
}

#[derive(Debug, Deserialize)]
struct GhCliAsset {
    name: String,
    size: u64,
    /// `gh` 2.102+ outputs this as `"url"`; older versions called it
    /// `"browserDownloadUrl"`. We accept both via alias.
    #[serde(alias = "url", alias = "browserDownloadUrl")]
    browser_download_url: String,
}

// ─── REST path ───────────────────────────────────────────────────────

fn try_rest(r: &GhRef) -> Result<(String, Vec<Asset>)> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        r.owner, r.repo
    );

    let mut req = ureq::get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "aim/0.1");

    // If `gh` is on PATH and authenticated, borrow its token. This
    // is a clean way to opt-in to higher rate limits without
    // requiring the user to set GITHUB_TOKEN.
    if let Some(tok) = gh_auth_token() {
        req = req.set("Authorization", &format!("Bearer {tok}"));
    }

    let resp = req.call().map_err(|e| Error::Download {
        url: url.clone(),
        message: format!("GitHub REST request failed: {e}"),
    })?;

    let parsed: RestRelease = resp
        .into_json()
        .map_err(|e| Error::Download {
            url: url.clone(),
            message: format!("could not parse GitHub response: {e}"),
        })?;

    let assets = parsed
        .assets
        .into_iter()
        .map(|a| Asset {
            name: a.name,
            url: a.browser_download_url,
            size_bytes: a.size,
        })
        .collect();
    Ok((parsed.tag_name, assets))
}

fn gh_auth_token() -> Option<String> {
    let output = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct RestRelease {
    tag_name: String,
    assets: Vec<RestAsset>,
}

#[derive(Debug, Deserialize)]
struct RestAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, size: u64) -> Asset {
        Asset {
            name: name.to_string(),
            url: format!("https://example.com/{name}"),
            size_bytes: size,
        }
    }

    #[test]
    fn parse_basic() {
        let r = parse("gh:foo/bar").unwrap();
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
        assert_eq!(r.version, None);
    }

    #[test]
    fn parse_with_version() {
        let r = parse("gh:foo/bar@v1.2.3").unwrap();
        assert_eq!(r.owner, "foo");
        assert_eq!(r.repo, "bar");
        assert_eq!(r.version.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn parse_strips_git_suffix() {
        let r = parse("gh:foo/bar.git").unwrap();
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_strips_trailing_slash() {
        let r = parse("gh:foo/bar/").unwrap();
        assert_eq!(r.repo, "bar");
    }

    #[test]
    fn parse_rejects_missing_prefix() {
        assert!(parse("foo/bar").is_err());
    }

    #[test]
    fn parse_rejects_missing_slash() {
        assert!(parse("gh:foobar").is_err());
    }

    #[test]
    fn parse_rejects_empty_owner_or_repo() {
        assert!(parse("gh:/bar").is_err());
        assert!(parse("gh:foo/").is_err());
    }

    #[test]
    fn pick_single_appimage() {
        let assets = vec![asset("readme.md", 100), asset("Foo.AppImage", 1024)];
        let chosen = pick_appimage(&assets).unwrap();
        assert_eq!(chosen.name, "Foo.AppImage");
    }

    #[test]
    fn pick_case_insensitive_appimage() {
        let assets = vec![asset("Foo.appimage", 1024)];
        let chosen = pick_appimage(&assets).unwrap();
        assert_eq!(chosen.name, "Foo.appimage");
    }

    #[test]
    fn pick_largest_when_multiple() {
        let assets = vec![
            asset("Foo-i386.AppImage", 1024),
            asset("Foo-x86_64.AppImage", 4096),
            asset("Foo-arm64.AppImage", 2048),
        ];
        let chosen = pick_appimage(&assets).unwrap();
        assert_eq!(chosen.name, "Foo-x86_64.AppImage");
    }

    #[test]
    fn pick_no_appimage_errors() {
        let assets = vec![asset("readme.md", 100), asset("foo.tar.gz", 100)];
        let err = pick_appimage(&assets).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
    }

    #[test]
    fn pick_skips_non_appimage_assets() {
        // Mix of AppImage and other files; only the AppImage is a
        // candidate.
        let assets = vec![
            asset("source.tar.gz", 8000),
            asset("Foo.AppImage", 4000),
        ];
        let chosen = pick_appimage(&assets).unwrap();
        assert_eq!(chosen.name, "Foo.AppImage");
    }
}
