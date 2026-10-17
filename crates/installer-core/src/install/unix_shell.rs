//! PATH management via shell profiles, shared by Linux and macOS.

use super::common::{add_profile_block, on_path, remove_profile_block};
use crate::error::Result;
use crate::paths::Layout;
use std::path::{Path, PathBuf};

fn profiles(layout: &Layout) -> Vec<PathBuf> {
    let home = &layout.home;
    let mut v = vec![home.join(".profile")];
    for f in [".bashrc", ".bash_profile", ".zshrc", ".zprofile"] {
        let p = home.join(f);
        if p.exists() {
            v.push(p);
        }
    }
    // zsh is the macOS default and does not read .profile.
    if layout.os == crate::platform::Os::Macos && !v.iter().any(|p| p.ends_with(".zprofile")) {
        v.push(home.join(".zprofile"));
    }
    v
}

fn marker(dir: &Path) -> String {
    format!("PATH ({})", dir.display())
}

/// Returns `true` if any profile changed.
pub fn add_to_path(layout: &Layout, dir: &Path) -> Result<bool> {
    if on_path(dir) {
        return Ok(false);
    }
    let body = format!("export PATH=\"{}:$PATH\"", dir.display());
    let mut changed = false;
    for p in profiles(layout) {
        changed |= add_profile_block(&p, &marker(dir), &body)?;
    }
    let fish_conf = layout.home.join(".config").join("fish").join("conf.d");
    if fish_conf.is_dir() {
        let f = fish_conf.join(format!(
            "omnistall-{}.fish",
            crate::fsutil::safe_file_name(&dir.display().to_string()).replace(' ', "_")
        ));
        std::fs::write(&f, format!("fish_add_path --global {}\n", dir.display())).ok();
    }
    Ok(changed)
}

pub fn remove_from_path(layout: &Layout, dir: &Path) -> Result<()> {
    for p in profiles(layout) {
        remove_profile_block(&p, &marker(dir))?;
    }
    let fish_conf = layout.home.join(".config").join("fish").join("conf.d");
    let f = fish_conf.join(format!(
        "omnistall-{}.fish",
        crate::fsutil::safe_file_name(&dir.display().to_string()).replace(' ', "_")
    ));
    let _ = std::fs::remove_file(f);
    Ok(())
}

/// Symlink `target` into the layout's bin dir under `name`.
pub fn expose_binary(layout: &Layout, target: &Path, name: &str) -> Result<PathBuf> {
    use crate::error::IoCtx;
    std::fs::create_dir_all(&layout.bin_dir).at(&layout.bin_dir)?;
    let link = layout.bin_dir.join(name);
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link).at(&link)?;
    Ok(link)
}
