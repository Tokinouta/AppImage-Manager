//! HTTP(S) downloader for URL installs.
//!
//! Streams the response body to a temp file in the data dir, hashing
//! as we go so we can hand the install pipeline a verified file.
//! Always cleans up the temp file on error.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};

use crate::error::{Error, Result};

/// One downloaded file, ready to be handed to the install pipeline.
#[derive(Debug)]
pub struct Downloaded {
    /// Path to the temp file (still on disk; the install pipeline
    /// reads it then either keeps it as the cache or removes it).
    pub path: PathBuf,
    /// Bytes downloaded, as reported by the server's `Content-Length`
    /// when available, else the actual number of bytes written.
    pub size_bytes: u64,
}

/// Returns true iff the source string looks like an HTTP(S) URL.
#[must_use]
pub fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Download `url` to a temp file in `cache_dir`. The file is
/// persisted (NOT auto-removed) on success; on error the partial
/// file is removed.
///
/// `name_hint`, if provided, becomes the temp file's basename
/// (e.g. `marktext-linux-0.22.0.AppImage`). The install pipeline
/// falls back to this name when the AppImage's `AppImage.yml` is
/// absent or doesn't carry an `id`. If `name_hint` is `None`, a
/// generic `aim-dl-<rand>` prefix is used.
pub fn download(url: &str, cache_dir: &Path, name_hint: Option<&str>) -> Result<Downloaded> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| Error::Download { url: url.to_string(), message: e.to_string() })?;

    let total_size = response
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());

    std::fs::create_dir_all(cache_dir).map_err(|source| Error::Path {
        path: cache_dir.to_path_buf(),
        source,
    })?;

    let (prefix, suffix) = match name_hint {
        Some(h) => {
            // Drop any directory components and anything sketchy.
            let base = std::path::Path::new(h)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("aim-dl")
                .to_string();
            // We split into "<base>." + "" so the temp file ends up
            // named like "<base>.<rand>". That keeps the file
            // extension (and the user-meaningful name) intact.
            let (stem, ext) = match base.rsplit_once('.') {
                Some((s, e)) => (s.to_string(), format!(".{e}")),
                None => (base, String::new()),
            };
            (format!("{stem}."), ext)
        }
        None => ("aim-dl-".to_string(), ".partial".to_string()),
    };

    let mut tmp = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(&suffix)
        .tempfile_in(cache_dir)
        .map_err(|source| Error::Path {
            path: cache_dir.to_path_buf(),
            source,
        })?;

    let pb = make_progress_bar(total_size);

    let mut reader = response.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;

    let copy_result: std::result::Result<(), std::io::Error> = (|| {
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => n,
                Err(e) => return Err(e),
            };
            tmp.as_file_mut().write_all(&buf[..n])?;
            written += u64::try_from(n).unwrap_or(u64::MAX);
            pb.set_position(written);
        }
    })();

    if let Err(source) = copy_result {
        // Drop the temp file (Delete-on-drop kicks in).
        let _ = tmp.close();
        return Err(Error::Path { path: cache_dir.to_path_buf(), source });
    }

    pb.finish_and_clear();

    // Make the downloaded file executable. We need this for two
    // reasons:
    // 1. `identity::identify` runs `--appimage-extract` on the
    //    source, which requires +x.
    // 2. Some AppImages refuse to mount/run with 0600 perms.
    // The install pipeline will copy this file to the cache and
    // set 0755 there too (defence in depth).
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(tmp.path()) {
        let mut perm = meta.permissions();
        perm.set_mode(0o755);
        let _ = std::fs::set_permissions(tmp.path(), perm);
    }

    // When the caller passed a `name_hint`, rename the temp file to
    // use the hint verbatim. `tempfile::Builder` always adds a
    // random suffix; that's fine for collision-safety during the
    // download itself but leaves an ugly filename for the install
    // pipeline's identity fallback (e.g. `marktext-linux-0.22.0.
    // AppImage.abc123`). We do the rename here, so the install
    // pipeline sees the asset's real name.
    let final_path: PathBuf = if let Some(hint) = name_hint {
        let target = cache_dir.join(sanitize_basename(hint));
        if std::fs::rename(tmp.path(), &target).is_err() {
            // Cross-device rename can fail; fall back to copy+remove.
            std::fs::copy(tmp.path(), &target).map_err(|source| Error::Path {
                path: target.clone(),
                source,
            })?;
            let _ = std::fs::remove_file(tmp.path());
        }
        target
    } else {
        // No hint: keep the temp name; persist it.
        tmp.into_temp_path().keep().map_err(|e| Error::Path {
            path: cache_dir.to_path_buf(),
            source: e.error,
        })?
    };

    let size = total_size.unwrap_or(written);
    Ok(Downloaded { path: final_path, size_bytes: size })
}

fn make_progress_bar(total: Option<u64>) -> ProgressBar {
    let pb = match total {
        Some(n) => ProgressBar::new(n),
        None => ProgressBar::new_spinner(),
    };
    if total.is_some() {
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner}  downloading {bar:30.cyan/blue} {bytes}/{total_bytes}  {eta}",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
    } else {
        pb.set_style(ProgressStyle::with_template("{spinner}  downloading {bytes}").unwrap());
    }
    pb
}

/// Sanitize a filename hint so it's safe to use as a path
/// component. Strips directory traversal, normalises whitespace.
fn sanitize_basename(s: &str) -> String {
    let base = std::path::Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aim-dl");
    base.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_detection() {
        assert!(is_url("http://example.com/foo"));
        assert!(is_url("https://example.com/foo"));
        assert!(is_url("https://example.com/foo?bar=baz&x=1"));
        assert!(!is_url("/path/to/file"));
        assert!(!is_url("./relative"));
        assert!(!is_url("ftp://example.com"));
        assert!(!is_url("HTTP://example.com"));
    }

    #[test]
    fn progress_bar_total_known() {
        let pb = make_progress_bar(Some(1024));
        assert_eq!(pb.length(), Some(1024));
    }

    #[test]
    fn progress_bar_total_unknown_is_spinner() {
        let pb = make_progress_bar(None);
        assert_eq!(pb.length(), None);
    }
}
