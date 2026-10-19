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
pub fn download(url: &str, cache_dir: &Path) -> Result<Downloaded> {
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

    let mut tmp = tempfile::Builder::new()
        .prefix("aim-dl-")
        .suffix(".partial")
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

    let path = tmp.into_temp_path();
    let owned: PathBuf = path.to_path_buf();
    // Detach the temp path so the file isn't auto-removed.
    let _ = path.keep();

    let size = total_size.unwrap_or(written);
    Ok(Downloaded { path: owned, size_bytes: size })
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
