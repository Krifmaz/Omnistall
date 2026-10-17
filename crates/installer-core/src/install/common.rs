//! Pieces shared by the per-OS integration modules.

use crate::error::{IoCtx, Result};
use std::path::{Path, PathBuf};

/// What a shortcut should point at.
#[derive(Debug, Clone)]
pub struct ShortcutRequest<'a> {
    pub app_id: &'a str,
    pub name: &'a str,
    pub exec: &'a Path,
    pub args: Option<&'a str>,
    pub icon: Option<&'a Path>,
    pub comment: Option<&'a str>,
    pub categories: &'a [String],
    pub terminal: bool,
    pub mime_types: &'a [String],
    pub working_dir: Option<&'a Path>,
}

/// Result of a platform-native install step (dmg/pkg/msi/exe/deb/AppImage).
pub struct NativeOutcome {
    pub method: crate::registry::InstallMethod,
    pub executable: Option<PathBuf>,
    pub app_bundle: Option<PathBuf>,
    pub deb_package: Option<String>,
    pub msi_cache: Option<PathBuf>,
    pub native_uninstall: Option<String>,
}

/// Expand `%VAR%`, `$VAR`, `${VAR}` and a leading `~`.
pub fn expand_env(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    if let Some(stripped) = rest.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            out.push_str(&home.to_string_lossy());
            rest = stripped;
        }
    }
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                let name: String = chars.by_ref().take_while(|&ch| ch != '%').collect();
                match std::env::var(&name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(&name);
                        out.push('%');
                    }
                }
            }
            '$' => {
                let braced = chars.peek() == Some(&'{');
                if braced {
                    chars.next();
                }
                let mut name = String::new();
                while let Some(&ch) = chars.peek() {
                    if braced && ch == '}' {
                        chars.next();
                        break;
                    }
                    if !braced && !(ch.is_alphanumeric() || ch == '_') {
                        break;
                    }
                    name.push(ch);
                    chars.next();
                }
                match std::env::var(&name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('$');
                        out.push_str(&name);
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Is `dir` already on the process PATH?
pub fn on_path(dir: &Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let want = normalize(dir);
    std::env::split_paths(&path).any(|p| normalize(&p) == want)
}

fn normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let trimmed = s.trim_end_matches(['/', '\\']);
    #[cfg(windows)]
    {
        PathBuf::from(trimmed.to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(trimmed)
    }
}

/// Insert a marked block into a shell profile (idempotent).
pub fn add_profile_block(file: &Path, marker: &str, body: &str) -> Result<bool> {
    let start = format!("# >>> omnistall {marker} >>>");
    let end = format!("# <<< omnistall {marker} <<<");
    let existing = std::fs::read_to_string(file).unwrap_or_default();
    if existing.contains(&start) {
        return Ok(false);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("{start}\n{body}\n{end}\n"));
    std::fs::write(file, text).at(file)?;
    Ok(true)
}

pub fn remove_profile_block(file: &Path, marker: &str) -> Result<()> {
    let Ok(existing) = std::fs::read_to_string(file) else {
        return Ok(());
    };
    let start = format!("# >>> omnistall {marker} >>>");
    let end = format!("# <<< omnistall {marker} <<<");
    if !existing.contains(&start) {
        return Ok(());
    }
    let mut out = String::new();
    let mut skipping = false;
    for line in existing.lines() {
        if line == start {
            skipping = true;
            continue;
        }
        if line == end {
            skipping = false;
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(file, out).at(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_blocks() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".profile");
        std::fs::write(&f, "export A=1").unwrap();
        assert!(add_profile_block(&f, "PATH (x)", "export PATH=\"$HOME/.local/bin:$PATH\"").unwrap());
        assert!(!add_profile_block(&f, "PATH (x)", "whatever").unwrap());
        let t = std::fs::read_to_string(&f).unwrap();
        assert!(t.contains(">>> omnistall PATH (x) >>>"));
        remove_profile_block(&f, "PATH (x)").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "export A=1\n");
    }

    #[test]
    fn env_expansion() {
        // SAFETY: test-only, single-threaded use of a unique variable name.
        unsafe { std::env::set_var("OMNI_TEST_VAR", "val") };
        assert_eq!(expand_env("a/%OMNI_TEST_VAR%/b"), "a/val/b");
        assert_eq!(expand_env("$OMNI_TEST_VAR/x"), "val/x");
        assert_eq!(expand_env("${OMNI_TEST_VAR}y"), "valy");
        assert_eq!(expand_env("%NOPE_NOT_SET%"), "%NOPE_NOT_SET%");
    }
}
