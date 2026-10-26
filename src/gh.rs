//! `gh:owner/repo` resolution.
//!
//! Two paths to find the `AppImage` asset URL in a GitHub release:
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

/// CPU-architecture class of a release asset, derived from the
/// arch tokens projects put in release filenames.
///
/// The ordering of the [`ARCH_TOKENS`] table matters: `arm64`
/// must classify as [`ArchClass::Aarch64`] before the looser
/// `arm` token can pull it into [`ArchClass::Arm`], and `x86_64`
/// must win over `x86`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchClass {
    X86_64,
    Aarch64,
    I686,
    Arm,
    /// No arch token in the name — the asset is either universal
    /// or the project didn't label it.
    Unknown,
}

impl ArchClass {
    /// Human/`std::env::consts::ARCH`-style spelling, for error
    /// messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
            Self::I686 => "i686",
            Self::Arm => "arm",
            Self::Unknown => "unknown",
        }
    }
}

/// Filenames tokens per arch class, in classification order.
/// First matching row wins.
const ARCH_TOKENS: &[(&str, &[&str])] = &[
    ("x86_64", &["x86_64", "x86-64", "x64", "amd64"]),
    ("aarch64", &["aarch64", "arm64"]),
    ("arm", &["armv7", "armhf", "arm"]),
    ("i686", &["i686", "i586", "i486", "i386", "x86"]),
];

/// Classify a release filename's architecture from its tokens.
/// Case-insensitive substring match; table order resolves the
/// `arm64`-contains-`arm` and `x86_64`-contains-`x86` traps.
#[must_use]
pub fn asset_arch(name: &str) -> ArchClass {
    let lower = name.to_lowercase();
    for (class, tokens) in ARCH_TOKENS {
        if tokens.iter().any(|t| lower.contains(t)) {
            return match *class {
                "x86_64" => ArchClass::X86_64,
                "aarch64" => ArchClass::Aarch64,
                "arm" => ArchClass::Arm,
                _ => ArchClass::I686,
            };
        }
    }
    ArchClass::Unknown
}

/// The current machine's arch class, from `std::env::consts::ARCH`.
#[must_use]
pub fn host_arch() -> ArchClass {
    match std::env::consts::ARCH {
        "x86_64" => ArchClass::X86_64,
        "aarch64" => ArchClass::Aarch64,
        "x86" => ArchClass::I686,
        "arm" => ArchClass::Arm,
        _ => ArchClass::Unknown,
    }
}

/// Choose the `AppImage` asset to download, matching the host
/// architecture. Heuristic, in order:
///
/// 1. Filter to assets whose name ends in `.appimage`
///    (case-insensitive).
/// 2. Keep those whose filename arch tokens match the host.
/// 3. Among matches, prefer the largest by `size_bytes` (a weak
///    proxy for "the full build, not a minimal variant").
/// 4. If nothing matches the host arch, fall back to assets with
///    no arch tokens in their name (universal/unlabeled builds).
/// 5. Otherwise error, listing the candidates.
///
/// Rationale (real-world bug this replaces): siyuan v3.8.8 ships
/// `…-linux.AppImage` (`x86_64`, 279,305,950 B) and
/// `…-linux-arm64.AppImage` (277,774,998 B) — 0.5% apart. The old
/// largest-wins picker chose `x86_64` by luck and would happily have
/// downloaded the `arm64` build had it been marginally bigger.
pub fn pick_appimage(assets: &[Asset]) -> Result<&Asset> {
    pick_appimage_for(assets, host_arch())
}

/// Largest asset in a non-empty pool (weak proxy for "the full
/// build, not a minimal variant").
fn largest(pool: Vec<&Asset>) -> &Asset {
    pool.into_iter()
        .max_by_key(|a| a.size_bytes)
        .expect("pool is non-empty")
}

