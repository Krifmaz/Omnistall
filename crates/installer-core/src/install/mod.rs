//! The install engine: turns a manifest into an installation, and back.

pub mod common;
#[cfg(unix)]
mod unix_shell;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
pub mod os;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
pub mod os;
#[cfg(windows)]
#[path = "windows.rs"]
pub mod os;

use crate::archive::{self, Format};
use crate::error::{InstallError, IoCtx, Result};
use crate::fetch::Fetcher;
use crate::hooks;
use crate::manifest::{AppKind, Manifest, ManifestOrigin, SourceKind};
use crate::options::InstallOptions;
use crate::paths::{Layout, Scope};
use crate::platform::{Os, Platform};
use crate::progress::{CancelToken, Frontend, FrontendExt, Stage};
use crate::registry::{InstallMethod, InstalledApp, Registry};
use crate::source_build;
use crate::toolchain;
use common::ShortcutRequest;
use std::path::{Path, PathBuf};

/// A manifest together with where it came from and its raw text.
#[derive(Debug, Clone)]
pub struct LoadedManifest {
    pub manifest: Manifest,
    pub origin: ManifestOrigin,
    pub text: String,
}

#[derive(Debug, Clone)]
pub enum InstallStatus {
    NotInstalled,
    Installed {
        app: Box<InstalledApp>,
        /// The manifest's version differs from the installed one.
        update_available: bool,
    },
}

