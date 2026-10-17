//! Progress reporting and user interaction, abstracted away from any UI.
//!
//! The engine never prints or prompts. It talks to a [`Frontend`] that the
//! GUI (or a test harness) implements.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Coarse phases of an installation. The GUI maps these to plain-language
/// status text and a weighted overall progress bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Preparing,
    ResolvingDependencies,
    Downloading,
    Verifying,
    Extracting,
    InstallingToolchains,
    Building,
    Configuring,
    Finishing,
    Removing,
    Done,
}

impl Stage {
    /// Short, non-technical status text.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Preparing => "Preparing…",
            Stage::ResolvingDependencies => "Checking requirements…",
            Stage::Downloading => "Downloading…",
            Stage::Verifying => "Verifying download…",
            Stage::Extracting => "Unpacking…",
            Stage::InstallingToolchains => "Installing build tools…",
            Stage::Building => "Building…",
            Stage::Configuring => "Setting up…",
            Stage::Finishing => "Almost done…",
            Stage::Removing => "Removing…",
            Stage::Done => "Done",
        }
    }

    /// Fraction of the overall bar reached when this stage *starts*.
    pub fn base_fraction(self) -> f32 {
        match self {
            Stage::Preparing => 0.00,
            Stage::ResolvingDependencies => 0.03,
            Stage::Downloading => 0.05,
            Stage::Verifying => 0.65,
            Stage::Extracting => 0.70,
            Stage::InstallingToolchains => 0.70,
            Stage::Building => 0.75,
            Stage::Configuring => 0.88,
            Stage::Finishing => 0.96,
            Stage::Removing => 0.10,
            Stage::Done => 1.00,
        }
    }
}

/// Events emitted by the engine while it works.
#[derive(Debug, Clone)]
pub enum Progress {
    /// A new stage started.
    Stage { stage: Stage, detail: String },
    /// Byte-level progress within the current stage (downloads, extraction).
    Bytes { done: u64, total: Option<u64> },
    /// A technical log line ("Show details" in the GUI).
    Log(String),
    /// A non-fatal warning worth surfacing.
    Warning(String),
}

/// Something the engine must ask the user before continuing.
#[derive(Debug, Clone)]
pub struct Question {
    pub title: String,
    pub body: String,
    pub accept_label: String,
    pub decline_label: String,
}

/// Implemented by the UI layer.
pub trait Frontend: Send + Sync {
    fn report(&self, progress: Progress);

    /// Ask a yes/no question. Blocking is fine: the engine runs on a worker
    /// thread. Return `false` to decline.
    fn confirm(&self, question: &Question) -> bool;
}

/// Shared cancellation flag.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn check(&self) -> crate::error::Result<()> {
        if self.is_cancelled() {
            Err(crate::error::InstallError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// A frontend that accepts everything and discards output. Useful for tests
/// and for unattended "--yes" style automation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SilentFrontend {
    pub accept: bool,
}

impl Frontend for SilentFrontend {
    fn report(&self, _progress: Progress) {}
    fn confirm(&self, _question: &Question) -> bool {
        self.accept
    }
}

/// Convenience wrappers so call sites stay short.
pub(crate) trait FrontendExt {
    fn stage(&self, stage: Stage, detail: impl Into<String>);
    fn log(&self, msg: impl Into<String>);
    fn warn(&self, msg: impl Into<String>);
    fn bytes(&self, done: u64, total: Option<u64>);
}

impl<F: Frontend + ?Sized> FrontendExt for F {
    fn stage(&self, stage: Stage, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!(?stage, %detail);
        self.report(Progress::Stage { stage, detail });
    }
    fn log(&self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::debug!(%msg);
        self.report(Progress::Log(msg));
    }
    fn warn(&self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::warn!(%msg);
        self.report(Progress::Warning(msg));
    }
    fn bytes(&self, done: u64, total: Option<u64>) {
        self.report(Progress::Bytes { done, total });
    }
}