/// [`pick_appimage`] with an explicit host arch, so tests don't
/// depend on the machine they run on.
pub fn pick_appimage_for(assets: &[Asset], host: ArchClass) -> Result<&Asset> {
    let candidates: Vec<&Asset> = assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".appimage"))
        .collect();

    let arch_matches = |a: &Asset| asset_arch(&a.name) == host;

    // Prefer arch-labeled matches for this machine…
    let matching: Vec<&Asset> = candidates.iter().copied().filter(|a| arch_matches(a)).collect();
    if !matching.is_empty() {
        return Ok(largest(matching));
    }

    // …then unlabeled (universal) builds…
    let unlabeled: Vec<&Asset> = candidates
        .iter()
        .copied()
        .filter(|a| asset_arch(&a.name) == ArchClass::Unknown)
        .collect();
    if !unlabeled.is_empty() {
        return Ok(largest(unlabeled));
    }

    // …else nothing sensible: either no .AppImage at all, or every
    // candidate is labeled for a foreign arch (downloading it would
    // waste hundreds of MB and crash on launch).
    if candidates.is_empty() {
        Err(Error::InvalidTarget {
            message: "no .AppImage asset in the latest release".to_string(),
        })
    } else {
        let names: Vec<&str> = candidates.iter().map(|a| a.name.as_str()).collect();
        Err(Error::InvalidTarget {
            message: format!(
                "no .AppImage asset for this machine's architecture ({}); \
                 candidates: {}",
                host.as_str(),
                names.join(", ")
            ),
        })
    }
}

/// Resolve a `GhRef` to an `AppImage` asset URL. Tries the `gh` CLI
/// first, then the GitHub REST API. Returns the first resolvable
/// release and the chosen `AppImage` asset.
pub fn resolve_release(r: &GhRef) -> Result<(String, Asset)> {
    // Try gh CLI
    if let Ok((tag, assets)) = try_gh_cli(r)
        && let Ok(asset) = pick_appimage(&assets)
    {
        return Ok((tag, asset.clone()));
    }
    // Fall back to REST
    let (tag, assets) = try_rest(r)?;
    let asset = pick_appimage(&assets)?.clone();
    Ok((tag, asset))
}

// ─── gh CLI path ─────────────────────────────────────────────────────

