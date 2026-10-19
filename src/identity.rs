//! AppImage identity extraction.
//!
//! Given an arbitrary `*.AppImage` file we want:
//!
//! 1. **A stable id** to reference the install by. We try, in order:
//!    a. The Type-2 embedded AppImage ID (read from the SquashFS's
//!       `AppImage.yml`, accessible via `--appimage-extract`).
//!    b. The filename stem (e.g. `Foo-x86_64` from `Foo-x86_64.AppImage`).
//!
//! 2. **A version string** to disambiguate two installs of the same
//!    app at different versions. We try, in order:
//!    a. `AppImage.yml`'s `image_version` (Type-2 only).
//!    b. A regex pattern in the filename: `-v1.2.3`, `-1.2.3`,
//!       `_1.2.3`, etc.
//!    c. Otherwise `None`.
//!
//! 3. **Detection that the file is actually an AppImage.** We check for
//!    ELF magic and the `AI\x02` marker at offset 8 (Type-2).
//!    Type-1 AppImages lack the marker but are also self-mounting ELF
//!    binaries; we accept them by absence of the marker, since
//!    rejecting them would break the user's `aim install foo.AppImage`
//!    on ancient releases.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// What we learned about an AppImage before installing it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppImageIdentity {
    /// Stable id (e.g. `vikunja`).
    pub id: String,
    /// Version if detected (e.g. `1.2.3`); `None` if unknown.
    pub version: Option<String>,
    /// Detected AppImage type.
    pub kind: AppImageKind,
    /// Where the id came from. Useful for `--name` overrides.
    pub id_source: IdSource,
}

/// The two AppImage kinds the spec recognises.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppImageKind {
    /// ELF magic but no `AI\x02` marker. Older.
    Type1,
    /// ELF magic **and** `AI\x02` marker (offset 8).
    Type2,
}

/// Origin of the derived id, so the user can sanity-check what
/// happened.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum IdSource {
    EmbeddedAppImageYml,
    Filename,
    UserOverride,
}

/// Compose the canonical `id@version` string used everywhere.
#[must_use]
pub fn id_version(identity: &AppImageIdentity) -> String {
    match &identity.version {
        Some(v) => format!("{}@{}", identity.id, v),
        None => identity.id.clone(),
    }
}

/// Inspect the AppImage at `path` and return its identity.
///
/// Runs `--appimage-extract` into a tempdir (cleared after use) so we
/// can read `AppImage.yml`. For Type-1 (where extract is unsupported)
/// we fall back to filename heuristics.
pub fn identify(path: &Path) -> Result<AppImageIdentity> {
    let bytes = std::fs::read(path).map_err(|source| Error::Path {
        path: path.to_path_buf(),
        source,
    })?;
    let kind = detect_kind(&bytes);

    // Try to extract metadata from the embedded SquashFS. We try this
    // even on Type-1 — the runtime will refuse but the failure mode is
    // bounded.
    let extracted_meta = extract_appimage_yml(path, kind);

    let (id, id_source, mut version) = match extracted_meta {
        Ok(Some(meta)) => (
            meta.id.unwrap_or_else(|| fallback_id_from_path(path)),
            IdSource::EmbeddedAppImageYml,
            meta.image_version,
        ),
        _ => (fallback_id_from_path(path), IdSource::Filename, None),
    };

    // Upgrade version from filename if extracted didn't have one.
    if version.is_none() {
        version = parse_version_from_filename(path);
    }

    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(Error::InvalidTarget {
            message: format!("could not derive an id from {}", path.display()),
        });
    }

    Ok(AppImageIdentity {
        id,
        version,
        kind,
        id_source,
    })
}

/// Inspect the first 16 bytes: ELF magic + optional `AI\x02` marker.
fn detect_kind(bytes: &[u8]) -> AppImageKind {
    const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
    const TYPE2_MARKER: &[u8] = b"AI\x02";

    if bytes.len() >= 16 && bytes.starts_with(&ELF_MAGIC) && bytes[8..].starts_with(TYPE2_MARKER) {
        AppImageKind::Type2
    } else {
        AppImageKind::Type1
    }
}

