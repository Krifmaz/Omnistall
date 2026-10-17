//! Windows integration: Start Menu / Desktop `.lnk` shortcuts, user PATH in
//! the registry, HKCU file associations, MSI / EXE installers.

use super::common::{ShortcutRequest, expand_env, on_path};
use crate::archive::Format;
use crate::error::{InstallError, IoCtx, Result};
use crate::paths::Layout;
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt};
use crate::registry::{InstallMethod, InstalledApp};
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::*;

pub use super::common::NativeOutcome;

fn ps(script: &str) -> std::process::Command {
    let mut c = process::command("powershell");
    c.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    c
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn make_lnk(lnk: &Path, req: &ShortcutRequest<'_>) -> Result<()> {
    if let Some(parent) = lnk.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }
    let icon = shortcut_icon(req.icon, req.exec)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| format!("{},0", req.exec.display()));
    let workdir = req
        .working_dir
        .map(Path::to_path_buf)
        .or_else(|| req.exec.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({lnk}); $s.TargetPath={exe}; $s.Arguments={args}; $s.WorkingDirectory={wd}; $s.IconLocation={icon}; $s.Description={desc}; $s.Save()",
        lnk = ps_quote(&lnk.display().to_string()),
        exe = ps_quote(&req.exec.display().to_string()),
        args = ps_quote(req.args.unwrap_or("")),
        wd = ps_quote(&workdir.display().to_string()),
        icon = ps_quote(&icon),
        desc = ps_quote(req.comment.unwrap_or(req.name)),
    );
    let out = ps(&script).output().at(lnk)?;
    if !out.status.success() {
        return Err(InstallError::Other(anyhow::anyhow!(
            "creating shortcut failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

pub fn create_menu_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    let lnk = layout
        .menu_dir
        .join(format!("{}.lnk", crate::fsutil::safe_file_name(req.name)));
    make_lnk(&lnk, req)?;
    artifacts.push(lnk);
    Ok(())
}

pub fn create_desktop_shortcut(layout: &Layout, req: &ShortcutRequest<'_>, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    let Some(desktop) = &layout.desktop_dir else {
        return Ok(());
    };
    let lnk = desktop.join(format!("{}.lnk", crate::fsutil::safe_file_name(req.name)));
    make_lnk(&lnk, req)?;
    artifacts.push(lnk);
    Ok(())
}

/// `.lnk` icons must be .ico/.exe/.dll; fall back to the executable's icon.
pub fn shortcut_icon(icon: Option<&Path>, exe: &Path) -> Option<PathBuf> {
    match icon {
        Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ico")) => Some(p.to_path_buf()),
        _ => Some(PathBuf::from(format!("{},0", exe.display()))),
    }
}

fn env_key(layout: &Layout) -> std::io::Result<RegKey> {
    match layout.scope {
        crate::paths::Scope::User => {
            RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        }
        crate::paths::Scope::System => RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            KEY_READ | KEY_WRITE,
        ),
    }
}

fn broadcast_env_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };
    let env: Vec<u16> = "Environment\0".encode_utf16().collect();
    // SAFETY: broadcasting a standard window message with a valid string.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        );
    }
}

pub fn add_to_path(layout: &Layout, dir: &Path) -> Result<bool> {
    let key = env_key(layout).at("HKCU\\Environment")?;
    let current: String = key.get_value("Path").unwrap_or_default();
    let want = dir.display().to_string();
    let present = current.split(';').any(|p| {
        p.trim_end_matches('\\')
            .eq_ignore_ascii_case(want.trim_end_matches('\\'))
    });
    if present || on_path(dir) {
        return Ok(false);
    }
    let new = if current.trim().is_empty() {
        want
    } else {
        format!("{};{}", current.trim_end_matches(';'), want)
    };
    key.set_value("Path", &new).at("HKCU\\Environment\\Path")?;
    broadcast_env_change();
    Ok(true)
}