fn try_gh_cli(r: &GhRef) -> Result<(String, Vec<Asset>)> {
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

    let tag = parsed.tag_name.unwrap_or_else(|| "latest".to_string());
    let assets = parsed
        .assets
        .into_iter()
        .map(|a| Asset {
            name: a.name,
            url: a.browser_download_url,
            size_bytes: a.size,
        })
        .collect();
    Ok((tag, assets))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhCliRelease {
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
        // Same arch (x86_64 via different token spellings), so the
        // largest one wins on size.
        let assets = vec![
            asset("Foo-amd64.AppImage", 1024),
            asset("Foo-x86_64.AppImage", 4096),
        ];
        let chosen = pick_appimage_for(&assets, ArchClass::X86_64).unwrap();
        assert_eq!(chosen.name, "Foo-x86_64.AppImage");
    }

    #[test]
    fn pick_arch_beats_size() {
        // The arm64 build is *larger*; an x86_64 host must still
        // take the x86_64 build. (Old largest-wins picker failed
        // this whenever the foreign arch happened to ship more
        // bytes.)
        let assets = vec![
            asset("Foo-linux-arm64.AppImage", 5000),
            asset("Foo-linux-x86_64.AppImage", 4000),
        ];
        let chosen = pick_appimage_for(&assets, ArchClass::X86_64).unwrap();
        assert_eq!(chosen.name, "Foo-linux-x86_64.AppImage");
    }

    #[test]
    fn pick_siyuan_v388_real_world() {
        // Regression for the actual siyuan v3.8.8 release that
        // motivated arch-aware picking: the x86_64 build is
        // unlabeled (`siyuan-3.8.8-linux.AppImage`) and only 0.5%
        // larger than the arm64 one. Sizes here are *inverted* so
        // only arch logic — not size luck — can pick correctly.
        let assets = vec![
            asset("siyuan-3.8.8-linux-arm64.AppImage", 279_305_950),
            asset("siyuan-3.8.8-linux.AppImage", 277_774_998),
        ];
        let chosen = pick_appimage_for(&assets, ArchClass::X86_64).unwrap();
        assert_eq!(chosen.name, "siyuan-3.8.8-linux.AppImage");
    }

    #[test]
    fn pick_prefers_host_labeled_over_unlabeled() {
        // When the host arch is explicitly labeled, that beats an
        // unlabeled (possibly foreign) build.
        let assets = vec![
            asset("Foo-linux.AppImage", 9000),
            asset("Foo-linux-x86_64.AppImage", 4000),
        ];
        let chosen = pick_appimage_for(&assets, ArchClass::X86_64).unwrap();
        assert_eq!(chosen.name, "Foo-linux-x86_64.AppImage");
    }

    #[test]
    fn pick_aarch64_host_takes_arm64_over_armv7() {
        let assets = vec![
            asset("Foo-linux-armv7.AppImage", 9000),
            asset("Foo-linux-arm64.AppImage", 8000),
        ];
        let chosen = pick_appimage_for(&assets, ArchClass::Aarch64).unwrap();
        assert_eq!(chosen.name, "Foo-linux-arm64.AppImage");
    }

    #[test]
    fn pick_foreign_arch_only_errors() {
        // Every candidate is labeled for a foreign arch: error
        // instead of downloading the wrong build.
        let assets = vec![asset("Foo-linux-arm64.AppImage", 5000)];
        let err = pick_appimage_for(&assets, ArchClass::X86_64).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
        let msg = err.to_string();
        assert!(msg.contains("x86_64"), "message should name the host arch: {msg}");
        assert!(
            msg.contains("Foo-linux-arm64.AppImage"),
            "message should list candidates: {msg}"
        );
    }

    #[test]
    fn asset_arch_substring_traps() {
        // `arm64` contains `arm`; `x86_64` contains `x86`; order
        // of ARCH_TOKENS must resolve both.
        assert_eq!(asset_arch("Foo-arm64.AppImage"), ArchClass::Aarch64);
        assert_eq!(asset_arch("Foo-aarch64.AppImage"), ArchClass::Aarch64);
        assert_eq!(asset_arch("Foo-armv7.AppImage"), ArchClass::Arm);
        assert_eq!(asset_arch("Foo-armhf.AppImage"), ArchClass::Arm);
        assert_eq!(asset_arch("Foo-x86_64.AppImage"), ArchClass::X86_64);
        assert_eq!(asset_arch("Foo-amd64.AppImage"), ArchClass::X86_64);
        assert_eq!(asset_arch("Foo-x64.AppImage"), ArchClass::X86_64);
        assert_eq!(asset_arch("Foo-i386.AppImage"), ArchClass::I686);
        assert_eq!(asset_arch("Foo.AppImage"), ArchClass::Unknown);
        assert_eq!(asset_arch("Foo-linux.AppImage"), ArchClass::Unknown);
    }

    #[test]
    fn pick_no_appimage_errors() {
        let assets = vec![asset("readme.md", 100), asset("foo.tar.gz", 100)];
        let err = pick_appimage_for(&assets, ArchClass::X86_64).unwrap_err();
        assert_eq!(err.code(), "aim::error::invalid_target");
    }

    /// Live check against the real siyuan-note/siyuan latest
    /// release: the picker must choose an asset that either
    /// matches the host arch or is unlabeled — never a foreign
    /// arch. Read-only (release metadata via `gh`); run with
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore = "hits the network via gh CLI"]
    fn pick_against_live_siyuan_release() {
        let r = parse("gh:siyuan-note/siyuan").unwrap();
        let (_tag, assets) = try_gh_cli(&r).expect("gh CLI available and authenticated");
        let chosen = pick_appimage(&assets).expect("siyuan ships AppImage assets");
        let class = asset_arch(&chosen.name);
        let host = host_arch();
        assert!(
            class == host || class == ArchClass::Unknown,
            "picked {} ({class:?}) on a {host:?} host",
            chosen.name
        );
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
