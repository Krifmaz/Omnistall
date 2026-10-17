//! Linux integration: freedesktop .desktop entries, MIME associations,
//! AppImage/deb handling, PATH via shell profiles.

use super::common::{NativeOutcome, ShortcutRequest};
use crate::archive::Format;
use crate::error::{InstallError, IoCtx, Result};
use crate::paths::Layout;
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt};
use crate::registry::{InstallMethod, InstalledApp};
use std::path::{Path, PathBuf};

pub use super::unix_shell::{add_to_path, expose_binary, remove_from_path};

fn desktop_file_name(app_id: &str) -> String {
    format!("omnistall-{app_id}.desktop")
}

fn desktop_entry(req: &ShortcutRequest<'_>) -> String {
    let quote = |p: &Path| {
        let s = p.display().to_string();
        if s.contains(' ') { format!("\"{s}\"") } else { s }
    };
    let mut exec = quote(req.exec);
    if let Some(a) = req.args {
        exec.push(' ');
        exec.push_str(a);
    } else if !req.mime_types.is_empty() {
        exec.push_str(" %U");
    }
    let mut s = String::from("[Desktop Entry]\nType=Application\nVersion=1.5\n");
    s.push_str(&format!("Name={}\n", req.name));
    if let Some(c) = req.comment {
        s.push_str(&format!("Comment={c}\n"));
    }
    s.push_str(&format!("Exec={exec}\n"));
    if let Some(d) = req.working_dir {
        s.push_str(&format!("Path={}\n", d.display()));
    }
    if let Some(i) = req.icon {
        s.push_str(&format!("Icon={}\n", i.display()));
    }
    s.push_str(&format!("Terminal={}\n", req.terminal));
    let cats = if req.categories.is_empty() {
        "Utility;".to_string()
    } else {
        let mut c = req.categories.join(";");
        c.push(';');
        c
    };
    s.push_str(&format!("Categories={cats}\n"));
    if !req.mime_types.is_empty() {
        s.push_str(&format!("MimeType={};\n", req.mime_types.join(";")));
    }
    s.push_str(&format!("X-Omnistall-App={}\n", req.app_id));
    s
}

