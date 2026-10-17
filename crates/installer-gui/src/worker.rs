//! Runs engine operations on a background thread and streams events back
//! into the Iced runtime.

use iced::Task;
use iced::futures::channel::{mpsc, oneshot};
use installer_core::{
    CancelToken, Engine, Frontend, InstallError, InstallOptions, InstalledApp, LoadedManifest, Progress, Question,
};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Repair,
    Uninstall,
}

impl Action {
    pub fn gerund(self) -> &'static str {
        match self {
            Action::Install => "Installing",
            Action::Update => "Updating",
            Action::Repair => "Repairing",
            Action::Uninstall => "Removing",
        }
    }
}

/// A friendly + technical description of a failure.
#[derive(Debug, Clone)]
pub struct Failure {
    pub friendly: String,
    pub technical: String,
    pub cancelled: bool,
}

impl From<InstallError> for Failure {
    fn from(e: InstallError) -> Self {
        Failure {
            friendly: e.friendly(),
            technical: format!("{e:#}"),
            cancelled: matches!(e, InstallError::Cancelled),
        }
    }
}

/// Reply channel for a question; shared so the message type stays `Clone`.
pub type Reply = Arc<Mutex<Option<SyncSender<bool>>>>;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Progress(Progress),
    Question(Question, Reply),
    Finished(Result<Option<InstalledApp>, Failure>),
}

struct ChannelFrontend {
    tx: mpsc::UnboundedSender<Event>,
}

impl Frontend for ChannelFrontend {
    fn report(&self, progress: Progress) {
        let _ = self.tx.unbounded_send(Event::Progress(progress));
    }

    fn confirm(&self, question: &Question) -> bool {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let reply: Reply = Arc::new(Mutex::new(Some(reply_tx)));
        if self
            .tx
            .unbounded_send(Event::Question(question.clone(), reply))
            .is_err()
        {
            return false;
        }
        reply_rx.recv().unwrap_or(false)
    }
}

/// Start `action` on a worker thread; events arrive through the returned task.
pub fn start<M: Send + 'static>(
    engine: Arc<Engine>,
    manifest: Arc<LoadedManifest>,
    options: InstallOptions,
    action: Action,
    cancel: CancelToken,
    map: impl Fn(Event) -> M + Send + 'static,
) -> Task<M> {
    let (tx, rx) = mpsc::unbounded::<Event>();
    std::thread::Builder::new()
        .name("omnistall-worker".into())
        .spawn(move || {
            let frontend = ChannelFrontend { tx: tx.clone() };
            let result = match action {
                Action::Install | Action::Update => engine.install(&manifest, &options, &frontend, &cancel).map(Some),
                Action::Repair => {
                    let mut o = options.clone();
                    o.force = true;
                    engine.install(&manifest, &o, &frontend, &cancel).map(Some)
                }
                Action::Uninstall => engine
                    .uninstall(&manifest.manifest.app.name, false, &frontend, &cancel)
                    .map(|_| None),
            };
            let _ = tx.unbounded_send(Event::Finished(result.map_err(Failure::from)));
        })
        .expect("spawn worker thread");
    Task::run(rx, map)
}

/// Run a blocking closure on its own thread and deliver the result as a task.
/// A panic inside `f` is reported as `Err(message)` instead of killing the UI.
pub fn blocking<T: Send + 'static, M: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    map: impl Fn(Result<T, String>) -> M + Send + 'static,
) -> Task<M> {
    let (tx, rx) = oneshot::channel::<Result<T, String>>();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|p| {
            p.downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unexpected internal error".to_string())
        });
        let _ = tx.send(result);
    });
    Task::perform(rx, move |r| match r {
        Ok(v) => map(v),
        Err(_) => map(Err("background task was dropped".to_string())),
    })
}
