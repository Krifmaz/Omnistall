//! macOS integration: `.app` bundles into ~/Applications, DMG mounting,
//! `.pkg` via `installer`, Desktop aliases, PATH via shell profiles.

use super::common::ShortcutRequest;
use crate::archive::Format;
use crate::error::{InstallError, IoCtx, Result};
use crate::paths::Layout;
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt, Question};
use crate::registry::{InstallMethod, InstalledApp};
use std::path::{Path, PathBuf};

pub use super::common::NativeOutcome;
pub use super::unix_shell::{add_to_path, expose_binary, remove_from_path};

/// Copy a bundle with `ditto` (preserves signatures, xattrs, symlinks).
pub fn install_app_bundle(
    layout: &Layout,
    bundle: &Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<PathBuf> {
    std::fs::create_dir_all(&layout.app_bundles_dir).at(&layout.app_bundles_dir)?;
    let name = bundle
        .file_name()
        .ok_or_else(|| InstallError::Extract("bundle has no name".into()))?;
    let dest = layout.app_bundles_dir.join(name);
    if dest.exists() {
        crate::fsutil::remove_path(&dest)?;
    }
    if which::which("ditto").is_ok() {
        let mut cmd = process::command("ditto");
        cmd.arg(bundle).arg(&dest);
        let st = process::run_logged(&mut cmd, frontend, cancel)?;
        if !st.success() {
            return Err(InstallError::Extract(format!("ditto failed with {:?}", st.code())));
        }
    } else {
        crate::fsutil::copy_dir(bundle, &dest)?;
    }
    // Register with Launch Services so Spotlight/Launchpad see it immediately.
    let _ = process::command(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
    )
    .args(["-f", &dest.display().to_string()])
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status();
    Ok(dest)
}

/// Launchpad shows everything in ~/Applications; nothing to create for
/// bundles. For non-bundle executables we create a tiny launcher bundle.
pub fn create_menu_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    if req.exec.extension().is_some_and(|e| e == "app") {
        return Ok(());
    }
    let bundle = layout
        .app_bundles_dir
        .join(format!("{}.app", crate::fsutil::safe_file_name(req.name)));
    let macos_dir = bundle.join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos_dir).at(&macos_dir)?;
    let script = macos_dir.join("launcher");
    let args = req.args.unwrap_or("");
    let body = if req.terminal {
        format!("#!/bin/sh\nopen -a Terminal \"{}\"\n", req.exec.display())
    } else {
        format!("#!/bin/sh\nexec \"{}\" {args} \"$@\"\n", req.exec.display())
    };
    std::fs::write(&script, body).at(&script)?;
    crate::archive::make_executable(&script)?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>CFBundleName</key><string>{}</string>\n<key>CFBundleIdentifier</key><string>dev.omnistall.launcher.{}</string>\n<key>CFBundleExecutable</key><string>launcher</string>\n<key>CFBundlePackageType</key><string>APPL</string>\n<key>LSUIElement</key><{}/>\n</dict></plist>\n",
        req.name,
        req.app_id,
        if req.terminal { "true" } else { "false" }
    );
    let plist_path = bundle.join("Contents").join("Info.plist");
    std::fs::write(&plist_path, plist).at(&plist_path)?;
    if let Some(icon) = req.icon {
        let res = bundle.join("Contents").join("Resources");
        std::fs::create_dir_all(&res).at(&res)?;
        let _ = std::fs::copy(icon, res.join(icon.file_name().unwrap_or_default()));
    }
    artifacts.push(bundle);
    Ok(())
}

pub fn create_desktop_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    let Some(desktop) = &layout.desktop_dir else {
        return Ok(());
    };
    let target = if req.exec.extension().is_some_and(|e| e == "app") {
        req.exec.to_path_buf()
    } else {
        layout
            .app_bundles_dir
            .join(format!("{}.app", crate::fsutil::safe_file_name(req.name)))
    };
    if !target.exists() {
        return Ok(());
    }
    let link = desktop.join(target.file_name().unwrap_or_default());
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link).at(&link)?;
    artifacts.push(link);
    Ok(())
}

pub fn register_file_associations(
    _layout: &Layout,
    _app_id: &str,
    assocs: &[crate::manifest::FileAssociation],
    frontend: &dyn Frontend,
) -> Result<(Vec<String>, Vec<PathBuf>)> {
    if !assocs.is_empty() {
        frontend.log("macOS file associations come from the app bundle's Info.plist (CFBundleDocumentTypes); nothing extra to register");
    }
    Ok((vec![], vec![]))
}

pub fn unregister_file_associations(_layout: &Layout, _mimes: &[String]) -> Result<()> {
    Ok(())
}

