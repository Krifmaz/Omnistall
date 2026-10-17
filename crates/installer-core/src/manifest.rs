//! The per-app manifest (TOML) schema, loading and validation.
//!
//! See `docs/MANIFEST.md` for the human-readable reference. Everything here is
//! plain data; the install engine interprets it.

use crate::error::{InstallError, Result};
use crate::platform::{Os, Platform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;

/// Where a manifest came from. Used to resolve relative paths and to re-fetch
/// it later for updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum ManifestOrigin {
    Url(String),
    Path(PathBuf),
    /// Embedded in the installer binary / bundle.
    Embedded,
}

impl ManifestOrigin {
    pub fn describe(&self) -> String {
        match self {
            ManifestOrigin::Url(u) => u.clone(),
            ManifestOrigin::Path(p) => p.display().to_string(),
            ManifestOrigin::Embedded => "embedded manifest".to_string(),
        }
    }

    /// Resolve a possibly-relative reference (dependency manifest, icon file)
    /// against this origin.
    pub fn resolve(&self, reference: &str) -> String {
        if reference.starts_with("http://") || reference.starts_with("https://") {
            return reference.to_string();
        }
        if Path::new(reference).is_absolute() {
            return reference.to_string();
        }
        match self {
            ManifestOrigin::Url(base) => {
                let trimmed = base.rsplit_once('/').map(|(b, _)| b).unwrap_or(base);
                format!("{trimmed}/{reference}")
            }
            ManifestOrigin::Path(p) => p
                .parent()
                .map(|d| d.join(reference).display().to_string())
                .unwrap_or_else(|| reference.to_string()),
            ManifestOrigin::Embedded => reference.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default = "default_schema")]
    pub schema: u32,
    pub app: AppInfo,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// Keyed by platform: `linux-x86_64`, `macos-aarch64`, `windows`, `any`…
    #[serde(default)]
    pub sources: BTreeMap<String, Source>,
    #[serde(default)]
    pub build: Option<BuildSpec>,
    #[serde(default)]
    pub install: InstallSpec,
    #[serde(default)]
    pub shortcuts: Vec<Shortcut>,
    #[serde(default)]
    pub file_associations: Vec<FileAssociation>,
    #[serde(default)]
    pub hooks: Hooks,
    /// Extra toolchains the manifest defines itself (beyond the built-ins).
    #[serde(default)]
    pub toolchains: BTreeMap<String, CustomToolchain>,
}

fn default_schema() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppInfo {
    /// Machine id: lowercase letters, digits, `.`, `_`, `-`. Used for folder
    /// names and the registry.
    pub name: String,
    /// Human-facing name ("Visual Studio Code"). Defaults to `name`.
    #[serde(default)]
    pub display_name: Option<String>,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    /// URL or path (relative to the manifest) of a PNG/SVG/ICO icon.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub kind: AppKind,
}

impl AppInfo {
    pub fn display_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    /// Command-line tool: binaries are put on PATH.
    Cli,
    /// Graphical app: shortcuts / .app bundle / Start Menu entry.
    #[default]
    Gui,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub name: String,
    /// URL or path (relative to this manifest) of the dependency's manifest.
    pub manifest: String,
    /// Optional semver requirement, e.g. `">=1.2"`.
    #[serde(default)]
    pub version: Option<String>,
}

/// One way of obtaining the app for one platform.
/// (No `deny_unknown_fields`: serde cannot combine it with a flattened,
/// internally tagged enum.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    #[serde(flatten)]
    pub kind: SourceKind,
    /// Hex SHA-256 of the downloaded file (archive, installer or raw binary).
    #[serde(default)]
    pub sha256: Option<String>,
    /// Override format detection (`zip`, `tar.gz`, `dmg`, `msi`, `raw`, …).
    #[serde(default)]
    pub format: Option<String>,
    /// Strip leading path components when extracting archives.
    #[serde(default)]
    pub strip_components: Option<u32>,
    /// For `type = "git"` / source tarballs: this source must be built.
    #[serde(default)]
    pub build: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SourceKind {
    /// Direct download. `file://` URLs and plain paths are also accepted.
    Url { url: String },
    /// Asset from a GitHub release.
    Github {
        /// `owner/repo`
        repo: String,
        /// `latest`, or a tag; `{version}` is expanded. Defaults to `v{version}`.
        #[serde(default)]
        tag: Option<String>,
        /// Asset file name; `*` and `?` wildcards allowed.
        asset: String,
        /// Optional checksum file asset (SHA256SUMS style). Used when `sha256`
        /// is absent.
        #[serde(default)]
        checksum_asset: Option<String>,
    },
    /// Clone a git repository and build it.
    Git {
        url: String,
        /// Branch, tag or commit. `{version}` is expanded.
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        submodules: bool,
    },
}

