//! Error type shared by the whole engine.
//!
//! Every error carries two faces:
//! * `Display` – the technical message (for logs / "Copy details").
//! * [`InstallError::friendly`] – a plain-language explanation a non-technical
//!   user can act on.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no download or build recipe matches this computer ({platform})")]
    NoMatchingSource { platform: String },

    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("download failed for {url}: {reason}")]
    Download { url: String, reason: String },

    #[error("manifest error: {0}")]
    Manifest(String),

    #[error("manifest could not be loaded from {source_desc}: {reason}")]
    ManifestLoad { source_desc: String, reason: String },

    #[error("unsupported file format: {0}")]
    UnsupportedFormat(String),

    #[error("archive extraction failed: {0}")]
    Extract(String),

    #[error("missing toolchain(s): {0:?}")]
    MissingToolchains(Vec<String>),

    #[error("toolchain installation failed for {name}: {reason}")]
    ToolchainInstall { name: String, reason: String },

    #[error("build step failed (exit {code:?}): {command}")]
    Build { command: String, code: Option<i32> },

    #[error("hook failed (exit {code:?}): {command}")]
    Hook { command: String, code: Option<i32> },

    #[error("native installer failed (exit {code:?}): {command}")]
    NativeInstaller { command: String, code: Option<i32> },

    #[error("this operation needs administrator rights")]
    NeedsElevation,

    #[error("{0} is not installed")]
    NotInstalled(String),

    #[error("the installation was cancelled")]
    Cancelled,

    #[error("the user declined: {0}")]
    Declined(String),

    #[error("dependency cycle detected involving {0}")]
    DependencyCycle(String),

    #[error("file system error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl InstallError {
    /// Helper to attach a path to an I/O error.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Plain-language explanation shown on the GUI error screen.
    pub fn friendly(&self) -> String {
        match self {
            Self::NoMatchingSource { .. } => {
                "This app does not provide a version for your computer's operating system or processor.".into()
            }
            Self::ChecksumMismatch { .. } => {
                "The downloaded file did not match what the publisher expected. It may be corrupted or tampered with, so the installation was stopped. Please try again later.".into()
            }
            Self::Download { .. } => {
                "The download could not be completed. Check your internet connection and try again.".into()
            }
            Self::Manifest(_) | Self::ManifestLoad { .. } => {
                "The installer could not read the app description it was given. The download page or file may be out of date.".into()
            }
            Self::UnsupportedFormat(_) => {
                "The app was delivered in a file format this installer cannot open on this system.".into()
            }
            Self::Extract(_) => "The app's files could not be unpacked.".into(),
            Self::MissingToolchains(_) | Self::ToolchainInstall { .. } => {
                "Some build tools required to prepare this app are missing or could not be installed.".into()
            }
            Self::Build { .. } => "Building the app from its source code failed.".into(),
            Self::Hook { .. } => "A setup step provided by the app publisher failed.".into(),
            Self::NativeInstaller { .. } => "The app's own installer reported an error.".into(),
            Self::NeedsElevation => {
                "This step needs administrator rights. Try the default per-user installation instead.".into()
            }
            Self::NotInstalled(name) => format!("{name} is not installed on this computer."),
            Self::Cancelled => "The installation was cancelled.".into(),
            Self::Declined(_) => "The installation was stopped because a required step was declined.".into(),
            Self::DependencyCycle(_) => "The app's dependencies reference each other in a loop.".into(),
            Self::Io { .. } => {
                "A file could not be read or written. Make sure you have enough disk space and permission to write to the install location.".into()
            }
            Self::Other(_) => "Something unexpected went wrong.".into(),
        }
    }
}

pub type Result<T, E = InstallError> = std::result::Result<T, E>;

/// Extension trait for attaching a path to `std::io::Result`.
pub trait IoCtx<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoCtx<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|e| InstallError::io(path, e))
    }
}
