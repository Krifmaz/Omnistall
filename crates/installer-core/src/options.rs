//! User-facing install choices (the GUI's "Options" section).

use crate::paths::Scope;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOptions {
    /// Override the payload directory. `None` = per-OS default.
    pub install_dir: Option<PathBuf>,
    pub desktop_shortcut: bool,
    pub menu_shortcut: bool,
    /// `None` = manifest decides (true for CLI apps).
    pub add_to_path: Option<bool>,
    /// Build from source even if a prebuilt download exists.
    pub from_source: bool,
    pub scope: Scope,
    /// Reinstall even when the same version is already present.
    pub force: bool,
}

impl Default for InstallOptions {
    fn default() -> Self {
        Self {
            install_dir: None,
            desktop_shortcut: true,
            menu_shortcut: true,
            add_to_path: None,
            from_source: false,
            scope: Scope::User,
            force: false,
        }
    }
}