/// Build-from-source recipe. Per-OS overrides live under `[build.linux]` etc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    #[serde(default)]
    pub toolchains: Vec<ToolchainReq>,
    /// Shell commands run in the source directory, in order.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Files/directories to copy into the install directory afterwards.
    #[serde(default)]
    pub outputs: Vec<BuildOutput>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub linux: Option<Box<BuildSpec>>,
    #[serde(default)]
    pub macos: Option<Box<BuildSpec>>,
    #[serde(default)]
    pub windows: Option<Box<BuildSpec>>,
}

impl BuildSpec {
    /// Merge OS overrides on top of the generic spec.
    pub fn for_os(&self, os: Os) -> BuildSpec {
        let over = match os {
            Os::Linux => self.linux.as_deref(),
            Os::Macos => self.macos.as_deref(),
            Os::Windows => self.windows.as_deref(),
        };
        let mut out = BuildSpec {
            toolchains: self.toolchains.clone(),
            commands: self.commands.clone(),
            outputs: self.outputs.clone(),
            env: self.env.clone(),
            ..Default::default()
        };
        if let Some(o) = over {
            if !o.toolchains.is_empty() {
                out.toolchains = o.toolchains.clone();
            }
            if !o.commands.is_empty() {
                out.commands = o.commands.clone();
            }
            if !o.outputs.is_empty() {
                out.outputs = o.outputs.clone();
            }
            out.env.extend(o.env.clone());
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolchainReq {
    Name(String),
    Detailed {
        name: String,
        #[serde(default)]
        version: Option<String>,
    },
}

impl ToolchainReq {
    pub fn name(&self) -> &str {
        match self {
            ToolchainReq::Name(n) => n,
            ToolchainReq::Detailed { name, .. } => name,
        }
    }
    pub fn version_req(&self) -> Option<&str> {
        match self {
            ToolchainReq::Name(_) => None,
            ToolchainReq::Detailed { version, .. } => version.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildOutput {
    /// Path relative to the source directory (e.g. `target/release/foo`).
    pub path: String,
    /// Destination relative to the install directory. Defaults to the file name.
    #[serde(default)]
    pub install_as: Option<String>,
}

/// A manifest-defined toolchain (for tools the built-in table doesn't know).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomToolchain {
    /// Executable(s) whose presence means the toolchain is installed.
    pub binaries: Vec<String>,
    /// Command printing a version, e.g. `zig version`.
    #[serde(default)]
    pub version_command: Option<String>,
    /// Shell command per OS that installs it.
    #[serde(default)]
    pub install: BTreeMap<String, String>,
}

/// How the fetched payload becomes an installation. Per-OS overrides under
/// `[install.linux]`, `[install.macos]`, `[install.windows]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallSpec {
    /// Executables (relative to the unpacked payload) to expose on PATH /
    /// use for shortcuts. Defaults to `[app.name]` when such a file exists.
    #[serde(default)]
    pub bin: Vec<String>,
    /// Main executable used by shortcuts and "Launch". Defaults to first `bin`.
    #[serde(default)]
    pub executable: Option<String>,
    /// macOS: name of the `.app` bundle inside the payload (auto-detected).
    #[serde(default)]
    pub app_bundle: Option<String>,
    /// Add the bin directory to the user's PATH (default: true for CLI apps).
    #[serde(default)]
    pub add_to_path: Option<bool>,
    /// Extra args for `.exe` installers (e.g. `["/S"]` for NSIS, `["/VERYSILENT"]` for Inno).
    #[serde(default)]
    pub installer_args: Vec<String>,
    /// MSI properties (`NAME=VALUE`).
    #[serde(default)]
    pub msi_properties: Vec<String>,
    /// Command that uninstalls a native (`.exe`/`.pkg`) installation.
    #[serde(default)]
    pub uninstall_command: Option<String>,
    /// Explicit file copies for unusual layouts.
    #[serde(default)]
    pub files: Vec<FileCopy>,
    #[serde(default)]
    pub linux: Option<Box<InstallSpec>>,
    #[serde(default)]
    pub macos: Option<Box<InstallSpec>>,
    #[serde(default)]
    pub windows: Option<Box<InstallSpec>>,
}

impl InstallSpec {
    pub fn for_os(&self, os: Os) -> InstallSpec {
        let over = match os {
            Os::Linux => self.linux.as_deref(),
            Os::Macos => self.macos.as_deref(),
            Os::Windows => self.windows.as_deref(),
        };
        let mut out = InstallSpec {
            bin: self.bin.clone(),
            executable: self.executable.clone(),
            app_bundle: self.app_bundle.clone(),
            add_to_path: self.add_to_path,
            installer_args: self.installer_args.clone(),
            msi_properties: self.msi_properties.clone(),
            uninstall_command: self.uninstall_command.clone(),
            files: self.files.clone(),
            ..Default::default()
        };
        if let Some(o) = over {
            if !o.bin.is_empty() {
                out.bin = o.bin.clone();
            }
            if o.executable.is_some() {
                out.executable = o.executable.clone();
            }
            if o.app_bundle.is_some() {
                out.app_bundle = o.app_bundle.clone();
            }
            if o.add_to_path.is_some() {
                out.add_to_path = o.add_to_path;
            }
            if !o.installer_args.is_empty() {
                out.installer_args = o.installer_args.clone();
            }
            if !o.msi_properties.is_empty() {
                out.msi_properties = o.msi_properties.clone();
            }
            if o.uninstall_command.is_some() {
                out.uninstall_command = o.uninstall_command.clone();
            }
            if !o.files.is_empty() {
                out.files = o.files.clone();
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCopy {
    pub src: String,
    pub dst: String,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shortcut {
    /// Display name. Defaults to the app display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Executable relative to the install dir. Defaults to `install.executable`.
    #[serde(default)]
    pub exec: Option<String>,
    #[serde(default)]
    pub args: Option<String>,
    /// Icon relative to the install dir, or omitted to use the app icon.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    /// freedesktop categories (Linux).
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileAssociation {
    /// e.g. `.md`
    pub extension: String,
    /// e.g. `text/markdown`
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Shell commands run around install / uninstall. Per-OS overrides under
/// `[hooks.linux]` etc. replace the whole list for that event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    #[serde(default)]
    pub pre_install: Vec<String>,
    #[serde(default)]
    pub post_install: Vec<String>,
    #[serde(default)]
    pub pre_uninstall: Vec<String>,
    #[serde(default)]
    pub post_uninstall: Vec<String>,
    #[serde(default)]
    pub linux: Option<Box<Hooks>>,
    #[serde(default)]
    pub macos: Option<Box<Hooks>>,
    #[serde(default)]
    pub windows: Option<Box<Hooks>>,
}

impl Hooks {
    pub fn for_os(&self, os: Os) -> Hooks {
        let over = match os {
            Os::Linux => self.linux.as_deref(),
            Os::Macos => self.macos.as_deref(),
            Os::Windows => self.windows.as_deref(),
        };
        let pick = |base: &Vec<String>, o: Option<&Vec<String>>| -> Vec<String> {
            match o {
                Some(v) if !v.is_empty() => v.clone(),
                _ => base.clone(),
            }
        };
        Hooks {
            pre_install: pick(&self.pre_install, over.map(|o| &o.pre_install)),
            post_install: pick(&self.post_install, over.map(|o| &o.post_install)),
            pre_uninstall: pick(&self.pre_uninstall, over.map(|o| &o.pre_uninstall)),
            post_uninstall: pick(&self.post_uninstall, over.map(|o| &o.post_uninstall)),
            ..Default::default()
        }
    }
}

/// A source chosen for the running platform.
#[derive(Debug, Clone)]
pub struct SelectedSource<'a> {
    pub key: String,
    pub source: &'a Source,
    /// True when the match relies on CPU emulation (Rosetta 2 / Windows x64
    /// emulation).
    pub emulated: bool,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(text).map_err(|e| InstallError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA_VERSION {
            return Err(InstallError::Manifest(format!(
                "unsupported schema version {} (this installer supports {})",
                self.schema, SCHEMA_VERSION
            )));
        }
        let name = &self.app.name;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        {
            return Err(InstallError::Manifest(format!(
                "app.name {name:?} must be lowercase letters, digits, '.', '_' or '-'"
            )));
        }
        if self.app.version.trim().is_empty() {
            return Err(InstallError::Manifest("app.version is required".into()));
        }
        if self.sources.is_empty() && self.build.is_none() {
            return Err(InstallError::Manifest(
                "manifest has neither [sources.*] nor [build]".into(),
            ));
        }
        for (key, src) in &self.sources {
            if !is_valid_source_key(key) {
                return Err(InstallError::Manifest(format!(
                    "unknown source key {key:?}; expected e.g. linux-x86_64, macos-aarch64, windows, any"
                )));
            }
            if let Some(sum) = &src.sha256 {
                if sum.len() != 64 || !sum.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(InstallError::Manifest(format!(
                        "sources.{key}.sha256 must be 64 hex characters"
                    )));
                }
            }
            if let SourceKind::Github { repo, .. } = &src.kind {
                if repo.split('/').count() != 2 {
                    return Err(InstallError::Manifest(format!("sources.{key}.repo must be owner/name")));
                }
            }
            if (src.build || matches!(src.kind, SourceKind::Git { .. })) && self.build.is_none() {
                return Err(InstallError::Manifest(format!(
                    "sources.{key} requires building but the manifest has no [build] section"
                )));
            }
        }
        for assoc in &self.file_associations {
            if !assoc.extension.starts_with('.') {
                return Err(InstallError::Manifest(format!(
                    "file association extension {:?} must start with '.'",
                    assoc.extension
                )));
            }
        }
        Ok(())
    }

    /// Choose the best source for the platform. `prefer_build` makes git /
    /// build sources win over prebuilt downloads.
    pub fn select_source(&self, platform: &Platform, prefer_build: bool) -> Option<SelectedSource<'_>> {
        let candidates = platform.candidate_keys();
        let pick = |want_build: bool| -> Option<SelectedSource<'_>> {
            for (key, emulated) in &candidates {
                if let Some(src) = self.sources.get(key) {
                    let is_build = src.build || matches!(src.kind, SourceKind::Git { .. });
                    if is_build == want_build {
                        return Some(SelectedSource {
                            key: key.clone(),
                            source: src,
                            emulated: *emulated,
                        });
                    }
                }
            }
            None
        };
        if prefer_build {
            pick(true).or_else(|| pick(false))
        } else {
            pick(false).or_else(|| pick(true))
        }
    }

    /// Expand `{version}`, `{name}`, `{os}`, `{arch}`, `{platform}`, `{triple}`.
    pub fn expand(&self, text: &str, platform: &Platform) -> String {
        text.replace("{version}", &self.app.version)
            .replace("{name}", &self.app.name)
            .replace("{os}", platform.os.as_str())
            .replace("{arch}", platform.arch.as_str())
            .replace("{platform}", &platform.key())
            .replace("{triple}", &platform.triple())
    }

    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(self.app.version.trim_start_matches('v')).ok()
    }
}

fn is_valid_source_key(key: &str) -> bool {
    const OS: [&str; 3] = ["linux", "macos", "windows"];
    const ARCH: [&str; 2] = ["x86_64", "aarch64"];
    if key == "any" || OS.contains(&key) {
        return true;
    }
    match key.split_once('-') {
        Some((os, arch)) => OS.contains(&os) && ARCH.contains(&arch),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Arch;

    const SAMPLE: &str = r#"
schema = 1
[app]
name = "demo"
version = "1.2.3"
kind = "cli"

[sources.linux-x86_64]
type = "url"
url = "https://example.com/demo-{version}-{triple}.tar.gz"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
strip_components = 1

[sources.any]
type = "git"
url = "https://example.com/demo.git"
rev = "v{version}"

[build]
toolchains = ["rust"]
commands = ["cargo build --release"]
outputs = [{ path = "target/release/demo" }]
[build.windows]
outputs = [{ path = "target/release/demo.exe" }]
"#;

    #[test]
    fn parses_and_selects() {
        let m = Manifest::parse(SAMPLE).unwrap();
        let linux = Platform::new(Os::Linux, Arch::X86_64);
        let s = m.select_source(&linux, false).unwrap();
        assert_eq!(s.key, "linux-x86_64");
        let s = m.select_source(&linux, true).unwrap();
        assert_eq!(s.key, "any");
        let mac = Platform::new(Os::Macos, Arch::Aarch64);
        let s = m.select_source(&mac, false).unwrap();
        assert_eq!(s.key, "any");
        assert_eq!(
            m.expand("x-{version}-{triple}", &linux),
            "x-1.2.3-x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            m.build.as_ref().unwrap().for_os(Os::Windows).outputs[0].path,
            "target/release/demo.exe"
        );
        assert_eq!(
            m.build.as_ref().unwrap().for_os(Os::Linux).outputs[0].path,
            "target/release/demo"
        );
    }

    #[test]
    fn rejects_bad_name() {
        let bad = SAMPLE.replace("name = \"demo\"", "name = \"Demo App\"");
        assert!(Manifest::parse(&bad).is_err());
    }

    #[test]
    fn origin_resolution() {
        let o = ManifestOrigin::Url("https://x.y/dir/app.toml".into());
        assert_eq!(o.resolve("dep.toml"), "https://x.y/dir/dep.toml");
        assert_eq!(o.resolve("https://z/q.toml"), "https://z/q.toml");
    }
}
