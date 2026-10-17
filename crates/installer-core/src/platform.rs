//! Runtime OS / CPU detection and the platform keys used by manifests.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Os::Linux => "Linux",
            Os::Macos => "macOS",
            Os::Windows => "Windows",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    #[serde(rename = "x86_64")]
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
    /// Alternate spellings accepted in manifests / asset names.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Arch::X86_64 => &["x86_64", "amd64", "x64"],
            Arch::Aarch64 => &["aarch64", "arm64"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    /// Detect the platform the installer is running on.
    pub fn current() -> Result<Self, String> {
        let os = match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            other => return Err(format!("unsupported operating system: {other}")),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => Arch::X86_64,
            "aarch64" => Arch::Aarch64,
            other => return Err(format!("unsupported CPU architecture: {other}")),
        };
        Ok(Self { os, arch })
    }

    pub fn new(os: Os, arch: Arch) -> Self {
        Self { os, arch }
    }

    /// Canonical manifest key, e.g. `linux-x86_64`.
    pub fn key(&self) -> String {
        format!("{}-{}", self.os.as_str(), self.arch.as_str())
    }

    /// Rust-style target triple (gnu flavour on Linux). Available to manifests
    /// as the `{triple}` placeholder.
    pub fn triple(&self) -> String {
        match (self.os, self.arch) {
            (Os::Linux, Arch::X86_64) => "x86_64-unknown-linux-gnu",
            (Os::Linux, Arch::Aarch64) => "aarch64-unknown-linux-gnu",
            (Os::Macos, Arch::X86_64) => "x86_64-apple-darwin",
            (Os::Macos, Arch::Aarch64) => "aarch64-apple-darwin",
            (Os::Windows, Arch::X86_64) => "x86_64-pc-windows-msvc",
            (Os::Windows, Arch::Aarch64) => "aarch64-pc-windows-msvc",
        }
        .to_string()
    }

    /// Manifest source keys to try, most specific first.
    ///
    /// Emulated fallbacks (Rosetta 2 on Apple Silicon, x64 emulation on
    /// Windows-on-ARM) are returned last and flagged so the caller can warn.
    pub fn candidate_keys(&self) -> Vec<(String, bool)> {
        let mut keys = vec![
            (self.key(), false),
            (self.os.as_str().to_string(), false),
            ("any".to_string(), false),
        ];
        if self.arch == Arch::Aarch64 && matches!(self.os, Os::Macos | Os::Windows) {
            keys.push((Platform::new(self.os, Arch::X86_64).key(), true));
        }
        keys
    }

    /// Does this platform run with administrator/root privileges right now?
    pub fn is_elevated() -> bool {
        imp::is_elevated()
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.os.display_name(), self.arch.as_str())
    }
}

#[cfg(unix)]
mod imp {
    pub fn is_elevated() -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn is_elevated() -> bool {
        // SAFETY: standard Win32 token query on the current process.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut elevation as *mut _ as *mut _,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            );
            CloseHandle(token);
            ok != 0 && elevation.TokenIsElevated != 0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_stable() {
        let p = Platform::new(Os::Linux, Arch::X86_64);
        assert_eq!(p.key(), "linux-x86_64");
        let keys: Vec<_> = p.candidate_keys().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["linux-x86_64", "linux", "any"]);
    }

    #[test]
    fn apple_silicon_falls_back_to_rosetta() {
        let p = Platform::new(Os::Macos, Arch::Aarch64);
        let keys = p.candidate_keys();
        assert_eq!(keys.last().unwrap(), &("macos-x86_64".to_string(), true));
    }
}