pub fn remove_from_path(layout: &Layout, dir: &Path) -> Result<()> {
    let key = env_key(layout).at("HKCU\\Environment")?;
    let current: String = key.get_value("Path").unwrap_or_default();
    let want = dir.display().to_string();
    let kept: Vec<&str> = current
        .split(';')
        .filter(|p| {
            !p.trim().is_empty()
                && !p
                    .trim_end_matches('\\')
                    .eq_ignore_ascii_case(want.trim_end_matches('\\'))
        })
        .collect();
    key.set_value("Path", &kept.join(";")).at("HKCU\\Environment\\Path")?;
    broadcast_env_change();
    Ok(())
}

/// Windows has no symlink-on-PATH convention; the executable's directory is
/// added to PATH instead. Returns that directory.
pub fn expose_binary(_layout: &Layout, target: &Path, _name: &str) -> Result<PathBuf> {
    Ok(target.parent().map(Path::to_path_buf).unwrap_or_default())
}

pub fn register_file_associations(
    _layout: &Layout,
    app_id: &str,
    assocs: &[crate::manifest::FileAssociation],
    exe: Option<&Path>,
    frontend: &dyn Frontend,
) -> Result<Vec<String>> {
    let Some(exe) = exe else {
        if !assocs.is_empty() {
            frontend.warn("file associations skipped: no executable known");
        }
        return Ok(vec![]);
    };
    let classes = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Classes", KEY_READ | KEY_WRITE)
        .at("HKCU\\Software\\Classes")?;
    let mut keys = Vec::new();
    for a in assocs {
        let prog_id = format!("Omnistall.{}{}", app_id, a.extension);
        let (ext_key, _) = classes.create_subkey(&a.extension).at(&a.extension)?;
        ext_key.set_value("", &prog_id).at(&a.extension)?;
        let (pk, _) = classes.create_subkey(&prog_id).at(&prog_id)?;
        pk.set_value("", &a.description.clone().unwrap_or_else(|| format!("{} file", app_id)))
            .at(&prog_id)?;
        let (icon_k, _) = pk.create_subkey("DefaultIcon").at(&prog_id)?;
        icon_k.set_value("", &format!("{},0", exe.display())).at(&prog_id)?;
        let (cmd_k, _) = pk.create_subkey(r"shell\open\command").at(&prog_id)?;
        cmd_k
            .set_value("", &format!("\"{}\" \"%1\"", exe.display()))
            .at(&prog_id)?;
        keys.push(format!(r"Software\Classes\{}", a.extension));
        keys.push(format!(r"Software\Classes\{prog_id}"));
    }
    notify_assoc_changed();
    Ok(keys)
}

pub fn unregister_file_associations(keys: &[String]) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for k in keys {
        let _ = hkcu.delete_subkey_all(k);
    }
    notify_assoc_changed();
    Ok(())
}

fn notify_assoc_changed() {
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: documented call with null item pointers.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
}