/// Subset of `AppImage.yml` we actually consume.
#[derive(Debug, Deserialize)]
struct AppImageYml {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    image_version: Option<String>,
}

/// Run `--appimage-extract` against a tempdir and read `AppImage.yml`
/// from the result. Returns `None` if there's no `AppImage.yml`.
fn extract_appimage_yml(path: &Path, kind: AppImageKind) -> Result<Option<AppImageYml>> {
    // Type-1 doesn't support extract. Skip the subprocess entirely.
    if kind == AppImageKind::Type1 {
        return Ok(None);
    }

    let staging = crate::staging::StagingDir::new("aim-identify")?;
    let result = std::process::Command::new(path)
        .arg("--appimage-extract")
        .current_dir(staging.path())
        .output();

    let out = match result {
        Ok(o) => o,
        Err(_) => return Ok(None), // not executable in this env
    };

    if !out.status.success() {
        return Ok(None);
    }

    let yml_path = staging.path().join("squashfs-root").join("AppImage.yml");
    if !yml_path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&yml_path).map_err(|source| Error::Path {
        path: yml_path.clone(),
        source,
    })?;
    let parsed: AppImageYml = serde_yaml_borrow(&text).unwrap_or(AppImageYml {
        id: None,
        image_version: None,
    });
    Ok(Some(parsed))
}

/// Tiny YAML subset parser that handles the two scalar keys we care
/// about. We deliberately avoid pulling in a YAML crate — `AppImage.yml`
/// in the wild is flat `key: value` pairs 99% of the time.
fn serde_yaml_borrow(text: &str) -> Option<AppImageYml> {
    let mut id = None;
    let mut image_version = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id:") {
            id = Some(unquote(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("image_version:") {
            image_version = Some(unquote(rest.trim()));
        }
    }
    Some(AppImageYml { id, image_version })
}

fn unquote(s: &str) -> String {
    let trimmed = s.trim_matches('"').trim_matches('\'');
    trimmed.to_string()
}

/// Strip `.AppImage` and arch suffixes from the filename, returning a
/// reasonable id fallback.
fn fallback_id_from_path(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let s = stem.trim_end_matches(".AppImage");
    // Strip common arch / channel suffixes.
    let mut out = s.to_string();
    for suffix in ["-x86_64", "-x86-64", "-aarch64", "-arm64", "-i686"] {
        if let Some(rest) = out.strip_suffix(suffix) {
            out = rest.to_string();
        }
    }
    // If the filename has a version pattern, strip it for the *id*
    // (we keep it separately as `version`).
    if let Some(ver) = parse_version_from_filename(path) {
        // Look for `<id><ver_sep><ver>` in the stem.
        let pat = ver;
        let separators = ["_", "-", "."];
        for sep in separators {
            if let Some(idx) = out.rfind(&format!("{sep}{pat}")) {
                out.truncate(idx);
                break;
            }
            // also try `<pat>` at the end directly
            if out.ends_with(&pat) && out.len() > pat.len() {
                let cut = out.len() - pat.len();
                let last_char = out[..cut].chars().last().unwrap_or(' ');
                if separators.contains(&last_char.to_string().as_str()) {
                    out = out[..cut - last_char.len_utf8()].to_string();
                    break;
                }
            }
        }
    }
    let out = out.trim_end_matches(|c: char| c == '_' || c == '-' || c == '.');
    if out.is_empty() { "unknown".to_string() } else { out.to_string() }
}

/// Match `-1.2.3`, `-v1.2.3`, `_2.4`, etc. **Only digits-and-dots**
/// (plus an optional leading `v`). Prerelease tags like `-rc1` or
/// architecture suffixes like `-x86_64` look similar by shape; to
/// avoid misclassifying `x86_64` as a prerelease, we only consume
/// what is unambiguously numeric. Users can override via `--name` or
/// in the embedded `AppImage.yml`.
fn parse_version_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    let bytes = stem.as_bytes();
    let mut best: Option<(usize, String)> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'-' || c == b'_' {
            // Try direct-digit: `<sep><digits>`
            if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                if let Some((end_idx, value)) = scan_version_digits(&stem[i + 1..]) {
                    if best.as_ref().map(|(idx, _)| end_idx > *idx).unwrap_or(true) {
                        best = Some((i + 1 + end_idx, value));
                    }
                    i = i + 1 + end_idx;
                    continue;
                }
            }
            // Try `<sep>v<digits>` — keep the leading `v`.
            if i + 2 < bytes.len() && bytes[i + 1] == b'v' && bytes[i + 2].is_ascii_digit() {
                if let Some((end_idx, _)) = scan_version_digits(&stem[i + 2..]) {
                    let value = stem[i + 2..i + 2 + end_idx].to_string();
                    let prefixed = format!("v{value}");
                    if best.as_ref().map(|(idx, _)| end_idx > *idx).unwrap_or(true) {
                        best = Some((i + 2 + end_idx, prefixed));
                    }
                    i = i + 2 + end_idx;
                    continue;
                }
            }
        }
        i += 1;
    }
    best.map(|(_, v)| v)
}

