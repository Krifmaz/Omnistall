//! Register the installer itself as the handler for `.omni` files.
//!
//! A `.omni` file is simply a manifest. Once any Omnistall installer has run
//! on a machine, double-clicking `SomeApp.omni` opens the installer for that
//! app, so publishers can ship one tiny file for every operating system.
//!
//! Registration is per-user and best-effort; failures are logged, never fatal.

use crate::error::Result;
use std::path::{Path, PathBuf};

pub const EXTENSION: &str = "omni";
pub const MIME_TYPE: &str = "application/x-omnistall-manifest";
pub const PROG_ID: &str = "Omnistall.Manifest";

/// The file that should be registered as the handler: the AppImage when
/// running from one, otherwise the current executable.
pub fn self_path() -> Option<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return Some(PathBuf::from(appimage));
    }
    std::env::current_exe().ok()
}

/// Register `exe` as the opener for `.omni` files. Returns `Ok(true)` when
/// something was written.
pub fn register(exe: &Path) -> Result<bool> {
    imp::register(exe)
}

/// Is `path` a manifest document we should open directly?
pub fn is_manifest_document(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|e| e == EXTENSION || e == "toml")
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use crate::error::IoCtx;

    pub fn register(exe: &Path) -> Result<bool> {
        let Some(data) = dirs::data_dir() else { return Ok(false) };
        let apps = data.join("applications");
        let mime_dir = data.join("mime");
        let packages = mime_dir.join("packages");
        std::fs::create_dir_all(&apps).at(&apps)?;
        std::fs::create_dir_all(&packages).at(&packages)?;

        let icon = crate::paths::Layout::detect(crate::paths::Scope::User)
            .map(|l| l.data_dir.join("omnistall.png"))
            .ok();
        let icon_line = match icon.filter(|p| p.exists()) {
            Some(p) => format!("Icon={}\n", p.display()),
            None => "Icon=system-software-install\n".to_string(),
        };
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Omnistall\nComment=Install applications from .omni files\nExec=\"{}\" %f\n{icon_line}Terminal=false\nNoDisplay=true\nCategories=System;\nMimeType={MIME_TYPE};\n",
            exe.display()
        );
        let desktop_path = apps.join("omnistall.desktop");
        let changed = std::fs::read_to_string(&desktop_path).ok().as_deref() != Some(desktop.as_str());
        std::fs::write(&desktop_path, desktop).at(&desktop_path)?;

        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n  <mime-type type=\"{MIME_TYPE}\">\n    <comment>Omnistall app manifest</comment>\n    <glob pattern=\"*.{EXTENSION}\"/>\n  </mime-type>\n</mime-info>\n"
        );
        let xml_path = packages.join("omnistall.xml");
        if std::fs::read_to_string(&xml_path).ok().as_deref() != Some(xml.as_str()) {
            std::fs::write(&xml_path, xml).at(&xml_path)?;
            let quiet = || std::process::Stdio::null();
            let _ = crate::process::command("update-mime-database")
                .arg(&mime_dir)
                .stdout(quiet())
                .stderr(quiet())
                .status();
            let _ = crate::process::command("update-desktop-database")
                .arg(&apps)
                .stdout(quiet())
                .stderr(quiet())
                .status();
        }
        let _ = crate::process::command("xdg-mime")
            .args(["default", "omnistall.desktop", MIME_TYPE])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Ok(changed)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::error::IoCtx;
    use winreg::RegKey;
    use winreg::enums::*;

    pub fn register(exe: &Path) -> Result<bool> {
        let classes = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(r"Software\Classes", KEY_READ | KEY_WRITE)
            .at("HKCU\\Software\\Classes")?;
        let ext = format!(".{EXTENSION}");
        let (ext_key, _) = classes.create_subkey(&ext).at(&ext)?;
        ext_key.set_value("", &PROG_ID.to_string()).at(&ext)?;
        ext_key.set_value("Content Type", &MIME_TYPE.to_string()).at(&ext)?;
        let (pk, _) = classes.create_subkey(PROG_ID).at(PROG_ID)?;
        pk.set_value("", &"Omnistall app manifest".to_string()).at(PROG_ID)?;
        let (icon_k, _) = pk.create_subkey("DefaultIcon").at(PROG_ID)?;
        icon_k.set_value("", &format!("{},0", exe.display())).at(PROG_ID)?;
        let (cmd_k, _) = pk.create_subkey(r"shell\open\command").at(PROG_ID)?;
        let command = format!("\"{}\" \"%1\"", exe.display());
        let previous: String = cmd_k.get_value("").unwrap_or_default();
        cmd_k.set_value("", &command).at(PROG_ID)?;
        if previous != command {
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
        Ok(previous != command)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    /// The document type is declared in the bundle's Info.plist
    /// (`CFBundleDocumentTypes` + `UTExportedTypeDeclarations`, written by
    /// `xtask app-bundle`); Launch Services only needs to see the bundle.
    pub fn register(exe: &Path) -> Result<bool> {
        let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"));
        if let Some(bundle) = bundle {
            let _ = crate::process::command(
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
            )
            .args(["-f", &bundle.display().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
            return Ok(true);
        }
        Ok(false)
    }
}
