//! Detecting and (with consent) installing build toolchains.

use crate::error::{InstallError, Result};
use crate::fetch::Fetcher;
use crate::manifest::{CustomToolchain, ToolchainReq};
use crate::platform::{Os, Platform};
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt, Question, Stage};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ToolchainDef {
    pub name: String,
    /// Any of these on PATH means "installed".
    pub binaries: Vec<String>,
    /// Command + args printing a version string.
    pub version_cmd: Option<(String, Vec<String>)>,
    pub install: InstallRecipe,
}

#[derive(Debug, Clone)]
pub enum InstallRecipe {
    /// Rust via rustup (no root needed).
    Rustup,
    /// Package manager ids: winget id, brew formula, and Linux packages per
    /// manager.
    Packages(PkgIds),
    /// Shell script per OS, from a manifest-defined toolchain.
    Custom(BTreeMap<String, String>),
    None,
}

#[derive(Debug, Clone, Default)]
pub struct PkgIds {
    pub winget: Option<&'static str>,
    pub winget_override: Option<&'static str>,
    pub brew: Option<&'static str>,
    pub apt: Option<&'static str>,
    pub dnf: Option<&'static str>,
    pub pacman: Option<&'static str>,
    pub zypper: Option<&'static str>,
    pub apk: Option<&'static str>,
    /// macOS fallback when brew is missing (e.g. xcode-select).
    pub macos_fallback: Option<&'static str>,
}

macro_rules! pkgs {
    ($($k:ident : $v:expr),* $(,)?) => {
        InstallRecipe::Packages(PkgIds { $($k: Some($v),)* ..Default::default() })
    };
}

pub fn builtin(name: &str) -> Option<ToolchainDef> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let ver = |c: &str, a: &[&str]| Some((c.to_string(), s(a)));
    let def = match name {
        "rust" | "cargo" => ToolchainDef {
            name: "rust".into(),
            binaries: s(&["cargo"]),
            version_cmd: ver("cargo", &["--version"]),
            install: InstallRecipe::Rustup,
        },
        "go" | "golang" => ToolchainDef {
            name: "go".into(),
            binaries: s(&["go"]),
            version_cmd: ver("go", &["version"]),
            install: pkgs!(winget: "GoLang.Go", brew: "go", apt: "golang-go", dnf: "golang", pacman: "go", zypper: "go", apk: "go"),
        },
        "node" | "nodejs" => ToolchainDef {
            name: "node".into(),
            binaries: s(&["node"]),
            version_cmd: ver("node", &["--version"]),
            install: pkgs!(winget: "OpenJS.NodeJS.LTS", brew: "node", apt: "nodejs npm", dnf: "nodejs npm", pacman: "nodejs npm", zypper: "nodejs npm", apk: "nodejs npm"),
        },
        "python" | "python3" => ToolchainDef {
            name: "python".into(),
            binaries: s(&["python3", "python"]),
            version_cmd: ver("python3", &["--version"]),
            install: pkgs!(winget: "Python.Python.3.12", brew: "python", apt: "python3 python3-pip python3-venv", dnf: "python3 python3-pip", pacman: "python python-pip", zypper: "python3 python3-pip", apk: "python3 py3-pip"),
        },
        "cmake" => ToolchainDef {
            name: "cmake".into(),
            binaries: s(&["cmake"]),
            version_cmd: ver("cmake", &["--version"]),
            install: pkgs!(winget: "Kitware.CMake", brew: "cmake", apt: "cmake", dnf: "cmake", pacman: "cmake", zypper: "cmake", apk: "cmake"),
        },
        "git" => ToolchainDef {
            name: "git".into(),
            binaries: s(&["git"]),
            version_cmd: ver("git", &["--version"]),
            install: pkgs!(winget: "Git.Git", brew: "git", apt: "git", dnf: "git", pacman: "git", zypper: "git", apk: "git", macos_fallback: "xcode-select --install"),
        },
        "make" => ToolchainDef {
            name: "make".into(),
            binaries: s(&["make", "gmake", "mingw32-make"]),
            version_cmd: ver("make", &["--version"]),
            install: pkgs!(winget: "GnuWin32.Make", brew: "make", apt: "make", dnf: "make", pacman: "make", zypper: "make", apk: "make", macos_fallback: "xcode-select --install"),
        },
        "ninja" => ToolchainDef {
            name: "ninja".into(),
            binaries: s(&["ninja"]),
            version_cmd: ver("ninja", &["--version"]),
            install: pkgs!(winget: "Ninja-build.Ninja", brew: "ninja", apt: "ninja-build", dnf: "ninja-build", pacman: "ninja", zypper: "ninja", apk: "ninja"),
        },
        "pkg-config" | "pkgconf" => ToolchainDef {
            name: "pkg-config".into(),
            binaries: s(&["pkg-config", "pkgconf"]),
            version_cmd: ver("pkg-config", &["--version"]),
            install: pkgs!(brew: "pkgconf", apt: "pkg-config", dnf: "pkgconf-pkg-config", pacman: "pkgconf", zypper: "pkg-config", apk: "pkgconf"),
        },
        "cc" | "c" | "c++" | "gcc" | "clang" => ToolchainDef {
            name: "cc".into(),
            binaries: s(&["cc", "gcc", "clang", "cl"]),
            version_cmd: ver("cc", &["--version"]),
            install: pkgs!(
                winget: "Microsoft.VisualStudio.2022.BuildTools",
                winget_override: "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended",
                apt: "build-essential", dnf: "gcc gcc-c++ make", pacman: "base-devel", zypper: "gcc gcc-c++ make", apk: "build-base",
                macos_fallback: "xcode-select --install"
            ),
        },
        "zig" => ToolchainDef {
            name: "zig".into(),
            binaries: s(&["zig"]),
            version_cmd: ver("zig", &["version"]),
            install: pkgs!(winget: "zig.zig", brew: "zig", apt: "zig", dnf: "zig", pacman: "zig", zypper: "zig", apk: "zig"),
        },
        "java" | "jdk" => ToolchainDef {
            name: "java".into(),
            binaries: s(&["javac"]),
            version_cmd: ver("javac", &["-version"]),
            install: pkgs!(winget: "EclipseAdoptium.Temurin.21.JDK", brew: "openjdk", apt: "default-jdk", dnf: "java-21-openjdk-devel", pacman: "jdk-openjdk", zypper: "java-21-openjdk-devel", apk: "openjdk21"),
        },
        "dotnet" => ToolchainDef {
            name: "dotnet".into(),
            binaries: s(&["dotnet"]),
            version_cmd: ver("dotnet", &["--version"]),
            install: pkgs!(winget: "Microsoft.DotNet.SDK.8", brew: "dotnet", apt: "dotnet-sdk-8.0", dnf: "dotnet-sdk-8.0", pacman: "dotnet-sdk", zypper: "dotnet-sdk-8.0", apk: "dotnet8-sdk"),
        },
        _ => return None,
    };
    Some(def)
}

