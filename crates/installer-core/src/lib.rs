//! # installer-core
//!
//! Manifest-driven, cross-platform application install engine. No UI code:
//! the GUI talks to it through [`Engine`] and the [`Frontend`] trait.
//!
//! Module map:
//! * [`manifest`]  – TOML schema, loading, validation, platform source selection.
//! * [`locator`]   – finds the manifest a double-clicked installer should use.
//! * [`selfreg`]   – registers the installer as the opener for `.omni` files.
//! * [`platform`]  – OS/CPU detection, elevation check.
//! * [`paths`]     – per-OS install locations (user vs. system scope).
//! * [`fetch`]     – HTTP downloads, GitHub Releases, git clone.
//! * [`checksum`]  – SHA-256 verification and checksum-file parsing.
//! * [`archive`]   – zip/tar.*/deb extraction, format detection.
//! * [`toolchain`] – detect / install Rust, Go, Node, Python, CMake, …
//! * [`source_build`] – run build commands and collect outputs.
//! * [`hooks`]     – pre/post install & uninstall commands.
//! * [`registry`]  – local JSON database of installed apps.
//! * [`install`]   – the engine: install, update, repair, uninstall, launch,
//!   plus per-OS integration (shortcuts, PATH, associations, native installers).
//! * [`progress`]  – progress events, questions, cancellation.
//! * [`error`]     – error type with technical and friendly messages.

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

pub mod archive;
pub mod checksum;
pub mod error;
pub mod fetch;
pub mod fsutil;
pub mod hooks;
pub mod install;
pub mod locator;
pub mod manifest;
pub mod options;
pub mod paths;
pub mod platform;
pub mod process;
pub mod progress;
pub mod registry;
pub mod selfreg;
pub mod source_build;
pub mod toolchain;

pub use error::{InstallError, Result};
pub use install::{Engine, InstallStatus, LoadedManifest};
pub use manifest::{Manifest, ManifestOrigin};
pub use options::InstallOptions;
pub use paths::{Layout, Scope};
pub use platform::{Arch, Os, Platform};
pub use progress::{CancelToken, Frontend, Progress, Question, SilentFrontend, Stage};
pub use registry::{InstallMethod, InstalledApp, Registry};

/// Crate version, for display.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
