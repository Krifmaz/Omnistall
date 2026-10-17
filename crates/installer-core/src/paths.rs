//! Where things go on each operating system.
//!
//! Per-user (default, never needs admin):
//! * Windows: `%LOCALAPPDATA%\Programs\<App>` + user Start Menu / Desktop
//! * macOS:   `~/Applications/<App>.app` for bundles, `~/.local/bin` for CLIs
//! * Linux:   `~/.local/share/omnistall/apps/<app>` + `~/.local/bin` +
//!   `~/.local/share/applications/*.desktop` + hicolor icons
//!
//! System-wide (opt-in, only when already elevated):
//! * Windows: `%ProgramFiles%\<App>` + all-users Start Menu
//! * macOS:   `/Applications`, `/usr/local/bin`
//! * Linux:   `/opt/<app>`, `/usr/local/bin`, `/usr/share/applications`

use crate::platform::Os;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    User,
    System,
}

/// Resolved directory layout for a given OS and scope.
#[derive(Debug, Clone)]
pub struct Layout {
    pub os: Os,
    pub scope: Scope,
    pub home: PathBuf,
    /// Root for app payload directories (`<apps_root>/<app>`).
    pub apps_root: PathBuf,
    /// Where macOS `.app` bundles go.
    pub app_bundles_dir: PathBuf,
    /// Directory on PATH that receives symlinks/shims.
    pub bin_dir: PathBuf,
    /// Start Menu / applications directory for launcher entries.
    pub menu_dir: PathBuf,
    /// User's desktop (for optional desktop shortcuts).
    pub desktop_dir: Option<PathBuf>,
    /// Icon theme root (Linux) or an icons folder (others).
    pub icons_dir: PathBuf,
    /// Installer's own state: registry, cache, temp.
    pub data_dir: PathBuf,
}

impl Layout {
    pub fn detect(scope: Scope) -> Result<Self, String> {
        let os = match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            other => return Err(format!("unsupported OS {other}")),
        };
        Self::for_os(os, scope)
    }

    pub fn for_os(os: Os, scope: Scope) -> Result<Self, String> {
        let home = dirs::home_dir().ok_or("could not determine the home directory")?;
        let data_dir = dirs::data_local_dir()
            .ok_or("could not determine the local data directory")?
            .join("omnistall");
        let desktop_dir = dirs::desktop_dir().filter(|d| d.is_dir());

        let layout = match (os, scope) {
            (Os::Windows, Scope::User) => {
                let local = dirs::data_local_dir().ok_or("LOCALAPPDATA not set")?;
                let roaming = dirs::data_dir().ok_or("APPDATA not set")?;
                Layout {
                    os,
                    scope,
                    apps_root: local.join("Programs"),
                    app_bundles_dir: local.join("Programs"),
                    bin_dir: local.join("Programs").join("omnistall").join("bin"),
                    menu_dir: roaming
                        .join("Microsoft")
                        .join("Windows")
                        .join("Start Menu")
                        .join("Programs"),
                    icons_dir: data_dir.join("icons"),
                    desktop_dir,
                    home,
                    data_dir,
                }
            }
            (Os::Windows, Scope::System) => {
                let pf = std::env::var_os("ProgramFiles")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
                let pd = std::env::var_os("ProgramData")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
                Layout {
                    os,
                    scope,
                    apps_root: pf.clone(),
                    app_bundles_dir: pf.clone(),
                    bin_dir: pf.join("omnistall").join("bin"),
                    menu_dir: pd.join("Microsoft").join("Windows").join("Start Menu").join("Programs"),
                    icons_dir: pd.join("omnistall").join("icons"),
                    desktop_dir,
                    home,
                    data_dir: pd.join("omnistall"),
                }
            }
            (Os::Macos, Scope::User) => Layout {
                os,
                scope,
                apps_root: data_dir.join("apps"),
                app_bundles_dir: home.join("Applications"),
                bin_dir: home.join(".local").join("bin"),
                menu_dir: home.join("Applications"),
                icons_dir: data_dir.join("icons"),
                desktop_dir,
                home,
                data_dir,
            },
            (Os::Macos, Scope::System) => Layout {
                os,
                scope,
                apps_root: PathBuf::from("/usr/local/opt/omnistall"),
                app_bundles_dir: PathBuf::from("/Applications"),
                bin_dir: PathBuf::from("/usr/local/bin"),
                menu_dir: PathBuf::from("/Applications"),
                icons_dir: PathBuf::from("/usr/local/share/omnistall/icons"),
                desktop_dir,
                home,
                data_dir: PathBuf::from("/usr/local/share/omnistall"),
            },
            (Os::Linux, Scope::User) => {
                let data = dirs::data_dir().ok_or("XDG data dir unavailable")?;
                Layout {
                    os,
                    scope,
                    apps_root: data_dir.join("apps"),
                    app_bundles_dir: data_dir.join("apps"),
                    bin_dir: home.join(".local").join("bin"),
                    menu_dir: data.join("applications"),
                    icons_dir: data.join("icons").join("hicolor"),
                    desktop_dir,
                    home,
                    data_dir,
                }
            }
            (Os::Linux, Scope::System) => Layout {
                os,
                scope,
                apps_root: PathBuf::from("/opt"),
                app_bundles_dir: PathBuf::from("/opt"),
                bin_dir: PathBuf::from("/usr/local/bin"),
                menu_dir: PathBuf::from("/usr/share/applications"),
                icons_dir: PathBuf::from("/usr/share/icons/hicolor"),
                desktop_dir,
                home,
                data_dir: PathBuf::from("/var/lib/omnistall"),
            },
        };
        Ok(layout)
    }

    /// Default payload directory for an app.
    pub fn app_dir(&self, app_name: &str) -> PathBuf {
        self.apps_root.join(app_name)
    }

    pub fn registry_file(&self) -> PathBuf {
        self.data_dir.join("registry.json")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }
}