fn from_custom(name: &str, c: &CustomToolchain) -> ToolchainDef {
    let version_cmd = c.version_command.as_ref().and_then(|v| {
        let mut parts = v.split_whitespace();
        let cmd = parts.next()?.to_string();
        Some((cmd, parts.map(|s| s.to_string()).collect()))
    });
    ToolchainDef {
        name: name.to_string(),
        binaries: c.binaries.clone(),
        version_cmd,
        install: if c.install.is_empty() {
            InstallRecipe::None
        } else {
            InstallRecipe::Custom(c.install.clone())
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Installed(Option<semver::Version>),
    TooOld(semver::Version),
    Missing,
}

/// PATH plus directories we learned about during this run (e.g. `~/.cargo/bin`).
pub fn search_path(extra: &[PathBuf]) -> OsString {
    let mut paths: Vec<PathBuf> = extra.to_vec();
    if let Some(p) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&p));
    }
    // Common locations that GUI launches (Finder/Explorer) often miss.
    if let Some(home) = dirs::home_dir() {
        for p in [".cargo/bin", ".local/bin", "go/bin", ".npm-global/bin"] {
            paths.push(home.join(p));
        }
    }
    for p in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/local/go/bin"] {
        paths.push(PathBuf::from(p));
    }
    std::env::join_paths(paths.iter().filter(|p| p.is_dir())).unwrap_or_default()
}

pub fn find_binary(bin: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    which::which_in(bin, Some(search_path(extra)), cwd).ok()
}

pub fn status(def: &ToolchainDef, req: Option<&str>, extra: &[PathBuf]) -> Status {
    let found = def.binaries.iter().find_map(|b| find_binary(b, extra));
    let Some(found_path) = found else {
        return Status::Missing;
    };
    let version = def.version_cmd.as_ref().and_then(|(cmd, args)| {
        // Use the resolved path when the version command is the binary itself.
        let program = if def.binaries.iter().any(|b| b == cmd) {
            found_path.to_string_lossy().to_string()
        } else {
            cmd.clone()
        };
        let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        process::capture(&program, &a).and_then(|out| parse_version(&out))
    });
    if let (Some(req), Some(v)) = (req, &version) {
        if let Ok(r) = semver::VersionReq::parse(req) {
            if !r.matches(v) {
                return Status::TooOld(v.clone());
            }
        }
    }
    Status::Installed(version)
}