pub fn create_menu_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    std::fs::create_dir_all(&layout.menu_dir).at(&layout.menu_dir)?;
    let path = layout.menu_dir.join(desktop_file_name(req.app_id));
    std::fs::write(&path, desktop_entry(req)).at(&path)?;
    crate::archive::make_executable(&path)?;
    artifacts.push(path);
    let _ = process::command("update-desktop-database")
        .arg(&layout.menu_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

pub fn create_desktop_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    let Some(desktop) = &layout.desktop_dir else {
        return Ok(());
    };
    let path = desktop.join(desktop_file_name(req.app_id));
    std::fs::write(&path, desktop_entry(req)).at(&path)?;
    crate::archive::make_executable(&path)?;
    // GNOME marks launchers untrusted until the user allows them; set the
    // metadata flag when gio is available so the icon works immediately.
    let _ = process::command("gio")
        .args(["set", &path.display().to_string(), "metadata::trusted", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    artifacts.push(path);
    Ok(())
}

pub fn register_file_associations(
    layout: &Layout,
    app_id: &str,
    assocs: &[crate::manifest::FileAssociation],
    frontend: &dyn Frontend,
) -> Result<(Vec<String>, Vec<PathBuf>)> {
    if assocs.is_empty() {
        return Ok((vec![], vec![]));
    }
    let mime_dir = dirs::data_dir()
        .map(|d| d.join("mime"))
        .unwrap_or_else(|| layout.home.join(".local/share/mime"));
    let packages = mime_dir.join("packages");
    std::fs::create_dir_all(&packages).at(&packages)?;
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
    );
    let mut mimes = Vec::new();
    for a in assocs {
        let mime = a
            .mime
            .clone()
            .unwrap_or_else(|| format!("application/x-{}-{}", app_id, a.extension.trim_start_matches('.')));
        xml.push_str(&format!("  <mime-type type=\"{mime}\">\n"));
        if let Some(d) = &a.description {
            xml.push_str(&format!("    <comment>{d}</comment>\n"));
        }
        xml.push_str(&format!("    <glob pattern=\"*{}\"/>\n  </mime-type>\n", a.extension));
        mimes.push(mime);
    }
    xml.push_str("</mime-info>\n");
    let file = packages.join(format!("omnistall-{app_id}.xml"));
    std::fs::write(&file, xml).at(&file)?;
    let _ = process::command("update-mime-database")
        .arg(&mime_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    for m in &mimes {
        let st = process::command("xdg-mime")
            .args(["default", &desktop_file_name(app_id), m])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if !matches!(st, Ok(s) if s.success()) {
            frontend.warn(format!("could not set {m} default via xdg-mime"));
        }
    }
    Ok((mimes, vec![file]))
}

pub fn unregister_file_associations(_layout: &Layout, _mimes: &[String]) -> Result<()> {
    if let Some(mime_dir) = dirs::data_dir().map(|d| d.join("mime")) {
        let _ = process::command("update-mime-database")
            .arg(&mime_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    Ok(())
}

/// Handle non-archive payloads. Returns `None` when the orchestrator should
/// treat the file as a plain payload (e.g. a deb without root).
pub fn install_native(
    format: Format,
    file: &Path,
    staging: &Path,
    display_name: &str,
    _spec: &crate::manifest::InstallSpec,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<Option<NativeOutcome>> {
    match format {
        Format::AppImage => {
            std::fs::create_dir_all(staging).at(staging)?;
            let dest = staging.join(format!("{}.AppImage", crate::fsutil::safe_file_name(display_name)));
            std::fs::copy(file, &dest).at(&dest)?;
            crate::archive::make_executable(&dest)?;
            Ok(Some(NativeOutcome {
                method: InstallMethod::AppImage,
                executable: Some(dest),
                app_bundle: None,
                deb_package: None,
                msi_cache: None,
                native_uninstall: None,
            }))
        }
        Format::Deb => {
            if crate::platform::Platform::is_elevated() && which::which("dpkg").is_ok() {
                let pkg = crate::archive::deb_package_name(file);
                let mut cmd = process::command("dpkg");
                cmd.arg("-i").arg(file).env("DEBIAN_FRONTEND", "noninteractive");
                let st = process::run_logged(&mut cmd, frontend, cancel)?;
                if !st.success() {
                    return Err(InstallError::NativeInstaller {
                        command: format!("dpkg -i {}", file.display()),
                        code: st.code(),
                    });
                }
                let exe = pkg.as_ref().and_then(|p| which::which(p).ok());
                return Ok(Some(NativeOutcome {
                    method: InstallMethod::DebDpkg,
                    executable: exe,
                    app_bundle: None,
                    deb_package: pkg,
                    msi_cache: None,
                    native_uninstall: None,
                }));
            }
            frontend.log("not root: unpacking the .deb into the per-user install directory instead of using dpkg");
            Ok(None)
        }
        Format::Dmg | Format::Pkg | Format::Msi | Format::Exe => Err(InstallError::UnsupportedFormat(format!(
            "{format:?} installers cannot be used on Linux"
        ))),
        _ => Ok(None),
    }
}

pub fn uninstall_native(app: &InstalledApp, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
    match app.method {
        InstallMethod::DebDpkg => {
            let Some(pkg) = &app.deb_package else {
                return Ok(());
            };
            if !crate::platform::Platform::is_elevated() {
                return Err(InstallError::NeedsElevation);
            }
            let mut cmd = process::command("dpkg");
            cmd.args(["-r", pkg]);
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                return Err(InstallError::NativeInstaller {
                    command: format!("dpkg -r {pkg}"),
                    code: st.code(),
                });
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub fn launch(app: &InstalledApp, is_cli: bool) -> Result<()> {
    let Some(exe) = &app.executable else {
        return Err(InstallError::Other(anyhow::anyhow!(
            "no executable recorded for {}",
            app.name
        )));
    };
    if is_cli {
        // Open a terminal with the tool on PATH.
        let candidates: &[(&str, &[&str])] = &[
            ("x-terminal-emulator", &["-e"]),
            ("gnome-terminal", &["--"]),
            ("konsole", &["-e"]),
            ("xfce4-terminal", &["-x"]),
            ("xterm", &["-e"]),
        ];
        for (term, args) in candidates {
            if which::which(term).is_ok() {
                let script = format!(
                    "\"{}\" --help; echo; echo 'Press Enter to close'; read _",
                    exe.display()
                );
                process::command(term)
                    .args(*args)
                    .args(["sh", "-c", &script])
                    .spawn()
                    .at(exe)?;
                return Ok(());
            }
        }
        return Err(InstallError::Other(anyhow::anyhow!("no terminal emulator found")));
    }
    let mut cmd = process::command(exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .at(exe)?;
    Ok(())
}

/// Convert a PNG/SVG icon to something the desktop can display: on Linux the
/// absolute path works directly.
pub fn shortcut_icon(icon: Option<&Path>, _exe: &Path) -> Option<PathBuf> {
    icon.map(Path::to_path_buf)
}
