//! SHA-256 verification.

use crate::error::{InstallError, IoCtx, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).at(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).at(path)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn verify(path: &Path, expected_hex: &str) -> Result<()> {
    let actual = sha256_file(path)?;
    let expected = expected_hex.trim().to_ascii_lowercase();
    if actual != expected {
        return Err(InstallError::ChecksumMismatch {
            file: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            expected,
            actual,
        });
    }
    Ok(())
}

/// Find the hash for `file_name` in a checksum listing. Accepts GNU
/// (`hash  name`, `hash *name`), BSD (`SHA256 (name) = hash`), `name: hash`,
/// and a bare 64-hex-digit file.
pub fn find_in_sums(text: &str, file_name: &str) -> Option<String> {
    let is_hash = |s: &str| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit());
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.len() == 1 {
        let mut tokens = lines[0].split_whitespace();
        if let (Some(first), None) = (tokens.next(), tokens.next()) {
            if is_hash(first) {
                return Some(first.to_ascii_lowercase());
            }
        }
    }
    for line in &lines {
        // BSD style
        if let Some(rest) = line.strip_prefix("SHA256 (") {
            if let Some((name, hash)) = rest.split_once(") = ") {
                if name_matches(name, file_name) && is_hash(hash.trim()) {
                    return Some(hash.trim().to_ascii_lowercase());
                }
            }
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let (a, b) = (parts[0], parts[parts.len() - 1]);
            if is_hash(a) && name_matches(b.trim_start_matches('*'), file_name) {
                return Some(a.to_ascii_lowercase());
            }
            if is_hash(b) && name_matches(a.trim_end_matches(':'), file_name) {
                return Some(b.to_ascii_lowercase());
            }
        }
    }
    None
}

fn name_matches(candidate: &str, file_name: &str) -> bool {
    candidate == file_name || candidate.rsplit(['/', '\\']).next() == Some(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify(&p, "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD").is_ok());
        assert!(matches!(
            verify(&p, &"0".repeat(64)),
            Err(InstallError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn parses_sum_files() {
        let h = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(
            find_in_sums(&format!("{h}  app.tar.gz\n"), "app.tar.gz").as_deref(),
            Some(h)
        );
        assert_eq!(
            find_in_sums(&format!("{h} *dist/app.tar.gz"), "app.tar.gz").as_deref(),
            Some(h)
        );
        assert_eq!(
            find_in_sums(&format!("SHA256 (app.tar.gz) = {h}"), "app.tar.gz").as_deref(),
            Some(h)
        );
        assert_eq!(find_in_sums(&format!("{h}\n"), "whatever").as_deref(), Some(h));
        assert_eq!(find_in_sums(&format!("{h}  other.zip"), "app.tar.gz"), None);
    }
}
