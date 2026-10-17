//! Local database of installed apps (`registry.json` in the data dir).

use crate::error::{InstallError, IoCtx, Result};
use crate::manifest::ManifestOrigin;
use crate::options::InstallOptions;
use crate::paths::Scope;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethod {
    /// Files unpacked into `install_dir`.
    Payload,
    /// A macOS `.app` copied into the Applications folder.
    AppBundle,
    Msi,
    ExeInstaller,
    Pkg,
    DebDpkg,
    AppImage,
    Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub platform_key: String,
    pub scope: Scope,
    pub method: InstallMethod,
    pub install_dir: Option<PathBuf>,
    /// Main executable (absolute) used by "Launch".
    pub executable: Option<PathBuf>,
    pub app_bundle: Option<PathBuf>,
    /// Files created outside `install_dir` (symlinks, shortcuts, icons…).
    #[serde(default)]
    pub artifacts: Vec<PathBuf>,
    /// Directories we added to PATH.
    #[serde(default)]
    pub path_entries: Vec<PathBuf>,
    /// Windows registry keys we created (file associations).
    #[serde(default)]
    pub registry_keys: Vec<String>,
    /// Linux mime types registered.
    #[serde(default)]
    pub mime_types: Vec<String>,
    pub native_uninstall: Option<String>,
    pub deb_package: Option<String>,
    /// Cached `.msi` so it can be used for `msiexec /x`.
    pub msi_cache: Option<PathBuf>,
    pub manifest_origin: ManifestOrigin,
    /// Copy of the manifest used, so uninstall/repair work offline.
    pub manifest_toml: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub installed_at: chrono::DateTime<chrono::Utc>,
    pub options: InstallOptions,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default = "schema_v")]
    pub schema: u32,
    #[serde(default)]
    pub apps: BTreeMap<String, InstalledApp>,
    #[serde(skip)]
    path: PathBuf,
}

fn schema_v() -> u32 {
    1
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema: schema_v(),
            apps: BTreeMap::new(),
            path: PathBuf::new(),
        }
    }
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self> {
        let mut reg = if path.exists() {
            let text = std::fs::read_to_string(path).at(path)?;
            serde_json::from_str::<Registry>(&text)
                .map_err(|e| InstallError::Other(anyhow::anyhow!("registry {} is corrupt: {e}", path.display())))?
        } else {
            Registry::default()
        };
        reg.path = path.to_path_buf();
        Ok(reg)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| InstallError::Other(anyhow::anyhow!("serialising registry: {e}")))?;
        std::fs::write(&tmp, text).at(&tmp)?;
        std::fs::rename(&tmp, &self.path).at(&self.path)?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&InstalledApp> {
        self.apps.get(name)
    }

    pub fn insert(&mut self, app: InstalledApp) {
        self.apps.insert(app.name.clone(), app);
    }

    pub fn remove(&mut self, name: &str) -> Option<InstalledApp> {
        self.apps.remove(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &InstalledApp> {
        self.apps.values()
    }

    /// Apps that depend on `name`.
    pub fn dependents_of(&self, name: &str) -> Vec<String> {
        self.apps
            .values()
            .filter(|a| a.dependencies.iter().any(|d| d == name))
            .map(|a| a.name.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.json");
        let mut r = Registry::load(&p).unwrap();
        r.insert(InstalledApp {
            name: "demo".into(),
            display_name: "Demo".into(),
            version: "1.0".into(),
            platform_key: "linux-x86_64".into(),
            scope: Scope::User,
            method: InstallMethod::Payload,
            install_dir: Some(PathBuf::from("/x")),
            executable: None,
            app_bundle: None,
            artifacts: vec![],
            path_entries: vec![],
            registry_keys: vec![],
            mime_types: vec![],
            native_uninstall: None,
            deb_package: None,
            msi_cache: None,
            manifest_origin: ManifestOrigin::Embedded,
            manifest_toml: String::new(),
            dependencies: vec![],
            installed_at: chrono::Utc::now(),
            options: InstallOptions::default(),
        });
        r.save().unwrap();
        let r2 = Registry::load(&p).unwrap();
        assert_eq!(r2.schema, 1);
        assert_eq!(r2.get("demo").unwrap().version, "1.0");
    }
}
