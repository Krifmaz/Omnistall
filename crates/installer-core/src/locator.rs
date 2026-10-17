//! Find the manifest a double-clicked installer should install.
//!
//! Search order:
//! 0. A `.omni` / `.toml` document the OS handed us (double-clicked file,
//!    "Open with…", or dropped on the installer icon): the first argument
//!    that is an existing file.
//! 1. `OMNISTALL_MANIFEST` environment variable (path or URL).
//! 2. A manifest appended to the installer binary by `xtask brand`
//!    (a TOML blob followed by an 8-byte length and a magic trailer). On Linux
//!    the `$APPIMAGE` file is checked too, because `current_exe()` points at
//!    the mounted copy inside the AppImage.
//! 3. A sidecar file next to the executable / inside the bundle:
//!    * Windows: `<exe dir>\manifest.toml`
//!    * macOS:   `<App>.app/Contents/Resources/manifest.toml`
//!    * Linux:   `$APPDIR/manifest.toml`, `<exe dir>/manifest.toml`,
//!      `/usr/share/omnistall/manifest.toml` (deb package)
//!
//! If nothing is found the GUI asks the user for a manifest path or URL.

use crate::manifest::ManifestOrigin;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const TRAILER_MAGIC: &[u8; 16] = b"OMNISTALL-MANIF1";

#[derive(Debug, Clone)]
pub struct Located {
    pub text: String,
    pub origin: ManifestOrigin,
}

pub fn discover() -> Option<Located> {
    if let Some(p) = document_argument() {
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(Located {
                text,
                origin: ManifestOrigin::Path(p),
            });
        }
    }
    if let Some(v) = std::env::var_os("OMNISTALL_MANIFEST") {
        let v = v.to_string_lossy().to_string();
        if v.starts_with("http://") || v.starts_with("https://") {
            // Fetched lazily by the caller; signal with an empty body.
            return Some(Located {
                text: String::new(),
                origin: ManifestOrigin::Url(v),
            });
        }
        let p = PathBuf::from(&v);
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(Located {
                text,
                origin: ManifestOrigin::Path(p),
            });
        }
    }

    for candidate in self_binaries() {
        if let Some(text) = read_trailer(&candidate) {
            return Some(Located {
                text,
                origin: ManifestOrigin::Embedded,
            });
        }
    }

    for p in sidecar_candidates() {
        if let Ok(text) = std::fs::read_to_string(&p) {
            return Some(Located {
                text,
                origin: ManifestOrigin::Path(p),
            });
        }
    }
    None
}

/// A manifest document passed by the OS when the user opened a `.omni` file.
pub fn document_argument() -> Option<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .find(|p| crate::selfreg::is_manifest_document(p))
        .and_then(|p| std::fs::canonicalize(&p).ok().or(Some(p)))
}

fn self_binaries() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        v.push(PathBuf::from(appimage));
    }
    if let Ok(exe) = std::env::current_exe() {
        v.push(exe);
    }
    v
}

fn sidecar_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(appdir) = std::env::var_os("APPDIR") {
        v.push(PathBuf::from(appdir).join("manifest.toml"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("manifest.toml"));
            // macOS bundle: Contents/MacOS/<bin> -> Contents/Resources
            if dir.file_name().is_some_and(|n| n == "MacOS") {
                if let Some(contents) = dir.parent() {
                    v.push(contents.join("Resources").join("manifest.toml"));
                }
            }
        }
    }
    v.push(PathBuf::from("/usr/share/omnistall/manifest.toml"));
    v.push(PathBuf::from("/usr/local/share/omnistall/manifest.toml"));
    v
}

/// Read a manifest appended with [`append_trailer`].
pub fn read_trailer(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let footer = 8 + TRAILER_MAGIC.len() as u64;
    if len < footer {
        return None;
    }
    f.seek(SeekFrom::End(-(footer as i64))).ok()?;
    let mut buf = [0u8; 8 + 16];
    f.read_exact(&mut buf).ok()?;
    if &buf[8..] != TRAILER_MAGIC {
        return None;
    }
    let body_len = u64::from_le_bytes(buf[..8].try_into().ok()?);
    if body_len == 0 || body_len > 8 * 1024 * 1024 || body_len + footer > len {
        return None;
    }
    f.seek(SeekFrom::End(-((footer + body_len) as i64))).ok()?;
    let mut body = vec![0u8; body_len as usize];
    f.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// Append a manifest to a copy of an installer binary. Used by `xtask brand`.
pub fn append_trailer(input: &Path, manifest_toml: &str, output: &Path) -> std::io::Result<()> {
    let mut data = std::fs::read(input)?;
    data.extend_from_slice(manifest_toml.as_bytes());
    data.extend_from_slice(&(manifest_toml.len() as u64).to_le_bytes());
    data.extend_from_slice(TRAILER_MAGIC);
    std::fs::write(output, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::metadata(input)?.permissions();
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(perm.mode() | 0o111))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("bin");
        std::fs::write(&input, b"\x7fELF fake binary").unwrap();
        let out = dir.path().join("branded");
        append_trailer(&input, "[app]\nname='x'", &out).unwrap();
        assert_eq!(read_trailer(&out).as_deref(), Some("[app]\nname='x'"));
        assert!(read_trailer(&input).is_none());
    }
}