pub fn install_native(
    format: Format,
    file: &Path,
    staging: &Path,
    _display_name: &str,
    spec: &crate::manifest::InstallSpec,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<Option<NativeOutcome>> {
    match format {
        Format::Dmg => {
            let mount = staging.with_extension("mount");
            std::fs::create_dir_all(&mount).at(&mount)?;
            let mut cmd = process::command("hdiutil");
            cmd.args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-noverify",
                "-mountpoint",
            ])
            .arg(&mount)
            .arg(file);
            // Some DMGs show a licence; piping "Y" accepts it the same way a
            // user would in the Finder dialog.
            cmd.stdin(std::process::Stdio::piped());
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                return Err(InstallError::NativeInstaller {
                    command: "hdiutil attach".into(),
                    code: st.code(),
                });
            }
            let result = (|| -> Result<Option<NativeOutcome>> {
                if let Some(bundle) = spec
                    .app_bundle
                    .as_ref()
                    .map(|b| mount.join(b))
                    .filter(|p| p.exists())
                    .or_else(|| crate::archive::find_app_bundle(&mount))
                {
                    std::fs::create_dir_all(staging).at(staging)?;
                    let dest = staging.join(bundle.file_name().unwrap_or_default());
                    let mut cmd = process::command("ditto");
                    cmd.arg(&bundle).arg(&dest);
                    let st = process::run_logged(&mut cmd, frontend, cancel)?;
                    if !st.success() {
                        return Err(InstallError::Extract("ditto from dmg failed".into()));
                    }
                    return Ok(Some(NativeOutcome {
                        method: InstallMethod::AppBundle,
                        executable: None,
                        app_bundle: Some(dest),
                        deb_package: None,
                        msi_cache: None,
                        native_uninstall: None,
                    }));
                }
                if let Some(pkg) = walkdir::WalkDir::new(&mount)
                    .max_depth(2)
                    .into_iter()
                    .flatten()
                    .map(|e| e.into_path())
                    .find(|p| p.extension().is_some_and(|e| e == "pkg"))
                {
                    return run_pkg(&pkg, spec, frontend, cancel).map(Some);
                }
                Err(InstallError::UnsupportedFormat(
                    "the disk image contains no .app or .pkg".into(),
                ))
            })();
            let _ = process::command("hdiutil")
                .args(["detach", "-force"])
                .arg(&mount)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            result
        }
        Format::Pkg => run_pkg(file, spec, frontend, cancel).map(Some),
        Format::Msi | Format::Exe => Err(InstallError::UnsupportedFormat(format!(
            "{format:?} installers cannot be used on macOS"
        ))),
        _ => Ok(None),
    }
}

fn run_pkg(
    pkg: &Path,
    spec: &crate::manifest::InstallSpec,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<NativeOutcome> {
    // Per-user target first (no admin). Many packages only support "/".
    let mut cmd = process::command("installer");
    cmd.args(["-pkg"])
        .arg(pkg)
        .args(["-target", "CurrentUserHomeDirectory"]);
    let st = process::run_logged(&mut cmd, frontend, cancel)?;
    if !st.success() {
        let q = Question {
            title: "Administrator permission needed".into(),
            body: "This app's installer package can only be installed system-wide. macOS will ask for your password."
                .into(),
            accept_label: "Continue".into(),
            decline_label: "Cancel".into(),
        };
        if !frontend.confirm(&q) {
            return Err(InstallError::Declined("system-wide pkg install".into()));
        }
        let script = format!(
            "do shell script \"installer -pkg {} -target /\" with administrator privileges",
            shell_escape_applescript(&pkg.display().to_string())
        );
        let mut cmd = process::command("osascript");
        cmd.args(["-e", &script]);
        let st = process::run_logged(&mut cmd, frontend, cancel)?;
        if !st.success() {
            return Err(InstallError::NativeInstaller {
                command: "installer -pkg".into(),
                code: st.code(),
            });
        }
    }
    let exe = spec
        .executable
        .as_ref()
        .map(|e| PathBuf::from(super::common::expand_env(e)));
    Ok(NativeOutcome {
        method: InstallMethod::Pkg,
        executable: exe,
        app_bundle: None,
        deb_package: None,
        msi_cache: None,
        native_uninstall: spec.uninstall_command.clone(),
    })
}

fn shell_escape_applescript(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''")).replace('"', "\\\"")
}

pub fn uninstall_native(app: &InstalledApp, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
    if let (InstallMethod::Pkg, Some(cmd)) = (app.method, &app.native_uninstall) {
        let st = process::run_logged(&mut process::shell(cmd), frontend, cancel)?;
        if !st.success() {
            return Err(InstallError::NativeInstaller {
                command: cmd.clone(),
                code: st.code(),
            });
        }
    } else if app.method == InstallMethod::Pkg {
        frontend.warn("this app was installed with a .pkg that provides no uninstaller; its files were left in place");
    }
    Ok(())
}

pub fn launch(app: &InstalledApp, is_cli: bool) -> Result<()> {
    if let Some(bundle) = &app.app_bundle {
        process::command("open").arg(bundle).spawn().at(bundle)?;
        return Ok(());
    }
    let Some(exe) = &app.executable else {
        return Err(InstallError::Other(anyhow::anyhow!(
            "no executable recorded for {}",
            app.name
        )));
    };
    if is_cli {
        process::command("open")
            .args(["-a", "Terminal"])
            .arg(exe)
            .spawn()
            .at(exe)?;
    } else {
        process::command("open").arg(exe).spawn().at(exe)?;
    }
    Ok(())
}

pub fn shortcut_icon(icon: Option<&Path>, _exe: &Path) -> Option<PathBuf> {
    icon.map(Path::to_path_buf)
}
