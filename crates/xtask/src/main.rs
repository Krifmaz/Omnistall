//! `cargo xtask <command>` – packaging helpers used by CI and by publishers.
//!
//! * `brand`      append a manifest to a built installer (exe / AppImage / ELF)
//! * `validate`   parse a manifest and print what it would do per platform
//! * `appdir`     assemble a Linux AppDir (then run `appimagetool` on it)
//! * `deb`        build a Debian package with `dpkg-deb`
//! * `app-bundle` assemble a macOS `.app` (then `codesign` + `hdiutil`)
//! * `sha256`     write a SHA256SUMS file

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(name = "xtask", about = "Omnistall developer tasks")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Append a manifest to an installer binary so it installs that app.
    /// With --icon, a Windows .exe also gets that icon as its file icon.
    Brand {
        #[arg(long)]
        installer: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// PNG icon (square, ideally 256px or larger).
        #[arg(long)]
        icon: Option<PathBuf>,
    },
    /// Validate a manifest and show the selected source for each platform.
    Validate { manifest: PathBuf },
    /// Assemble an AppDir for appimagetool.
    Appdir {
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long, default_value = "Omnistall")]
        name: String,
        /// PNG icon replacing the default one.
        #[arg(long)]
        icon: Option<PathBuf>,
    },
    /// Build a .deb with dpkg-deb.
    Deb {
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        version: String,
        /// Debian architecture: amd64 or arm64.
        #[arg(long)]
        arch: String,
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// PNG icon replacing the default one.
        #[arg(long)]
        icon: Option<PathBuf>,
    },
    /// Assemble a macOS .app bundle.
    AppBundle {
        #[arg(long)]
        binary: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        manifest: Option<PathBuf>,
        #[arg(long, default_value = "Omnistall")]
        name: String,
        /// PNG icon replacing the default one (converted to .icns on macOS).
        #[arg(long)]
        icon: Option<PathBuf>,
    },
    /// Write SHA256SUMS for the given files.
    Sha256 {
        #[arg(long)]
        out: PathBuf,
        files: Vec<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = workspace_root();
    match cli.cmd {
        Cmd::Brand {
            installer,
            manifest,
            out,
            icon,
        } => {
            let text = std::fs::read_to_string(&manifest).with_context(|| format!("reading {}", manifest.display()))?;
            installer_core::Manifest::parse(&text).map_err(|e| anyhow::anyhow!("invalid manifest: {e}"))?;
            let mut source = installer.clone();
            let tmp;
            if let Some(icon) = &icon {
                if is_pe(&installer)? {
                    tmp = out.with_extension("icon.tmp");
                    set_exe_icon(&installer, icon, &tmp)?;
                    source = tmp.clone();
                } else {
                    eprintln!(
                        "note: --icon only changes the file icon of Windows executables; use `appdir`/`app-bundle --icon` for Linux/macOS packaging"
                    );
                }
            }
            installer_core::locator::append_trailer(&source, &text, &out)?;
            if source != installer {
                let _ = std::fs::remove_file(&source);
            }
            println!("wrote {}", out.display());
        }
        Cmd::Validate { manifest } => {
            let text = std::fs::read_to_string(&manifest)?;
            let m = installer_core::Manifest::parse(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("{} {} ({:?})", m.app.display_name(), m.app.version, m.app.kind);
            use installer_core::{Arch, Os, Platform};
            for os in [Os::Linux, Os::Macos, Os::Windows] {
                for arch in [Arch::X86_64, Arch::Aarch64] {
                    let p = Platform::new(os, arch);
                    match m.select_source(&p, false) {
                        Some(s) => println!(
                            "  {:<16} -> sources.{}{}",
                            p.key(),
                            s.key,
                            if s.emulated { " (emulated)" } else { "" }
                        ),
                        None if m.build.is_some() => println!("  {:<16} -> build from source", p.key()),
                        None => println!("  {:<16} -> NOT AVAILABLE", p.key()),
                    }
                }
            }
        }
        Cmd::Appdir {
            binary,
            out,
            manifest,
            name,
            icon,
        } => {
            let icon_src = icon.unwrap_or_else(|| root.join("assets/icon-256.png"));
            let _ = std::fs::remove_dir_all(&out);
            let bin_dir = out.join("usr").join("bin");
            std::fs::create_dir_all(&bin_dir)?;
            copy_exec(&binary, &bin_dir.join("omnistall"))?;
            std::fs::write(
                out.join("omnistall.desktop"),
                desktop_entry(&name, "omnistall", "omnistall"),
            )?;
            write_png(&icon_src, 256, &out.join("omnistall.png"))?;
            std::fs::copy(out.join("omnistall.png"), out.join(".DirIcon"))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink("usr/bin/omnistall", out.join("AppRun"))?;
            if let Some(m) = manifest {
                std::fs::copy(&m, out.join("manifest.toml"))?;
            }
            println!("AppDir ready at {}", out.display());
        }
        Cmd::Deb {
            binary,
            out,
            version,
            arch,
            manifest,
            icon,
        } => {
            let icon_src = icon.unwrap_or_else(|| root.join("assets/icon-256.png"));
            let stage = out.with_extension("stage");
            let _ = std::fs::remove_dir_all(&stage);
            let bin_dir = stage.join("usr/bin");
            let app_dir = stage.join("usr/share/applications");
            let icon_dir = stage.join("usr/share/icons/hicolor/256x256/apps");
            let data_dir = stage.join("usr/share/omnistall");
            for d in [&bin_dir, &app_dir, &icon_dir, &data_dir, &stage.join("DEBIAN")] {
                std::fs::create_dir_all(d)?;
            }
            copy_exec(&binary, &bin_dir.join("omnistall"))?;
            std::fs::write(
                app_dir.join("omnistall.desktop"),
                desktop_entry("Omnistall", "omnistall", "omnistall"),
            )?;
            write_png(&icon_src, 256, &icon_dir.join("omnistall.png"))?;
            if let Some(m) = manifest {
                std::fs::copy(&m, data_dir.join("manifest.toml"))?;
            }
            let size_kb = dir_size(&stage)? / 1024;
            std::fs::write(
                stage.join("DEBIAN/control"),
                format!(
                    "Package: omnistall\nVersion: {version}\nSection: utils\nPriority: optional\nArchitecture: {arch}\nInstalled-Size: {size_kb}\nDepends: libc6 (>= 2.35), libxkbcommon0, libxkbcommon-x11-0\nRecommends: libvulkan1\nMaintainer: Omnistall contributors <noreply@example.com>\nHomepage: https://github.com/Krifmaz/Omnistall\nDescription: Universal manifest-driven application installer\n A graphical installer that installs any application described by a\n manifest: downloads, verifies, unpacks and integrates it for the user.\n"
                ),
            )?;
            run(Command::new("dpkg-deb")
                .args(["--build", "--root-owner-group"])
                .arg(&stage)
                .arg(&out))?;
            let _ = std::fs::remove_dir_all(&stage);
            println!("wrote {}", out.display());
        }
        Cmd::AppBundle {
            binary,
            out,
            version,
            manifest,
            name,
            icon,
        } => {
            let icon_src = icon.unwrap_or_else(|| root.join("assets/icon-256.png"));
            let _ = std::fs::remove_dir_all(&out);
            let contents = out.join("Contents");
            let macos = contents.join("MacOS");
            let resources = contents.join("Resources");
            std::fs::create_dir_all(&macos)?;
            std::fs::create_dir_all(&resources)?;
            copy_exec(&binary, &macos.join("omnistall"))?;
            if let Some(m) = manifest {
                std::fs::copy(&m, resources.join("manifest.toml"))?;
            }
            // Icon: build an .icns with iconutil when available (macOS), else ship the PNG.
            let icon_file = match build_icns(&icon_src, &resources) {
                Ok(()) => "AppIcon.icns",
                Err(e) => {
                    eprintln!("note: iconutil unavailable ({e}); using PNG icon");
                    write_png(&icon_src, 512, &resources.join("AppIcon.png"))?;
                    "AppIcon.png"
                }
            };
            std::fs::write(
                contents.join("Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>{name}</string>
  <key>CFBundleDisplayName</key><string>{name}</string>
  <key>CFBundleIdentifier</key><string>dev.omnistall.installer</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleExecutable</key><string>omnistall</string>
  <key>CFBundleIconFile</key><string>{icon_file}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>MIT</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Omnistall app manifest</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSItemContentTypes</key><array><string>dev.omnistall.manifest</string></array>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>dev.omnistall.manifest</string>
      <key>UTTypeDescription</key><string>Omnistall app manifest</string>
      <key>UTTypeConformsTo</key><array><string>public.plain-text</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key><array><string>omni</string></array>
        <key>public.mime-type</key><array><string>application/x-omnistall-manifest</string></array>
      </dict>
    </dict>
  </array>
</dict>
</plist>
"#
                ),
            )?;
            std::fs::write(contents.join("PkgInfo"), "APPL????")?;
            println!("bundle ready at {}", out.display());
        }
        Cmd::Sha256 { out, files } => {
            use sha2::Digest;
            let mut text = String::new();
            for f in files {
                let data = std::fs::read(&f)?;
                let sum = hex::encode(sha2::Sha256::digest(&data));
                let name = f.file_name().unwrap().to_string_lossy();
                text.push_str(&format!("{sum}  {name}\n"));
            }
            std::fs::write(&out, text)?;
            println!("wrote {}", out.display());
        }
    }
    Ok(())
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn desktop_entry(name: &str, exec: &str, icon: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nComment=Install applications\nExec={exec}\nIcon={icon}\nTerminal=false\nCategories=System;Utility;\nStartupWMClass=omnistall\n"
    )
}

fn copy_exec(src: &Path, dst: &Path) -> Result<()> {
    std::fs::copy(src, dst).with_context(|| format!("copying {} -> {}", src.display(), dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            dir_size(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

fn build_icns(icon: &Path, resources: &Path) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("not macOS");
    }
    let iconset = resources.join("AppIcon.iconset");
    std::fs::create_dir_all(&iconset)?;
    for (size, names) in [
        (16, vec!["icon_16x16.png"]),
        (32, vec!["icon_16x16@2x.png", "icon_32x32.png"]),
        (64, vec!["icon_32x32@2x.png"]),
        (128, vec!["icon_128x128.png"]),
        (256, vec!["icon_128x128@2x.png", "icon_256x256.png"]),
        (512, vec!["icon_256x256@2x.png", "icon_512x512.png"]),
        (1024, vec!["icon_512x512@2x.png"]),
    ] {
        for n in names {
            write_png(icon, size, &iconset.join(n))?;
        }
    }
    run(Command::new("iconutil")
        .args(["-c", "icns"])
        .arg(&iconset)
        .arg("-o")
        .arg(resources.join("AppIcon.icns")))?;
    let _ = std::fs::remove_dir_all(&iconset);
    Ok(())
}

/// Load an image (PNG/JPEG/ICO…) and write it as a square PNG of `size` px.
fn write_png(src: &Path, size: u32, dst: &Path) -> Result<()> {
    let img = image::open(src).with_context(|| format!("reading icon {}", src.display()))?;
    let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    resized
        .save_with_format(dst, image::ImageFormat::Png)
        .with_context(|| format!("writing {}", dst.display()))?;
    Ok(())
}

fn is_pe(path: &Path) -> Result<bool> {
    let mut head = [0u8; 2];
    use std::io::Read;
    std::fs::File::open(path)?.read_exact(&mut head)?;
    Ok(&head == b"MZ")
}

/// Replace the main icon resource of a Windows executable (pure Rust, runs on any OS).
fn set_exe_icon(exe: &Path, icon: &Path, out: &Path) -> Result<()> {
    let mut image = editpe::Image::parse_file(exe).map_err(|e| anyhow::anyhow!("parsing {}: {e}", exe.display()))?;
    let mut resources = image.resource_directory().cloned().unwrap_or_default();
    let img = image::open(icon).with_context(|| format!("reading icon {}", icon.display()))?;
    resources
        .set_main_icon(img)
        .map_err(|e| anyhow::anyhow!("setting icon: {e}"))?;
    image
        .set_resource_directory(resources)
        .map_err(|e| anyhow::anyhow!("updating resources: {e}"))?;
    image
        .write_file(out)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", out.display()))?;
    Ok(())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status().with_context(|| format!("running {cmd:?}"))?;
    if !status.success() {
        bail!("{cmd:?} failed with {status}");
    }
    Ok(())
}