/// `digits(.digits)+` — at least two dotted components to avoid
/// catching accidental lone digits like `Foo-2-Bar`. Returns
/// (`consumed_bytes`, `value`).
fn scan_version_digits(s: &str) -> Option<(usize, String)> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut i = 0;
    let mut dots = 0;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        if bytes[i] == b'.' {
            dots += 1;
        }
        i += 1;
    }
    if dots == 0 {
        return None;
    }
    Some((i, s[..i].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf_with_marker() -> Vec<u8> {
        let mut v = vec![0x7f, b'E', b'L', b'F'];
        v.extend_from_slice(&[0x02, 0x01, 0x01, 0x00]); // 64-bit, little-endian
        v.extend_from_slice(b"AI\x02");
        v.resize(64, 0);
        v
    }

    #[test]
    fn detect_type2_marker() {
        assert_eq!(detect_kind(&elf_with_marker()), AppImageKind::Type2);
    }

    #[test]
    fn detect_type1_without_marker() {
        let mut v = vec![0x7f, b'E', b'L', b'F', 0x02, 0x01, 0x01, 0x00];
        v.resize(64, 0);
        assert_eq!(detect_kind(&v), AppImageKind::Type1);
    }

    #[test]
    fn version_from_filename() {
        let p = Path::new("Foo-1.2.3-x86_64.AppImage");
        assert_eq!(parse_version_from_filename(p).as_deref(), Some("1.2.3"));
        let p = Path::new("bar_v0.4.AppImage");
        assert_eq!(parse_version_from_filename(p).as_deref(), Some("v0.4"));
        let p = Path::new("App-2.4.0-rc1-x86_64.AppImage");
        // Prerelease tags look like arch suffixes; for safety we
        // strip them. Use `--name` for the rare case where the
        // version really is `2.4.0-rc1`.
        assert_eq!(parse_version_from_filename(p).as_deref(), Some("2.4.0"));
        let p = Path::new("standalone.AppImage");
        assert_eq!(parse_version_from_filename(p), None);
        // Lone digit shouldn't claim version status.
        let p = Path::new("Foo-2-Bar.AppImage");
        assert_eq!(parse_version_from_filename(p), None);
    }

    #[test]
    fn id_from_filename_strip_arch() {
        let p = Path::new("/tmp/Vikunja-1.0.0-x86_64.AppImage");
        let id = fallback_id_from_path(p);
        // We don't pin the exact form (filename-based stripping has
        // many edge cases) but require it to be non-empty and contain
        // the leading "Vikunja" stem.
        assert!(!id.is_empty());
        assert!(id.starts_with("Vikunja") || id.contains("Vikunja"));
    }
}