pub struct Engine {
    pub platform: Platform,
    pub layout: Layout,
    pub fetcher: Fetcher,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("platform", &self.platform)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl Engine {
    pub fn new(scope: Scope) -> Result<Self> {
        let platform = Platform::current().map_err(|e| InstallError::Other(anyhow::anyhow!(e)))?;
        if scope == Scope::System && !Platform::is_elevated() {
            return Err(InstallError::NeedsElevation);
        }
        let layout = Layout::detect(scope).map_err(|e| InstallError::Other(anyhow::anyhow!(e)))?;
        Ok(Self {
            platform,
            layout,
            fetcher: Fetcher::new(),
        })
    }

    pub fn registry(&self) -> Result<Registry> {
        Registry::load(&self.layout.registry_file())
    }

    /// Load a manifest from a URL or local path.
    pub fn load_manifest(&self, spec: &str) -> Result<LoadedManifest> {
        let (text, origin) = if spec.starts_with("http://") || spec.starts_with("https://") {
            let t = self.fetcher.get_text(spec).map_err(|e| InstallError::ManifestLoad {
                source_desc: spec.to_string(),
                reason: e.to_string(),
            })?;
            (t, ManifestOrigin::Url(spec.to_string()))
        } else {
            let p = PathBuf::from(common::expand_env(spec));
            let t = std::fs::read_to_string(&p).map_err(|e| InstallError::ManifestLoad {
                source_desc: p.display().to_string(),
                reason: e.to_string(),
            })?;
            let abs = std::fs::canonicalize(&p).unwrap_or(p);
            (t, ManifestOrigin::Path(abs))
        };
        let manifest = Manifest::parse(&text)?;
        Ok(LoadedManifest { manifest, origin, text })
    }

    pub fn parse_manifest(&self, text: &str, origin: ManifestOrigin) -> Result<LoadedManifest> {
        Ok(LoadedManifest {
            manifest: Manifest::parse(text)?,
            origin,
            text: text.to_string(),
        })
    }

    /// Fetch the app icon bytes (PNG/SVG/ICO) for display, if any.
    pub fn load_icon(&self, lm: &LoadedManifest) -> Option<Vec<u8>> {
        let icon = lm.manifest.app.icon.as_ref()?;
        let url = lm.origin.resolve(icon);
        self.fetcher.get_bytes(&url, 4 * 1024 * 1024).ok()
    }

    pub fn status(&self, manifest: &Manifest) -> Result<InstallStatus> {
        let reg = self.registry()?;
        Ok(match reg.get(&manifest.app.name) {
            None => InstallStatus::NotInstalled,
            Some(app) => InstallStatus::Installed {
                update_available: app.version != manifest.app.version,
                app: Box::new(app.clone()),
            },
        })
    }

    /// Default payload directory for this manifest, for display in Options.
    pub fn default_install_dir(&self, manifest: &Manifest) -> PathBuf {
        let needs_bundle = self.platform.os == Os::Macos && manifest.app.kind == AppKind::Gui;
        if needs_bundle {
            self.layout.app_bundles_dir.clone()
        } else if self.platform.os == Os::Windows {
            self.layout
                .apps_root
                .join(crate::fsutil::safe_file_name(manifest.app.display_name()))
        } else {
            self.layout.app_dir(&manifest.app.name)
        }
    }

    pub fn install(
        &self,
        lm: &LoadedManifest,
        options: &InstallOptions,
        frontend: &dyn Frontend,
        cancel: &CancelToken,
    ) -> Result<InstalledApp> {
        let mut stack = Vec::new();
        self.install_inner(lm, options, frontend, cancel, &mut stack)
    }

    /// Re-fetch the manifest an app was installed from (for update checks).
    pub fn refetch_manifest(&self, app: &InstalledApp) -> Result<LoadedManifest> {
        match &app.manifest_origin {
            ManifestOrigin::Url(u) => self.load_manifest(u),
            ManifestOrigin::Path(p) => self.load_manifest(&p.display().to_string()),
            ManifestOrigin::Embedded => self.parse_manifest(&app.manifest_toml, ManifestOrigin::Embedded),
        }
    }

    /// Reinstall the same version with the same options.
    pub fn repair(&self, name: &str, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<InstalledApp> {
        let reg = self.registry()?;
        let app = reg
            .get(name)
            .ok_or_else(|| InstallError::NotInstalled(name.to_string()))?
            .clone();
        let lm = self.parse_manifest(&app.manifest_toml, app.manifest_origin.clone())?;
        let mut opts = app.options.clone();
        opts.force = true;
        self.install(&lm, &opts, frontend, cancel)
    }

    pub fn launch(&self, app: &InstalledApp) -> Result<()> {
        let is_cli = Manifest::parse(&app.manifest_toml)
            .map(|m| m.app.kind == AppKind::Cli)
            .unwrap_or(false);
        os::launch(app, is_cli)
    }

    fn install_inner(
        &self,
        lm: &LoadedManifest,
        options: &InstallOptions,
        frontend: &dyn Frontend,
        cancel: &CancelToken,
        stack: &mut Vec<String>,
    ) -> Result<InstalledApp> {
        let manifest = &lm.manifest;
        let name = manifest.app.name.clone();
        let display = manifest.app.display_name().to_string();
        if stack.contains(&name) {
            return Err(InstallError::DependencyCycle(name));
        }
        stack.push(name.clone());
        let result = self.install_guarded(lm, options, frontend, cancel, stack);
        stack.pop();
        match &result {
            Ok(_) => frontend.log(format!("{display} installed")),
            Err(e) => frontend.log(format!("{display} failed: {e}")),
        }
        result
    }

    fn install_guarded(
        &self,
        lm: &LoadedManifest,
        options: &InstallOptions,
        frontend: &dyn Frontend,
        cancel: &CancelToken,
        stack: &mut Vec<String>,
    ) -> Result<InstalledApp> {
        let manifest = &lm.manifest;
        let platform = &self.platform;
        frontend.stage(Stage::Preparing, manifest.app.display_name());

        let mut registry = self.registry()?;
        if let Some(existing) = registry.get(&manifest.app.name) {
            if existing.version == manifest.app.version && !options.force {
                frontend.log(format!(
                    "{} {} is already installed",
                    existing.display_name, existing.version
                ));
                return Ok(existing.clone());
            }
        }

        // Work directory: lives in our data dir (same filesystem as the
        // install target where possible, so the final move is a rename).
        let tmp_root = self.layout.tmp_dir();
        std::fs::create_dir_all(&tmp_root).at(&tmp_root)?;
        let work = tempfile::Builder::new()
            .prefix(&format!("{}-", manifest.app.name))
            .tempdir_in(&tmp_root)
            .at(&tmp_root)?;
        let work_dir = work.path().to_path_buf();

        // 1. Dependencies.
        if !manifest.dependencies.is_empty() {
            frontend.stage(
                Stage::ResolvingDependencies,
                format!("{} dependencies", manifest.dependencies.len()),
            );
        }
        let mut dep_names = Vec::new();
        for dep in &manifest.dependencies {
            cancel.check()?;
            let spec = lm.origin.resolve(&dep.manifest);
            let dep_lm = self.load_manifest(&spec)?;
            if dep_lm.manifest.app.name != dep.name {
                frontend.warn(format!(
                    "dependency {} resolved to a manifest named {}",
                    dep.name, dep_lm.manifest.app.name
                ));
            }
            let satisfied = registry.get(&dep_lm.manifest.app.name).is_some_and(|inst| {
                match (
                    &dep.version,
                    semver::Version::parse(inst.version.trim_start_matches('v')),
                ) {
                    (Some(req), Ok(v)) => semver::VersionReq::parse(req).map(|r| r.matches(&v)).unwrap_or(false),
                    _ => true,
                }
            });
            if !satisfied {
                let dep_opts = InstallOptions {
                    install_dir: None,
                    desktop_shortcut: false,
                    menu_shortcut: dep_lm.manifest.app.kind == AppKind::Gui,
                    add_to_path: None,
                    from_source: options.from_source,
                    scope: options.scope,
                    force: false,
                };
                self.install_inner(&dep_lm, &dep_opts, frontend, cancel, stack)?;
                registry = self.registry()?;
            }
            dep_names.push(dep_lm.manifest.app.name.clone());
        }
        frontend.stage(Stage::Preparing, manifest.app.display_name());

        // 2. Pick a source.
        let selected = manifest.select_source(platform, options.from_source);
        let build_only = selected.is_none() && manifest.build.is_some();
        if selected.is_none() && !build_only {
            return Err(InstallError::NoMatchingSource {
                platform: platform.to_string(),
            });
        }
        if let Some(s) = &selected {
            frontend.log(format!("using source `{}`", s.key));
            if s.emulated {
                frontend.warn(format!(
                    "no native build for {}; installing the x86_64 version, which runs under emulation",
                    platform
                ));
            }
        }

        let spec = manifest.install.for_os(platform.os);
        let hooks = manifest.hooks.for_os(platform.os);
        let is_gui = manifest.app.kind == AppKind::Gui;

        // Where the final payload lives.
        let install_dir = options
            .install_dir
            .clone()
            .map(|d| PathBuf::from(common::expand_env(&d.display().to_string())))
            .unwrap_or_else(|| {
                if platform.os == Os::Windows {
                    self.layout
                        .apps_root
                        .join(crate::fsutil::safe_file_name(manifest.app.display_name()))
                } else {
                    self.layout.app_dir(&manifest.app.name)
                }
            });
        let staging = work_dir.join("payload");

        // 3. Pre-install hooks.
        hooks::run(
            "pre-install",
            &hooks.pre_install,
            manifest,
            platform,
            &work_dir,
            &install_dir,
            frontend,
            cancel,
        )?;

        // 4. Acquire the payload.
        let mut method = InstallMethod::Payload;
        let mut executable: Option<PathBuf> = None;
        let mut app_bundle: Option<PathBuf> = None;
        let mut deb_package = None;
        let mut msi_cache = None;
        let mut native_uninstall = spec.uninstall_command.clone();
        let mut payload_placed = false; // true when `staging` holds files for install_dir

        let mut must_build = build_only;
        let mut src_dir: Option<PathBuf> = None;

        if let Some(sel) = &selected {
            match &sel.source.kind {
                SourceKind::Git { url, rev, submodules } => {
                    frontend.stage(Stage::Downloading, "source code");
                    let dest = work_dir.join("src");
                    let rev = rev.as_ref().map(|r| manifest.expand(r, platform));
                    crate::fetch::git_clone(
                        &manifest.expand(url, platform),
                        rev.as_deref(),
                        *submodules,
                        &dest,
                        frontend,
                        cancel,
                    )?;
                    src_dir = Some(dest);
                    must_build = true;
                }
                SourceKind::Url { .. } | SourceKind::Github { .. } => {
                    let (url, file_name, expected_sha) = self.resolve_download(manifest, sel.source, frontend)?;
                    frontend.stage(Stage::Downloading, &file_name);
                    let file = work_dir.join("download").join(&file_name);
                    self.fetcher.download(&url, &file, frontend, cancel)?;

                    frontend.stage(Stage::Verifying, &file_name);
                    match expected_sha {
                        Some(sum) => {
                            crate::checksum::verify(&file, &sum)?;
                            frontend.log("checksum OK");
                        }
                        None => frontend.warn("no checksum provided by the manifest; download integrity not verified"),
                    }

                    let format = archive::detect(&file, sel.source.format.as_deref(), &file_name)?;
                    frontend.log(format!("payload format: {format:?}"));
                    let strip = sel.source.strip_components.unwrap_or(0);

                    if sel.source.build {
                        let dest = work_dir.join("src");
                        frontend.stage(Stage::Extracting, &file_name);
                        archive::extract(&file, format, &dest, strip, frontend, cancel)?;
                        src_dir = Some(dest);
                        must_build = true;
                    } else if format.is_archive() {
                        let native = self.try_native(format, &file, &staging, manifest, &spec, frontend, cancel)?;
                        if let Some(n) = native {
                            method = n.method;
                            executable = n.executable;
                            app_bundle = n.app_bundle;
                            deb_package = n.deb_package;
                            msi_cache = n.msi_cache;
                            native_uninstall = n.native_uninstall.or(native_uninstall);
                        } else {
                            frontend.stage(Stage::Extracting, &file_name);
                            archive::extract(&file, format, &staging, strip, frontend, cancel)?;
                            payload_placed = true;
                        }
                    } else if format == Format::Raw {
                        std::fs::create_dir_all(&staging).at(&staging)?;
                        let bin_name =
                            if platform.os == Os::Windows && !file_name.to_ascii_lowercase().ends_with(".exe") {
                                format!("{}.exe", manifest.app.name)
                            } else if platform.os == Os::Windows {
                                file_name.clone()
                            } else {
                                manifest.app.name.clone()
                            };
                        let dest = staging.join(&bin_name);
                        std::fs::copy(&file, &dest).at(&dest)?;
                        archive::make_executable(&dest)?;
                        payload_placed = true;
                    } else {
                        frontend.stage(Stage::Configuring, "running installer");
                        let native = self
                            .try_native(format, &file, &staging, manifest, &spec, frontend, cancel)?
                            .ok_or_else(|| InstallError::UnsupportedFormat(format!("{format:?}")))?;
                        method = native.method;
                        executable = native.executable;
                        app_bundle = native.app_bundle;
                        deb_package = native.deb_package;
                        msi_cache = native.msi_cache;
                        native_uninstall = native.native_uninstall.or(native_uninstall);
                        if method == InstallMethod::AppImage || app_bundle.is_some() {
                            payload_placed = staging.exists();
                        }
                    }
                }
            }
        }

        // 5. Build from source if needed.
        if must_build {
            let build = manifest
                .build
                .as_ref()
                .ok_or_else(|| InstallError::Manifest("source requires building but [build] is missing".into()))?
                .for_os(platform.os);
            let src = match src_dir {
                Some(d) => d,
                None => {
                    let d = work_dir.join("src");
                    std::fs::create_dir_all(&d).at(&d)?;
                    d
                }
            };
            frontend.stage(Stage::ResolvingDependencies, "build tools");
            let extra = toolchain::ensure(
                &build.toolchains,
                &manifest.toolchains,
                platform,
                &self.fetcher,
                &work_dir,
                frontend,
                cancel,
            )?;
            let outputs =
                source_build::build(manifest, &build, &src, &install_dir, platform, &extra, frontend, cancel)?;
            std::fs::create_dir_all(&staging).at(&staging)?;
            source_build::place_outputs(&outputs, &staging)?;
            payload_placed = true;
            method = InstallMethod::Source;
        }

        cancel.check()?;
        frontend.stage(Stage::Configuring, manifest.app.display_name());

        // 6. macOS: a .app inside the payload goes to ~/Applications.
        if platform.os == Os::Macos && payload_placed && app_bundle.is_none() {
            let found = spec
                .app_bundle
                .as_ref()
                .map(|b| staging.join(b))
                .filter(|p| p.is_dir())
                .or_else(|| archive::find_app_bundle(&staging));
            if let Some(b) = found {
                app_bundle = Some(b);
            }
        }

        // 7. Remove the previous installation's integration (update/repair).
        if let Some(old) = registry.get(&manifest.app.name).cloned() {
            frontend.log(format!("replacing {} {}", old.display_name, old.version));
            self.remove_integration(&old, &registry, frontend)?;
            if old.method == InstallMethod::AppBundle {
                if let Some(b) = &old.app_bundle {
                    crate::fsutil::remove_path(b)?;
                }
            }
        }

        // 8. Place the payload.
        let mut final_install_dir: Option<PathBuf> = None;
        #[allow(unused_mut)]
        let mut final_bundle: Option<PathBuf> = None;
        if let Some(bundle) = app_bundle.take() {
            #[cfg(target_os = "macos")]
            {
                let dest = os::install_app_bundle(&self.layout, &bundle, frontend, cancel)?;
                method = InstallMethod::AppBundle;
                final_bundle = Some(dest.clone());
                executable = Some(dest);
                payload_placed = false;
                // Any remaining CLI helpers in the payload are still installed
                // if the manifest lists `bin` entries.
                if !spec.bin.is_empty() {
                    crate::fsutil::remove_path(&bundle)?;
                    payload_placed = true;
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = bundle;
            }
        }
        if payload_placed {
            crate::fsutil::replace_dir(&staging, &install_dir)?;
            final_install_dir = Some(install_dir.clone());
            for f in &spec.files {
                let src = install_dir.join(manifest.expand(&f.src, platform));
                let dst = install_dir.join(manifest.expand(&f.dst, platform));
                if let Some(p) = dst.parent() {
                    std::fs::create_dir_all(p).at(p)?;
                }
                std::fs::rename(&src, &dst).at(&dst)?;
                if f.executable {
                    archive::make_executable(&dst)?;
                }
            }
        }

        // 9. Executables.
        let mut bins: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &final_install_dir {
            let mut names = spec
                .bin
                .iter()
                .map(|b| manifest.expand(b, platform))
                .collect::<Vec<_>>();
            if names.is_empty() {
                names = default_bin_candidates(&manifest.app.name, platform.os);
            }
            for n in names {
                let p = dir.join(&n);
                if p.is_file() {
                    archive::make_executable(&p)?;
                    bins.push(p);
                } else if !spec.bin.is_empty() {
                    frontend.warn(format!("bin entry {n:?} not found in payload"));
                }
            }
            if executable.is_none() {
                executable = spec
                    .executable
                    .as_ref()
                    .map(|e| dir.join(manifest.expand(e, platform)))
                    .filter(|p| p.exists())
                    .or_else(|| bins.first().cloned());
            }
            if method == InstallMethod::Payload && executable.is_none() && is_gui {
                frontend.warn("no executable found in the payload; set install.executable in the manifest");
            }
        }
        if let Some(e) = &executable {
            if method != InstallMethod::AppBundle && !bins.contains(e) && e.is_file() && final_install_dir.is_some() {
                bins.push(e.clone());
            }
        }

        // 10. PATH / symlinks.
        let mut artifacts: Vec<PathBuf> = Vec::new();
        let mut path_entries: Vec<PathBuf> = Vec::new();
        let add_to_path = options.add_to_path.or(spec.add_to_path).unwrap_or(!is_gui);
        if !bins.is_empty() && (add_to_path || !is_gui) {
            for b in &bins {
                let name = b.file_name().unwrap_or_default().to_string_lossy().to_string();
                let exposed = os::expose_binary(&self.layout, b, &name)?;
                if platform.os == Os::Windows {
                    if add_to_path && !path_entries.contains(&exposed) {
                        path_entries.push(exposed);
                    }
                } else {
                    artifacts.push(exposed);
                }
            }
            if add_to_path && platform.os != Os::Windows {
                path_entries.push(self.layout.bin_dir.clone());
            }
            for dir in &path_entries {
                match os::add_to_path(&self.layout, dir) {
                    Ok(true) => frontend.log(format!(
                        "added {} to PATH (takes effect in new terminals)",
                        dir.display()
                    )),
                    Ok(false) => {}
                    Err(e) => frontend.warn(format!("could not update PATH: {e}")),
                }
            }
        }

        // 11. Icon.
        let icon_path = self.resolve_icon(lm, final_install_dir.as_deref(), &spec, frontend);

        // 12. Shortcuts (GUI apps; also when the manifest lists any).
        let shortcut_exec = final_bundle.clone().or_else(|| executable.clone());
        if let Some(exec) = &shortcut_exec {
            let mime_types: Vec<String> = Vec::new();
            let mut requests: Vec<(
                String,
                PathBuf,
                Option<String>,
                Option<PathBuf>,
                Option<String>,
                Vec<String>,
                bool,
            )> = Vec::new();
            if manifest.shortcuts.is_empty() {
                if is_gui {
                    requests.push((
                        manifest.app.display_name().to_string(),
                        exec.clone(),
                        None,
                        icon_path.clone(),
                        manifest.app.description.clone(),
                        vec![],
                        false,
                    ));
                }
            } else {
                for s in &manifest.shortcuts {
                    let exe = s
                        .exec
                        .as_ref()
                        .and_then(|e| final_install_dir.as_ref().map(|d| d.join(manifest.expand(e, platform))))
                        .unwrap_or_else(|| exec.clone());
                    let icon = s
                        .icon
                        .as_ref()
                        .and_then(|i| final_install_dir.as_ref().map(|d| d.join(i)))
                        .filter(|p| p.exists())
                        .or_else(|| icon_path.clone());
                    requests.push((
                        s.name
                            .clone()
                            .unwrap_or_else(|| manifest.app.display_name().to_string()),
                        exe,
                        s.args.clone(),
                        icon,
                        s.comment.clone().or_else(|| manifest.app.description.clone()),
                        s.categories.clone(),
                        s.terminal,
                    ));
                }
            }
            for (sname, exe, args, icon, comment, categories, terminal) in &requests {
                let req = ShortcutRequest {
                    app_id: &manifest.app.name,
                    name: sname,
                    exec: exe,
                    args: args.as_deref(),
                    icon: icon.as_deref(),
                    comment: comment.as_deref(),
                    categories,
                    terminal: *terminal,
                    mime_types: &mime_types,
                    working_dir: final_install_dir.as_deref(),
                };
                if options.menu_shortcut {
                    if let Err(e) = os::create_menu_shortcut(&self.layout, &req, &mut artifacts) {
                        frontend.warn(format!("menu shortcut failed: {e}"));
                    }
                }
                if options.desktop_shortcut {
                    if let Err(e) = os::create_desktop_shortcut(&self.layout, &req, &mut artifacts) {
                        frontend.warn(format!("desktop shortcut failed: {e}"));
                    }
                }
            }
        }

        // 13. File associations.
        let (registry_keys, mime_types) = self.register_associations(manifest, executable.as_deref(), frontend)?;
        if !mime_types.is_empty() {
            // Re-write the menu entry so it advertises the MIME types.
            if let (Some(exec), true) = (&shortcut_exec, options.menu_shortcut) {
                let req = ShortcutRequest {
                    app_id: &manifest.app.name,
                    name: manifest.app.display_name(),
                    exec,
                    args: None,
                    icon: icon_path.as_deref(),
                    comment: manifest.app.description.as_deref(),
                    categories: &[],
                    terminal: false,
                    mime_types: &mime_types,
                    working_dir: final_install_dir.as_deref(),
                };
                let _ = os::create_menu_shortcut(&self.layout, &req, &mut artifacts);
            }
        }
        artifacts.sort();
        artifacts.dedup();

        // 14. Post-install hooks.
        frontend.stage(Stage::Finishing, manifest.app.display_name());
        let hook_cwd = final_install_dir.clone().unwrap_or_else(|| work_dir.clone());
        hooks::run(
            "post-install",
            &hooks.post_install,
            manifest,
            platform,
            &hook_cwd,
            &install_dir,
            frontend,
            cancel,
        )?;

        // 15. Record.
        let app = InstalledApp {
            name: manifest.app.name.clone(),
            display_name: manifest.app.display_name().to_string(),
            version: manifest.app.version.clone(),
            platform_key: selected
                .as_ref()
                .map(|s| s.key.clone())
                .unwrap_or_else(|| platform.key()),
            scope: self.layout.scope,
            method,
            install_dir: final_install_dir,
            executable,
            app_bundle: final_bundle,
            artifacts,
            path_entries,
            registry_keys,
            mime_types,
            native_uninstall,
            deb_package,
            msi_cache,
            manifest_origin: lm.origin.clone(),
            manifest_toml: lm.text.clone(),
            dependencies: dep_names,
            installed_at: chrono::Utc::now(),
            options: options.clone(),
        };
        let mut registry = self.registry()?;
        registry.insert(app.clone());
        registry.save()?;
        frontend.stage(Stage::Done, manifest.app.display_name());
        Ok(app)
    }

    /// Resolve the download URL, file name and expected checksum.
    fn resolve_download(
        &self,
        manifest: &Manifest,
        source: &crate::manifest::Source,
        frontend: &dyn Frontend,
    ) -> Result<(String, String, Option<String>)> {
        let platform = &self.platform;
        match &source.kind {
            SourceKind::Url { url } => {
                let url = manifest.expand(url, platform);
                let name = file_name_from_url(&url).unwrap_or_else(|| manifest.app.name.clone());
                Ok((url, name, source.sha256.clone()))
            }
            SourceKind::Github {
                repo,
                tag,
                asset,
                checksum_asset,
            } => {
                let tag = tag
                    .as_ref()
                    .map(|t| manifest.expand(t, platform))
                    .unwrap_or_else(|| format!("v{}", manifest.app.version));
                let pattern = manifest.expand(asset, platform);
                frontend.log(format!("looking up {repo} release {tag} asset {pattern}"));
                let assets = self.fetcher.github_release_assets(repo, &tag)?;
                let matched: Vec<_> = assets
                    .iter()
                    .filter(|a| crate::fetch::wildcard_match(&pattern, &a.name))
                    .collect();
                let asset = match matched.as_slice() {
                    [one] => (*one).clone(),
                    [] => {
                        return Err(InstallError::Download {
                            url: format!("github:{repo}@{tag}"),
                            reason: format!(
                                "no release asset matches {pattern:?} (available: {})",
                                assets.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                            ),
                        });
                    }
                    many => {
                        return Err(InstallError::Download {
                            url: format!("github:{repo}@{tag}"),
                            reason: format!(
                                "asset pattern {pattern:?} matches several files: {}",
                                many.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                            ),
                        });
                    }
                };
                let mut sha = source.sha256.clone();
                if sha.is_none() {
                    if let Some(cs) = checksum_asset {
                        let cs_pattern = manifest.expand(cs, platform);
                        if let Some(cs_asset) = assets
                            .iter()
                            .find(|a| crate::fetch::wildcard_match(&cs_pattern, &a.name))
                        {
                            let text = self.fetcher.get_text(&cs_asset.browser_download_url)?;
                            sha = crate::checksum::find_in_sums(&text, &asset.name);
                            if sha.is_none() {
                                frontend.warn(format!("{} does not list {}", cs_asset.name, asset.name));
                            }
                        } else {
                            frontend.warn(format!("checksum asset {cs_pattern:?} not found in release"));
                        }
                    }
                }
                Ok((asset.browser_download_url.clone(), asset.name.clone(), sha))
            }
            SourceKind::Git { .. } => unreachable!("git sources are cloned, not downloaded"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_native(
        &self,
        format: Format,
        file: &Path,
        staging: &Path,
        manifest: &Manifest,
        spec: &crate::manifest::InstallSpec,
        frontend: &dyn Frontend,
        cancel: &CancelToken,
    ) -> Result<Option<common::NativeOutcome>> {
        #[cfg(windows)]
        {
            os::install_native(
                format,
                file,
                staging,
                manifest.app.display_name(),
                spec,
                &self.layout.cache_dir(),
                frontend,
                cancel,
            )
        }
        #[cfg(not(windows))]
        {
            os::install_native(
                format,
                file,
                staging,
                manifest.app.display_name(),
                spec,
                frontend,
                cancel,
            )
        }
    }

    fn register_associations(
        &self,
        manifest: &Manifest,
        executable: Option<&Path>,
        frontend: &dyn Frontend,
    ) -> Result<(Vec<String>, Vec<String>)> {
        if manifest.file_associations.is_empty() {
            return Ok((vec![], vec![]));
        }
        #[cfg(windows)]
        {
            let keys = os::register_file_associations(
                &self.layout,
                &manifest.app.name,
                &manifest.file_associations,
                executable,
                frontend,
            )?;
            Ok((keys, vec![]))
        }
        #[cfg(not(windows))]
        {
            let _ = executable;
            let (mimes, _files) = os::register_file_associations(
                &self.layout,
                &manifest.app.name,
                &manifest.file_associations,
                frontend,
            )?;
            Ok((vec![], mimes))
        }
    }

    /// Put the app icon somewhere permanent and return its path.
    fn resolve_icon(
        &self,
        lm: &LoadedManifest,
        install_dir: Option<&Path>,
        spec: &crate::manifest::InstallSpec,
        frontend: &dyn Frontend,
    ) -> Option<PathBuf> {
        let manifest = &lm.manifest;
        let icon_ref = manifest.app.icon.as_ref()?;
        // Inside the payload?
        if let Some(dir) = install_dir {
            let p = dir.join(icon_ref);
            if p.is_file() {
                return Some(p);
            }
        }
        let _ = spec;
        let url = lm.origin.resolve(icon_ref);
        let ext = Path::new(icon_ref.split('?').next().unwrap_or(icon_ref))
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "png".into());
        let dest_dir = install_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.layout.icons_dir.clone());
        let dest = dest_dir.join(format!("{}-icon.{ext}", manifest.app.name));
        match self.fetcher.get_bytes(&url, 8 * 1024 * 1024) {
            Ok(bytes) => {
                if std::fs::create_dir_all(&dest_dir).is_ok() && std::fs::write(&dest, bytes).is_ok() {
                    return Some(dest);
                }
                None
            }
            Err(e) => {
                frontend.warn(format!("icon unavailable: {e}"));
                None
            }
        }
    }

    /// Remove shortcuts, PATH entries, symlinks and associations.
    /// PATH entries shared with other installed apps are kept.
    fn remove_integration(&self, app: &InstalledApp, registry: &Registry, frontend: &dyn Frontend) -> Result<()> {
        for a in &app.artifacts {
            if let Err(e) = crate::fsutil::remove_path(a) {
                frontend.warn(format!("could not remove {}: {e}", a.display()));
            }
        }
        for p in &app.path_entries {
            let shared = registry
                .list()
                .any(|other| other.name != app.name && other.path_entries.contains(p));
            if shared {
                frontend.log(format!("keeping PATH entry {} (used by other apps)", p.display()));
                continue;
            }
            if let Err(e) = os::remove_from_path(&self.layout, p) {
                frontend.warn(format!("could not clean PATH entry {}: {e}", p.display()));
            }
        }
        #[cfg(windows)]
        os::unregister_file_associations(&app.registry_keys)?;
        #[cfg(not(windows))]
        os::unregister_file_associations(&self.layout, &app.mime_types)?;
        Ok(())
    }

    pub fn uninstall(&self, name: &str, force: bool, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
        let mut registry = self.registry()?;
        let app = registry
            .get(name)
            .ok_or_else(|| InstallError::NotInstalled(name.to_string()))?
            .clone();
        let dependents = registry.dependents_of(name);
        if !dependents.is_empty() && !force {
            return Err(InstallError::Other(anyhow::anyhow!(
                "{} is still needed by: {}",
                app.display_name,
                dependents.join(", ")
            )));
        }
        frontend.stage(Stage::Removing, &app.display_name);
        let manifest = Manifest::parse(&app.manifest_toml).ok();
        let cwd = app
            .install_dir
            .clone()
            .filter(|d| d.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        if let Some(m) = &manifest {
            let h = m.hooks.for_os(self.platform.os);
            hooks::run(
                "pre-uninstall",
                &h.pre_uninstall,
                m,
                &self.platform,
                &cwd,
                &cwd,
                frontend,
                cancel,
            )?;
        }
        self.remove_integration(&app, &registry, frontend)?;
        os::uninstall_native(&app, frontend, cancel)?;
        match app.method {
            InstallMethod::Payload | InstallMethod::Source | InstallMethod::AppImage => {
                if let Some(dir) = &app.install_dir {
                    crate::fsutil::remove_path(dir)?;
                }
            }
            InstallMethod::AppBundle => {
                if let Some(b) = &app.app_bundle {
                    crate::fsutil::remove_path(b)?;
                }
                if let Some(dir) = &app.install_dir {
                    crate::fsutil::remove_path(dir)?;
                }
            }
            _ => {}
        }
        if let Some(m) = &manifest {
            let h = m.hooks.for_os(self.platform.os);
            let cwd = std::env::temp_dir();
            hooks::run(
                "post-uninstall",
                &h.post_uninstall,
                m,
                &self.platform,
                &cwd,
                &cwd,
                frontend,
                cancel,
            )?;
        }
        registry.remove(name);
        registry.save()?;
        frontend.stage(Stage::Done, &app.display_name);
        Ok(())
    }
}

fn default_bin_candidates(name: &str, os: Os) -> Vec<String> {
    let mut v = vec![
        name.to_string(),
        format!("bin/{name}"),
        format!("usr/bin/{name}"),
        format!("usr/local/bin/{name}"),
    ];
    if os == Os::Windows {
        v = v.iter().map(|s| format!("{s}.exe")).chain(v.iter().cloned()).collect();
    }
    v
}

fn file_name_from_url(url: &str) -> Option<String> {
    let no_query = url.split(['?', '#']).next()?;
    let last = no_query.rsplit('/').next()?;
    if last.is_empty() { None } else { Some(last.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_file_names() {
        assert_eq!(
            file_name_from_url("https://x/y/app-1.0.tar.gz?token=1").as_deref(),
            Some("app-1.0.tar.gz")
        );
        assert_eq!(file_name_from_url("https://x/y/"), None);
    }
}