pub fn install_native(
    format: Format,
    file: &Path,
    _staging: &Path,
    _display_name: &str,
    spec: &crate::manifest::InstallSpec,
    cache_dir: &Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<Option<NativeOutcome>> {
    match format {
        Format::Msi => {
            std::fs::create_dir_all(cache_dir).at(cache_dir)?;
            let cached = cache_dir.join(file.file_name().unwrap_or_default());
            std::fs::copy(file, &cached).at(&cached)?;
            let mut cmd = process::command("msiexec");
            cmd.arg("/i").arg(&cached).args(["/qn", "/norestart"]);
            if !crate::platform::Platform::is_elevated() {
                // Per-user install: never triggers UAC for packages that support it.
                cmd.args(["ALLUSERS=\"\"", "MSIINSTALLPERUSER=1"]);
            }
            for p in &spec.msi_properties {
                cmd.arg(expand_env(p));
            }
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                // 1641/3010 = success, reboot requested.
                if !matches!(st.code(), Some(1641) | Some(3010)) {
                    return Err(InstallError::NativeInstaller {
                        command: format!("msiexec /i {}", cached.display()),
                        code: st.code(),
                    });
                }
                frontend.warn("the installer requested a restart to finish");
            }
            Ok(Some(NativeOutcome {
                method: InstallMethod::Msi,
                executable: spec.executable.as_ref().map(|e| PathBuf::from(expand_env(e))),
                app_bundle: None,
                deb_package: None,
                msi_cache: Some(cached),
                native_uninstall: None,
            }))
        }
        Format::Exe => {
            let mut cmd = process::command(file);
            for a in &spec.installer_args {
                cmd.arg(expand_env(a));
            }
            if spec.installer_args.is_empty() {
                frontend.warn("no installer_args given; the setup program may show its own wizard");
            }
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                return Err(InstallError::NativeInstaller {
                    command: file.display().to_string(),
                    code: st.code(),
                });
            }
            Ok(Some(NativeOutcome {
                method: InstallMethod::ExeInstaller,
                executable: spec.executable.as_ref().map(|e| PathBuf::from(expand_env(e))),
                app_bundle: None,
                deb_package: None,
                msi_cache: None,
                native_uninstall: spec.uninstall_command.clone().map(|c| expand_env(&c)),
            }))
        }
        Format::Dmg | Format::Pkg | Format::Deb | Format::AppImage => Err(InstallError::UnsupportedFormat(format!(
            "{format:?} cannot be used on Windows"
        ))),
        _ => Ok(None),
    }
}

/// Processes whose executable lives under `dir`: `(pid, name)`.
pub fn processes_in(dir: &Path) -> Vec<(u32, String)> {
    let prefix = dir.display().to_string().trim_end_matches('\\').to_string();
    let script = format!(
        "Get-Process | Where-Object {{ $_.Path -and $_.Path.StartsWith({}, [System.StringComparison]::OrdinalIgnoreCase) }} | ForEach-Object {{ \"$($_.Id)|$($_.ProcessName)\" }}",
        ps_quote(&format!("{prefix}\\"))
    );
    let Ok(out) = ps(&script).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (pid, name) = l.trim().split_once('|')?;
            Some((pid.parse().ok()?, name.to_string()))
        })
        .collect()
}

pub fn terminate(procs: &[(u32, String)], frontend: &dyn Frontend) {
    for (pid, name) in procs {
        frontend.log(format!("closing {name} (pid {pid})"));
        let _ = ps(&format!("Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue")).output();
    }
}

pub fn uninstall_native(app: &InstalledApp, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
    match app.method {
        InstallMethod::Msi => {
            let Some(msi) = &app.msi_cache else {
                frontend.warn("original .msi not cached; remove the app from Windows Settings > Apps");
                return Ok(());
            };
            let mut cmd = process::command("msiexec");
            cmd.arg("/x").arg(msi).args(["/qn", "/norestart"]);
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() && !matches!(st.code(), Some(1605) | Some(3010) | Some(1641)) {
                return Err(InstallError::NativeInstaller {
                    command: format!("msiexec /x {}", msi.display()),
                    code: st.code(),
                });
            }
            let _ = std::fs::remove_file(msi);
            Ok(())
        }
        InstallMethod::ExeInstaller => {
            if let Some(cmd) = &app.native_uninstall {
                let st = process::run_logged(&mut process::shell(cmd), frontend, cancel)?;
                if !st.success() {
                    return Err(InstallError::NativeInstaller {
                        command: cmd.clone(),
                        code: st.code(),
                    });
                }
            } else {
                frontend.warn(
                    "this app's setup program provides no silent uninstaller; remove it from Windows Settings > Apps",
                );
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
        let mut cmd = process::command("cmd");
        cmd.args(["/C", "start", "", "cmd", "/K"])
            .arg(format!("\"{}\" --help", exe.display()));
        cmd.spawn().at(exe)?;
        return Ok(());
    }
    let mut cmd = std::process::Command::new(exe);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().at(exe)?;
    Ok(())
}
