//! End-to-end: install, update, repair and uninstall a local tar.gz payload
//! inside an isolated HOME. Unix only (Windows paths come from known folders,
//! not environment variables).
#![cfg(unix)]

use installer_core::{
    CancelToken, Engine, InstallError, InstallOptions, InstallStatus, ManifestOrigin, Scope, SilentFrontend,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Once;

static ENV: Once = Once::new();
/// All tests share one HOME and the same `bin/hello` name, so run them one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn isolated_home() -> (PathBuf, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!("omnistall-e2e-{}", std::process::id()));
    ENV.call_once(|| {
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: set once before any test thread touches the environment-
        // dependent code paths; all tests in this binary share the same HOME.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
            std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
            std::env::set_var("PATH", "/usr/bin:/bin");
        }
    });
    (home, guard)
}

fn make_targz(dir: &Path, version: &str, extra: &str) -> PathBuf {
    let path = dir.join(format!("hello-{version}.tar.gz"));
    let f = std::fs::File::create(&path).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    let script = format!("#!/bin/sh\necho hello {version} {extra}\n");
    let mut h = tar::Header::new_gnu();
    h.set_size(script.len() as u64);
    h.set_mode(0o755);
    h.set_cksum();
    tar.append_data(&mut h, format!("hello-{version}/bin/hello"), script.as_bytes())
        .unwrap();
    let icon = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
    let mut h = tar::Header::new_gnu();
    h.set_size(icon.len() as u64);
    h.set_mode(0o644);
    h.set_cksum();
    tar.append_data(&mut h, format!("hello-{version}/icon.svg"), &icon[..])
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    path
}

fn manifest(name: &str, kind: &str, version: &str, archive: &Path, sha: &str, extra: &str) -> String {
    format!(
        r#"
schema = 1
[app]
name = "{name}"
display_name = "Hello Tool"
version = "{version}"
description = "test app"
kind = "{kind}"
icon = "icon.svg"

[sources.any]
type = "url"
url = "file://{archive}"
sha256 = "{sha}"
strip_components = 1

[install]
bin = ["bin/hello"]
executable = "bin/hello"
{extra}
"#,
        archive = archive.display()
    )
}

#[test]
fn install_update_uninstall_cli() {
    let (home, _serial) = isolated_home();
    let work = home.join("work-cli");
    std::fs::create_dir_all(&work).unwrap();
    let engine = Engine::new(Scope::User).unwrap();
    let fe = SilentFrontend { accept: true };
    let cancel = CancelToken::new();

    let a1 = make_targz(&work, "1.0.0", "");
    let sha1 = installer_core::checksum::sha256_file(&a1).unwrap();
    let marker = work.join("hook-ran");
    let hooks = format!("[hooks]\npost_install = [\"touch {}\"]\n", marker.display());
    let lm1 = engine
        .parse_manifest(
            &manifest("hello-cli", "cli", "1.0.0", &a1, &sha1, &hooks),
            ManifestOrigin::Embedded,
        )
        .unwrap();

    let opts = InstallOptions::default();
    let app = engine.install(&lm1, &opts, &fe, &cancel).unwrap();
    let install_dir = app.install_dir.clone().unwrap();
    assert!(install_dir.join("bin/hello").is_file(), "payload placed");
    let link = home.join(".local/bin/hello");
    assert!(link.symlink_metadata().is_ok(), "symlink on bin dir");
    assert!(marker.exists(), "post-install hook ran");
    let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
    assert!(profile.contains(".local/bin"), "PATH added to profile");
    assert!(matches!(
        engine.status(&lm1.manifest).unwrap(),
        InstallStatus::Installed {
            update_available: false,
            ..
        }
    ));

    // Same version again: no-op.
    let again = engine.install(&lm1, &opts, &fe, &cancel).unwrap();
    assert_eq!(again.version, "1.0.0");

    // Update to 1.1.0.
    let a2 = make_targz(&work, "1.1.0", "updated");
    let sha2 = installer_core::checksum::sha256_file(&a2).unwrap();
    let lm2 = engine
        .parse_manifest(
            &manifest("hello-cli", "cli", "1.1.0", &a2, &sha2, ""),
            ManifestOrigin::Embedded,
        )
        .unwrap();
    assert!(matches!(
        engine.status(&lm2.manifest).unwrap(),
        InstallStatus::Installed {
            update_available: true,
            ..
        }
    ));
    let app2 = engine.install(&lm2, &opts, &fe, &cancel).unwrap();
    assert_eq!(app2.version, "1.1.0");
    let body = std::fs::read_to_string(install_dir.join("bin/hello")).unwrap();
    assert!(body.contains("1.1.0 updated"));

    // Repair reinstalls the same version.
    std::fs::remove_file(&link).unwrap();
    let repaired = engine.repair("hello-cli", &fe, &cancel).unwrap();
    assert_eq!(repaired.version, "1.1.0");
    assert!(link.symlink_metadata().is_ok(), "repair restored the symlink");

    // Uninstall cleans everything.
    engine.uninstall("hello-cli", false, &fe, &cancel).unwrap();
    assert!(!install_dir.exists());
    assert!(link.symlink_metadata().is_err());
    assert!(engine.registry().unwrap().get("hello-cli").is_none());
    let profile = std::fs::read_to_string(home.join(".profile")).unwrap();
    assert!(!profile.contains("omnistall"), "profile block removed: {profile}");
}

