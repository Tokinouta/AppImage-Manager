//! Streaming SHA-256 hash of a file. Used by `install` (to record
//! the canonical hash in `state.toml`), `verify` (to compare the
//! cached file against the recorded hash), and `upgrade` (to hash
//! the freshly downloaded asset).

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Streaming SHA-256 of `path`. Returns `(hex_sha256, size_bytes)`.
pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let file = std::fs::File::open(path).map_err(|source| Error::Path {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    let digest = hasher.finalize();
    Ok((hex::encode(digest), total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_bytes(path: &Path, body: &[u8]) {
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(body).unwrap();
    }

    #[test]
    fn hash_known_short_input() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aim-hash-{nanos}"));
        write_bytes(&path, b"hello world");
        let (hex, size) = hash_file(&path).unwrap();
        assert_eq!(
            hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(size, 11);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn hash_empty_file() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aim-hash-empty-{nanos}"));
        write_bytes(&path, b"");
        let (hex, size) = hash_file(&path).unwrap();
        // SHA-256 of zero bytes.
        assert_eq!(
            hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(size, 0);
        std::fs::remove_file(&path).ok();
    }
}
