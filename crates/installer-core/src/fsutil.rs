//! Small filesystem helpers.

use crate::error::{IoCtx, Result};
use std::path::Path;

/// Recursive copy preserving symlinks (important for macOS `.app` bundles).
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).at(dst)?;
    for entry in walkdir::WalkDir::new(src).min_depth(1).follow_links(false) {
        let entry =
            entry.map_err(|e| crate::error::InstallError::Other(anyhow::anyhow!("walking {}: {e}", src.display())))?;
        let rel = entry.path().strip_prefix(src).unwrap();
        let target = dst.join(rel);
        let ft = entry.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&target).at(&target)?;
        } else if ft.is_symlink() {
            let link = std::fs::read_link(entry.path()).at(entry.path())?;
            let _ = std::fs::remove_file(&target);
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target).at(&target)?;
            #[cfg(windows)]
            {
                // Symlinks need privileges on Windows; copy the target instead.
                let resolved = entry.path().parent().unwrap_or(src).join(&link);
                if resolved.is_dir() {
                    copy_dir(&resolved, &target)?;
                } else if resolved.is_file() {
                    std::fs::copy(&resolved, &target).at(&target)?;
                }
            }
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).at(parent)?;
            }
            std::fs::copy(entry.path(), &target).at(&target)?;
        }
    }
    Ok(())
}

/// Replace `dst` with `src` atomically where possible (rename, falling back
/// to copy+delete across filesystems).
pub fn replace_dir(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        remove_path(dst)?;
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            copy_dir(src, dst)?;
            std::fs::remove_dir_all(src).at(src)?;
            Ok(())
        }
    }
}

/// Remove a file, symlink or directory tree. Missing paths are fine.
pub fn remove_path(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).at(path),
        Ok(_) => std::fs::remove_file(path).at(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::error::InstallError::io(path, e)),
    }
}

/// Sanitize a string for use as a file name.
pub fn safe_file_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}