#[test]
fn checksum_mismatch_fails_clearly() {
    let (home, _serial) = isolated_home();
    let work = home.join("work-sum");
    std::fs::create_dir_all(&work).unwrap();
    let engine = Engine::new(Scope::User).unwrap();
    let a = make_targz(&work, "2.0.0", "");
    let lm = engine
        .parse_manifest(
            &manifest("hello-bad", "cli", "2.0.0", &a, &"0".repeat(64), ""),
            ManifestOrigin::Embedded,
        )
        .unwrap();
    let err = engine
        .install(
            &lm,
            &InstallOptions::default(),
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, InstallError::ChecksumMismatch { .. }), "{err}");
    assert!(engine.registry().unwrap().get("hello-bad").is_none());
}

#[test]
fn declined_hook_aborts() {
    let (home, _serial) = isolated_home();
    let work = home.join("work-decline");
    std::fs::create_dir_all(&work).unwrap();
    let engine = Engine::new(Scope::User).unwrap();
    let a = make_targz(&work, "3.0.0", "");
    let sha = installer_core::checksum::sha256_file(&a).unwrap();
    let hooks = "[hooks]\npre_install = [\"true\"]\n";
    let lm = engine
        .parse_manifest(
            &manifest("hello-decline", "cli", "3.0.0", &a, &sha, hooks),
            ManifestOrigin::Embedded,
        )
        .unwrap();
    let err = engine
        .install(
            &lm,
            &InstallOptions::default(),
            &SilentFrontend { accept: false },
            &CancelToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, InstallError::Declined(_)), "{err}");
}

#[cfg(target_os = "linux")]
#[test]
fn gui_app_gets_desktop_entry() {
    let (home, _serial) = isolated_home();
    let work = home.join("work-gui");
    std::fs::create_dir_all(&work).unwrap();
    let engine = Engine::new(Scope::User).unwrap();
    let a = make_targz(&work, "4.0.0", "");
    let sha = installer_core::checksum::sha256_file(&a).unwrap();
    let extra = "\n[[file_associations]]\nextension = \".hello\"\nmime = \"application/x-hello\"\n";
    let lm = engine
        .parse_manifest(
            &manifest("hello-gui", "gui", "4.0.0", &a, &sha, extra),
            ManifestOrigin::Embedded,
        )
        .unwrap();
    let app = engine
        .install(
            &lm,
            &InstallOptions::default(),
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap();
    let desktop = home.join(".local/share/applications/omnistall-hello-gui.desktop");
    assert!(desktop.exists(), "desktop entry created");
    let text = std::fs::read_to_string(&desktop).unwrap();
    assert!(text.contains("Name=Hello Tool"));
    assert!(text.contains("Icon=") && text.contains("icon.svg"), "{text}");
    assert!(text.contains("MimeType=application/x-hello;"), "{text}");
    assert!(app.artifacts.iter().any(|p| p == &desktop));
    assert!(home.join(".local/share/mime/packages/omnistall-hello-gui.xml").exists());
    // GUI apps are not put on PATH by default.
    assert!(home.join(".local/bin/hello").symlink_metadata().is_err() || app.path_entries.is_empty());

    engine
        .uninstall(
            "hello-gui",
            false,
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap();
    assert!(!desktop.exists());
}

#[test]
fn dependency_is_installed_first() {
    let (home, _serial) = isolated_home();
    let work = home.join("work-dep");
    std::fs::create_dir_all(&work).unwrap();
    let engine = Engine::new(Scope::User).unwrap();
    let dep_archive = make_targz(&work, "5.0.0", "dep");
    let dep_sha = installer_core::checksum::sha256_file(&dep_archive).unwrap();
    let dep_manifest = work.join("dep.toml");
    std::fs::File::create(&dep_manifest)
        .unwrap()
        .write_all(manifest("hello-dep", "cli", "5.0.0", &dep_archive, &dep_sha, "").as_bytes())
        .unwrap();

    let main_archive = make_targz(&work, "6.0.0", "main");
    let main_sha = installer_core::checksum::sha256_file(&main_archive).unwrap();
    let main_text = manifest("hello-main", "cli", "6.0.0", &main_archive, &main_sha, "")
        + "\n[[dependencies]]\nname = \"hello-dep\"\nmanifest = \"dep.toml\"\nversion = \">=5\"\n";
    let main_manifest = work.join("main.toml");
    std::fs::write(&main_manifest, main_text).unwrap();

    let lm = engine.load_manifest(&main_manifest.display().to_string()).unwrap();
    let app = engine
        .install(
            &lm,
            &InstallOptions::default(),
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(app.dependencies, vec!["hello-dep".to_string()]);
    let reg = engine.registry().unwrap();
    assert!(reg.get("hello-dep").is_some());
    // Cannot remove a dependency that is still needed.
    assert!(
        engine
            .uninstall(
                "hello-dep",
                false,
                &SilentFrontend { accept: true },
                &CancelToken::new()
            )
            .is_err()
    );
    engine
        .uninstall(
            "hello-main",
            false,
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap();
    engine
        .uninstall(
            "hello-dep",
            false,
            &SilentFrontend { accept: true },
            &CancelToken::new(),
        )
        .unwrap();
}