pub fn parse_version(text: &str) -> Option<semver::Version> {
    let re = regex::Regex::new(r"(\d+)\.(\d+)(?:\.(\d+))?").ok()?;
    let c = re.captures(text)?;
    Some(semver::Version::new(
        c[1].parse().ok()?,
        c[2].parse().ok()?,
        c.get(3).and_then(|m| m.as_str().parse().ok()).unwrap_or(0),
    ))
}

/// Linux package manager detection.
fn linux_pm() -> Option<&'static str> {
    ["apt-get", "dnf", "pacman", "zypper", "apk"]
        .into_iter()
        .find(|pm| which::which(pm).is_ok())
}

/// Wrap a root-only command for a GUI context: direct when already root,
/// `pkexec` (graphical prompt) when available, otherwise `sudo`.
fn as_root(script: &str) -> Result<String> {
    if Platform::is_elevated() {
        return Ok(script.to_string());
    }
    if which::which("pkexec").is_ok() {
        return Ok(format!("pkexec sh -c {}", shell_quote(script)));
    }
    if which::which("sudo").is_ok() {
        return Ok(format!("sudo sh -c {}", shell_quote(script)));
    }
    Err(InstallError::NeedsElevation)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Produce the shell command that installs a toolchain on this machine.
fn install_script(def: &ToolchainDef, platform: &Platform) -> Result<Option<String>> {
    Ok(match &def.install {
        InstallRecipe::None => None,
        InstallRecipe::Rustup => Some(String::new()), // handled specially
        InstallRecipe::Custom(map) => map
            .get(&platform.key())
            .or_else(|| map.get(platform.os.as_str()))
            .or_else(|| map.get("any"))
            .cloned(),
        InstallRecipe::Packages(PkgIds {
            winget,
            winget_override,
            brew,
            apt,
            dnf,
            pacman,
            zypper,
            apk,
            macos_fallback,
        }) => match platform.os {
            Os::Windows => winget.map(|id| {
                let mut s = format!(
                    "winget install -e --id {id} --silent --accept-source-agreements --accept-package-agreements"
                );
                if let Some(o) = winget_override {
                    s.push_str(&format!(" --override \"{o}\""));
                }
                s
            }),
            Os::Macos => {
                if which::which("brew").is_ok() {
                    brew.map(|f| format!("brew install {f}"))
                } else {
                    macos_fallback.map(|s| s.to_string())
                }
            }
            Os::Linux => {
                let Some(pm) = linux_pm() else {
                    return Ok(None);
                };
                let script = match pm {
                    "apt-get" => apt.map(|p| format!("DEBIAN_FRONTEND=noninteractive apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq {p}")),
                    "dnf" => dnf.map(|p| format!("dnf install -y {p}")),
                    "pacman" => pacman.map(|p| format!("pacman -Sy --noconfirm --needed {p}")),
                    "zypper" => zypper.map(|p| format!("zypper --non-interactive install {p}")),
                    "apk" => apk.map(|p| format!("apk add {p}")),
                    _ => None,
                };
                match script {
                    Some(s) => Some(as_root(&s)?),
                    None => None,
                }
            }
        },
    })
}

/// Make sure every requested toolchain is present, asking before installing.
/// Returns extra PATH directories to use for subsequent commands.
pub fn ensure(
    reqs: &[ToolchainReq],
    custom: &BTreeMap<String, CustomToolchain>,
    platform: &Platform,
    fetcher: &Fetcher,
    work_dir: &std::path::Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>> {
    let mut extra: Vec<PathBuf> = Vec::new();
    let mut missing: Vec<(ToolchainDef, Option<String>)> = Vec::new();

    for req in reqs {
        let def = custom
            .get(req.name())
            .map(|c| from_custom(req.name(), c))
            .or_else(|| builtin(req.name()))
            .ok_or_else(|| {
                InstallError::Manifest(format!(
                    "unknown toolchain {:?}; define it under [toolchains.{}]",
                    req.name(),
                    req.name()
                ))
            })?;
        match status(&def, req.version_req(), &extra) {
            Status::Installed(v) => frontend.log(format!(
                "toolchain {} present{}",
                def.name,
                v.map(|v| format!(" (v{v})")).unwrap_or_default()
            )),
            Status::TooOld(v) => {
                frontend.warn(format!(
                    "toolchain {} is v{v}, but {} is required",
                    def.name,
                    req.version_req().unwrap_or("newer")
                ));
                missing.push((def, req.version_req().map(String::from)));
            }
            Status::Missing => missing.push((def, req.version_req().map(String::from))),
        }
    }
    if missing.is_empty() {
        return Ok(extra);
    }

    let names: Vec<String> = missing.iter().map(|(d, _)| d.name.clone()).collect();
    let mut plan = Vec::new();
    for (def, _) in &missing {
        let script = install_script(def, platform)?;
        match script {
            Some(s) if s.is_empty() => plan.push((def.clone(), "rustup (official Rust installer)".to_string())),
            Some(s) => plan.push((def.clone(), s)),
            None => {
                return Err(InstallError::ToolchainInstall {
                    name: def.name.clone(),
                    reason: format!(
                        "no automatic install method on {}; please install it manually and try again",
                        platform.os.display_name()
                    ),
                });
            }
        }
    }

    let body = format!(
        "This app must be built on your computer and needs: {}.\n\nThe installer can install them now by running:\n{}",
        names.join(", "),
        plan.iter()
            .map(|(_, s)| format!("  • {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let q = Question {
        title: "Install build tools?".into(),
        body,
        accept_label: "Install tools".into(),
        decline_label: "Cancel".into(),
    };
    if !frontend.confirm(&q) {
        return Err(InstallError::Declined(format!("toolchains: {}", names.join(", "))));
    }

    frontend.stage(Stage::InstallingToolchains, names.join(", "));
    for (def, _) in &missing {
        cancel.check()?;
        match &def.install {
            InstallRecipe::Rustup => {
                install_rustup(platform, fetcher, work_dir, frontend, cancel)?;
                if let Some(home) = dirs::home_dir() {
                    extra.push(home.join(".cargo").join("bin"));
                }
            }
            _ => {
                let script = install_script(def, platform)?.unwrap_or_default();
                let env = vec![("PATH".to_string(), search_path(&extra).to_string_lossy().to_string())];
                process::run_shell_checked(&script, work_dir, &env, frontend, cancel, |code| {
                    InstallError::ToolchainInstall {
                        name: def.name.clone(),
                        reason: format!("`{script}` exited with {code:?}"),
                    }
                })?;
            }
        }
        if status(def, None, &extra) == Status::Missing {
            return Err(InstallError::ToolchainInstall {
                name: def.name.clone(),
                reason:
                    "installation finished but the tool is still not found on PATH (a logout/login may be required)"
                        .into(),
            });
        }
    }
    Ok(extra)
}

fn install_rustup(
    platform: &Platform,
    fetcher: &Fetcher,
    work_dir: &std::path::Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<()> {
    match platform.os {
        Os::Windows => {
            let arch = match platform.arch {
                crate::platform::Arch::X86_64 => "x86_64",
                crate::platform::Arch::Aarch64 => "aarch64",
            };
            let url = format!("https://static.rust-lang.org/rustup/dist/{arch}-pc-windows-msvc/rustup-init.exe");
            let exe = work_dir.join("rustup-init.exe");
            fetcher.download(&url, &exe, frontend, cancel)?;
            let mut cmd = process::command(&exe);
            cmd.args(["-y", "--profile", "minimal", "--no-modify-path"]);
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                return Err(InstallError::ToolchainInstall {
                    name: "rust".into(),
                    reason: format!("rustup-init exited with {:?}", st.code()),
                });
            }
        }
        _ => {
            let script = work_dir.join("rustup-init.sh");
            fetcher.download("https://sh.rustup.rs", &script, frontend, cancel)?;
            let mut cmd = process::command("sh");
            cmd.arg(&script)
                .args(["-y", "--profile", "minimal", "--no-modify-path"]);
            let st = process::run_logged(&mut cmd, frontend, cancel)?;
            if !st.success() {
                return Err(InstallError::ToolchainInstall {
                    name: "rust".into(),
                    reason: format!("rustup-init exited with {:?}", st.code()),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(
            parse_version("cargo 1.97.0 (abc 2026-06-30)").unwrap(),
            semver::Version::new(1, 97, 0)
        );
        assert_eq!(
            parse_version("go version go1.23 linux/amd64").unwrap(),
            semver::Version::new(1, 23, 0)
        );
        assert_eq!(parse_version("v22.11.0").unwrap(), semver::Version::new(22, 11, 0));
    }

    #[test]
    fn builtins_exist() {
        for n in ["rust", "go", "node", "python", "cmake", "git", "cc"] {
            assert!(builtin(n).is_some(), "{n}");
        }
        assert!(builtin("nope").is_none());
    }
}
