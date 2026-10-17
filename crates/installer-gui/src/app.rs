//! Application state, messages and update logic (Elm style).

use crate::screens;
use crate::style;
use crate::worker::{self, Action, Event, Failure, Reply};
use iced::keyboard::{self, Key, key::Named};
use iced::widget::image;
use iced::{Element, Size, Subscription, Task, Theme, window};
use installer_core::progress::Stage;
use installer_core::{
    CancelToken, Engine, InstallOptions, InstallStatus, InstalledApp, LoadedManifest, ManifestOrigin, Progress,
    Question, Scope, locator,
};
use std::sync::Arc;

pub fn run() -> iced::Result {
    let icon = window::icon::from_file_data(include_bytes!("../../../assets/icon.png"), None).ok();
    let app = iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .window(window::Settings {
            size: Size::new(style::WINDOW_W, style::WINDOW_H),
            min_size: Some(Size::new(560.0, 460.0)),
            icon,
            ..window::Settings::default()
        })
        .centered();
    // Without an explicit theme Iced follows the system light/dark preference.
    match forced_theme() {
        Some(theme) => app.theme(move |_: &App| theme.clone()).run(),
        None => app.run(),
    }
}

/// `OMNISTALL_THEME=light|dark` overrides system theme detection.
fn forced_theme() -> Option<Theme> {
    match std::env::var("OMNISTALL_THEME").ok()?.as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None,
    }
}

/// Which screen is showing.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Loading,
    /// No manifest was found: ask for one.
    PickManifest,
    Welcome,
    /// Already installed: Update / Repair / Uninstall.
    Manage,
    Progress,
    Finished,
    Error,
}

/// Actionable buttons, in keyboard-focus order, for the current screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focusable {
    Install,
    Update,
    Repair,
    Uninstall,
    Options,
    Cancel,
    Launch,
    Close,
    Retry,
    CopyDetails,
    Details,
    Continue,
    Yes,
    No,
    Back,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    Booted(Result<Boot, String>),
    ManifestInput(String),
    LoadManifest,
    ManifestLoaded(Result<Loaded, String>),
    ToggleOptions,
    InstallDirChanged(String),
    DesktopShortcut(bool),
    MenuShortcut(bool),
    AddToPath(bool),
    FromSource(bool),
    Start(Action),
    Worker(Event),
    Answer(bool),
    Cancel,
    Retry,
    Launch,
    Launched(Result<(), String>),
    Close,
    CopyDetails,
    ToggleDetails,
    Key(keyboard::Event),
    Activate(Focusable),
    OpenHomepage,
    Back,
}

#[derive(Debug, Clone)]
pub struct Boot {
    pub engine: Arc<Engine>,
    pub located: Option<Loaded>,
    pub needs_pick: bool,
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub manifest: Arc<LoadedManifest>,
    pub icon: Option<Vec<u8>>,
    pub status: InstallStatus,
    pub default_dir: String,
}

#[derive(Debug, Default, Clone)]
pub struct ProgressState {
    pub stage: Option<Stage>,
    pub detail: String,
    pub fraction: f32,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub logs: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct App {
    pub engine: Option<Arc<Engine>>,
    pub manifest: Option<Arc<LoadedManifest>>,
    pub icon: Option<image::Handle>,
    pub icon_svg: Option<iced::widget::svg::Handle>,
    pub status: Option<InstallStatus>,
    pub screen: Screen,
    pub manifest_input: String,
    pub load_error: Option<String>,
    pub options: InstallOptions,
    pub options_open: bool,
    pub install_dir_text: String,
    pub default_dir: String,
    pub progress: ProgressState,
    pub show_details: bool,
    pub question: Option<(Question, Reply)>,
    pub cancel: CancelToken,
    pub action: Action,
    pub result: Option<InstalledApp>,
    pub failure: Option<Failure>,
    pub focus: usize,
    pub copied: bool,
    pub launch_error: Option<String>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = App {
            engine: None,
            manifest: None,
            icon: None,
            icon_svg: None,
            status: None,
            screen: Screen::Loading,
            manifest_input: String::new(),
            load_error: None,
            options: InstallOptions::default(),
            options_open: false,
            install_dir_text: String::new(),
            default_dir: String::new(),
            progress: ProgressState::default(),
            show_details: false,
            question: None,
            cancel: CancelToken::new(),
            action: Action::Install,
            result: None,
            failure: None,
            focus: 0,
            copied: false,
            launch_error: None,
        };
        let task = worker::blocking(boot_blocking, |r| Message::Booted(r.and_then(|x| x)));
        (app, task)
    }

    fn title(&self) -> String {
        match &self.manifest {
            Some(m) => format!("{} Setup", m.manifest.app.display_name()),
            None => "Omnistall Setup".to_string(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        keyboard::listen().map(Message::Key)
    }

    pub fn app_name(&self) -> String {
        self.manifest
            .as_ref()
            .map(|m| m.manifest.app.display_name().to_string())
            .unwrap_or_else(|| "this app".to_string())
    }

    /// The command a CLI user will type: the first `bin` entry's file name
    /// for this OS, falling back to the app name.
    pub fn command_name(&self) -> String {
        let Some(m) = &self.manifest else {
            return "the command".into();
        };
        let os = self
            .engine
            .as_ref()
            .map(|e| e.platform.os)
            .unwrap_or(installer_core::Os::Linux);
        m.manifest
            .install
            .for_os(os)
            .bin
            .first()
            .and_then(|b| {
                std::path::Path::new(b)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| m.manifest.app.name.clone())
    }

    pub fn is_cli(&self) -> bool {
        self.manifest
            .as_ref()
            .is_some_and(|m| m.manifest.app.kind == installer_core::manifest::AppKind::Cli)
    }

    /// Buttons available on the current screen, in focus order.
    pub fn focusables(&self) -> Vec<Focusable> {
        use Focusable::*;
        match self.screen {
            Screen::Loading => vec![],
            Screen::PickManifest => vec![Continue, Close],
            Screen::Welcome => vec![Install, Options, Close],
            Screen::Manage => {
                let mut v = vec![];
                if let Some(InstallStatus::Installed {
                    update_available: true, ..
                }) = &self.status
                {
                    v.push(Update);
                }
                v.extend([Repair, Uninstall, Close]);
                v
            }
            Screen::Progress => {
                if self.question.is_some() {
                    vec![Yes, No]
                } else {
                    vec![Cancel, Details]
                }
            }
            Screen::Finished => {
                let mut v = vec![];
                if self.result.is_some() && !self.is_cli() {
                    v.push(Launch);
                }
                v.push(Close);
                v
            }
            Screen::Error => vec![Retry, CopyDetails, Details, Back, Close],
        }
    }

    pub fn focused(&self) -> Option<Focusable> {
        let f = self.focusables();
        f.get(self.focus.min(f.len().saturating_sub(1))).copied()
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.focus = 0;
    }

    fn apply_loaded(&mut self, loaded: Loaded) {
        self.icon = None;
        self.icon_svg = None;
        if let Some(bytes) = loaded.icon {
            if bytes.starts_with(b"<") || bytes.windows(4).take(256).any(|w| w == b"<svg") {
                self.icon_svg = Some(iced::widget::svg::Handle::from_memory(bytes));
            } else {
                self.icon = Some(image::Handle::from_bytes(bytes));
            }
        }
        self.manifest = Some(loaded.manifest);
        self.default_dir = loaded.default_dir.clone();
        self.install_dir_text = loaded.default_dir;
        self.options = InstallOptions::default();
        self.options_open = false;
        let installed = matches!(loaded.status, InstallStatus::Installed { .. });
        if let InstallStatus::Installed { app, .. } = &loaded.status {
            // Keep the user's previous choices for repair/update.
            self.options = app.options.clone();
            self.options.force = false;
            if let Some(d) = &app.install_dir {
                self.install_dir_text = d.display().to_string();
            }
        }
        self.status = Some(loaded.status);
        self.set_screen(if installed { Screen::Manage } else { Screen::Welcome });
    }

    fn start(&mut self, action: Action) -> Task<Message> {
        let (Some(engine), Some(manifest)) = (self.engine.clone(), self.manifest.clone()) else {
            return Task::none();
        };
        let mut options = self.options.clone();
        let dir = self.install_dir_text.trim();
        options.install_dir = if dir.is_empty() || dir == self.default_dir {
            None
        } else {
            Some(dir.into())
        };
        self.options = options.clone();
        self.action = action;
        self.progress = ProgressState::default();
        self.question = None;
        self.failure = None;
        self.result = None;
        self.copied = false;
        self.launch_error = None;
        self.cancel = CancelToken::new();
        self.set_screen(Screen::Progress);
        worker::start(engine, manifest, options, action, self.cancel.clone(), Message::Worker)
    }

    fn on_progress(&mut self, p: Progress) {
        match p {
            Progress::Stage { stage, detail } => {
                self.progress.stage = Some(stage);
                self.progress.detail = detail.clone();
                self.progress.fraction = self.progress.fraction.max(stage.base_fraction());
                self.progress.bytes_done = 0;
                self.progress.bytes_total = None;
                self.progress
                    .logs
                    .push(format!("[{}] {}", stage.label().trim_end_matches('…'), detail));
            }
            Progress::Bytes { done, total } => {
                self.progress.bytes_done = done;
                self.progress.bytes_total = total;
                if let Some(stage) = self.progress.stage {
                    let base = stage.base_fraction();
                    let next = next_fraction(stage);
                    let within = match total {
                        Some(t) if t > 0 => (done as f32 / t as f32).min(1.0),
                        _ => 0.0,
                    };
                    self.progress.fraction = self.progress.fraction.max(base + (next - base) * within);
                }
            }
            Progress::Log(line) => self.progress.logs.push(line),
            Progress::Warning(w) => {
                self.progress.logs.push(format!("warning: {w}"));
                self.progress.warnings.push(w);
            }
        }
        if self.progress.logs.len() > 2000 {
            let drop = self.progress.logs.len() - 2000;
            self.progress.logs.drain(..drop);
        }
    }

    /// Everything a support person would want, for "Copy details".
    pub fn details_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "Omnistall {} — {}\n",
            installer_core::VERSION,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        if let Some(e) = &self.engine {
            s.push_str(&format!("Platform: {}\n", e.platform));
        }
        if let Some(m) = &self.manifest {
            s.push_str(&format!(
                "App: {} {} (manifest: {})\n",
                m.manifest.app.display_name(),
                m.manifest.app.version,
                m.origin.describe()
            ));
        }
        s.push_str(&format!("Action: {:?}\n", self.action));
        if let Some(f) = &self.failure {
            s.push_str(&format!("Error: {}\n", f.technical));
        }
        if !self.progress.logs.is_empty() {
            s.push_str("\nLog:\n");
            for l in &self.progress.logs {
                s.push_str(l);
                s.push('\n');
            }
        }
        s
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Booted(Ok(boot)) => {
                self.engine = Some(boot.engine);
                match boot.located {
                    Some(loaded) => {
                        self.apply_loaded(loaded);
                        // Test hook: OMNISTALL_AUTO_INSTALL=1 presses Install at once
                        // (used by headless screenshot tests).
                        if self.screen == Screen::Welcome && std::env::var_os("OMNISTALL_AUTO_INSTALL").is_some() {
                            return self.start(Action::Install);
                        }
                    }
                    None => {
                        self.set_screen(Screen::PickManifest);
                        let _ = boot.needs_pick;
                        return iced::widget::operation::focus("manifest-input");
                    }
                }
                Task::none()
            }
            Message::Booted(Err(e)) => {
                self.failure = Some(Failure {
                    friendly: "The installer could not start.".into(),
                    technical: e,
                    cancelled: false,
                });
                self.set_screen(Screen::Error);
                Task::none()
            }
            Message::ManifestInput(s) => {
                self.manifest_input = s;
                Task::none()
            }
            Message::LoadManifest => {
                let spec = self.manifest_input.trim().to_string();
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                if spec.is_empty() {
                    return Task::none();
                }
                self.load_error = None;
                self.set_screen(Screen::Loading);
                worker::blocking(
                    move || load_spec(&engine, &spec),
                    |r| Message::ManifestLoaded(r.and_then(|x| x)),
                )
            }
            Message::ManifestLoaded(Ok(loaded)) => {
                self.apply_loaded(loaded);
                Task::none()
            }
            Message::ManifestLoaded(Err(e)) => {
                self.load_error = Some(e);
                self.set_screen(Screen::PickManifest);
                iced::widget::operation::focus("manifest-input")
            }
            Message::ToggleOptions => {
                self.options_open = !self.options_open;
                Task::none()
            }
            Message::InstallDirChanged(s) => {
                self.install_dir_text = s;
                Task::none()
            }
            Message::DesktopShortcut(v) => {
                self.options.desktop_shortcut = v;
                Task::none()
            }
            Message::MenuShortcut(v) => {
                self.options.menu_shortcut = v;
                Task::none()
            }
            Message::AddToPath(v) => {
                self.options.add_to_path = Some(v);
                Task::none()
            }
            Message::FromSource(v) => {
                self.options.from_source = v;
                Task::none()
            }
            Message::Start(action) => self.start(action),
            Message::Worker(Event::Progress(p)) => {
                self.on_progress(p);
                Task::none()
            }
            Message::Worker(Event::Question(q, reply)) => {
                self.question = Some((q, reply));
                self.focus = 0;
                Task::none()
            }
            Message::Worker(Event::Finished(result)) => {
                self.question = None;
                match result {
                    Ok(app) => {
                        self.progress.fraction = 1.0;
                        self.result = app;
                        self.set_screen(Screen::Finished);
                    }
                    Err(f) => {
                        if f.cancelled {
                            // Back to where we came from.
                            let installed = matches!(self.status, Some(InstallStatus::Installed { .. }));
                            self.set_screen(if installed { Screen::Manage } else { Screen::Welcome });
                        } else {
                            self.failure = Some(f);
                            self.set_screen(Screen::Error);
                        }
                    }
                }
                // Refresh install status for the Manage screen.
                if let (Some(engine), Some(m)) = (self.engine.clone(), self.manifest.clone()) {
                    if let Ok(st) = engine.status(&m.manifest) {
                        self.status = Some(st);
                    }
                }
                Task::none()
            }
            Message::Answer(yes) => {
                if let Some((_, reply)) = self.question.take() {
                    if let Ok(mut guard) = reply.lock() {
                        if let Some(tx) = guard.take() {
                            let _ = tx.send(yes);
                        }
                    }
                }
                self.focus = 0;
                Task::none()
            }
            Message::Cancel => {
                self.cancel.cancel();
                self.progress.detail = "Cancelling…".into();
                Task::none()
            }
            Message::Retry => {
                if self.manifest.is_some() {
                    self.start(self.action)
                } else {
                    self.set_screen(Screen::PickManifest);
                    Task::none()
                }
            }
            Message::Launch => {
                let (Some(engine), Some(app)) = (self.engine.clone(), self.result.clone()) else {
                    return Task::none();
                };
                worker::blocking(
                    move || engine.launch(&app).map_err(|e| e.to_string()),
                    |r| Message::Launched(r.and_then(|x| x)),
                )
            }
            Message::Launched(Ok(())) => iced::exit(),
            Message::Launched(Err(e)) => {
                self.launch_error = Some(e);
                Task::none()
            }
            Message::Close => iced::exit(),
            Message::CopyDetails => {
                self.copied = true;
                iced::clipboard::write(self.details_text())
            }
            Message::ToggleDetails => {
                self.show_details = !self.show_details;
                Task::none()
            }
            Message::OpenHomepage => {
                if let Some(url) = self.manifest.as_ref().and_then(|m| m.manifest.app.homepage.clone()) {
                    let _ = open::that_detached(url);
                }
                Task::none()
            }
            Message::Back => {
                let installed = matches!(self.status, Some(InstallStatus::Installed { .. }));
                if self.manifest.is_some() {
                    self.set_screen(if installed { Screen::Manage } else { Screen::Welcome });
                } else {
                    self.set_screen(Screen::PickManifest);
                }
                Task::none()
            }
            Message::Activate(f) => self.activate(f),
            Message::Key(keyboard::Event::KeyPressed { key, modifiers, .. }) => self.on_key(key, modifiers),
            Message::Key(_) => Task::none(),
        }
    }

    fn activate(&mut self, f: Focusable) -> Task<Message> {
        match f {
            Focusable::Install => self.update(Message::Start(Action::Install)),
            Focusable::Update => self.update(Message::Start(Action::Update)),
            Focusable::Repair => self.update(Message::Start(Action::Repair)),
            Focusable::Uninstall => self.update(Message::Start(Action::Uninstall)),
            Focusable::Options => self.update(Message::ToggleOptions),
            Focusable::Cancel => self.update(Message::Cancel),
            Focusable::Launch => self.update(Message::Launch),
            Focusable::Close => self.update(Message::Close),
            Focusable::Retry => self.update(Message::Retry),
            Focusable::CopyDetails => self.update(Message::CopyDetails),
            Focusable::Details => self.update(Message::ToggleDetails),
            Focusable::Continue => self.update(Message::LoadManifest),
            Focusable::Yes => self.update(Message::Answer(true)),
            Focusable::No => self.update(Message::Answer(false)),
            Focusable::Back => self.update(Message::Back),
        }
    }

    fn on_key(&mut self, key: Key, modifiers: keyboard::Modifiers) -> Task<Message> {
        let count = self.focusables().len();
        match key {
            Key::Named(Named::Tab) => {
                if count > 0 {
                    self.focus = if modifiers.shift() {
                        (self.focus + count - 1) % count
                    } else {
                        (self.focus + 1) % count
                    };
                }
                Task::none()
            }
            Key::Named(Named::ArrowRight) | Key::Named(Named::ArrowDown) => {
                if count > 0 {
                    self.focus = (self.focus + 1) % count;
                }
                Task::none()
            }
            Key::Named(Named::ArrowLeft) | Key::Named(Named::ArrowUp) => {
                if count > 0 {
                    self.focus = (self.focus + count - 1) % count;
                }
                Task::none()
            }
            Key::Named(Named::Enter) | Key::Named(Named::Space) => match self.focused() {
                Some(f) => self.activate(f),
                None => Task::none(),
            },
            Key::Named(Named::Escape) => match self.screen {
                Screen::Progress if self.question.is_some() => self.update(Message::Answer(false)),
                Screen::Progress => self.update(Message::Cancel),
                Screen::Error => self.update(Message::Back),
                Screen::Loading => Task::none(),
                _ => self.update(Message::Close),
            },
            Key::Character(c) if modifiers.command() && c.as_str() == "q" => iced::exit(),
            _ => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        screens::view(self)
    }
}

fn next_fraction(stage: Stage) -> f32 {
    use Stage::*;
    match stage {
        Preparing => ResolvingDependencies.base_fraction(),
        ResolvingDependencies => Downloading.base_fraction(),
        Downloading => Verifying.base_fraction(),
        Verifying => Extracting.base_fraction(),
        Extracting => Configuring.base_fraction(),
        InstallingToolchains => Building.base_fraction(),
        Building => Configuring.base_fraction(),
        Configuring => Finishing.base_fraction(),
        Finishing => Done.base_fraction(),
        Removing => Done.base_fraction(),
        Done => 1.0,
    }
}

/// Start-up work: create the engine and find the manifest.
fn boot_blocking() -> Result<Boot, String> {
    let engine = Arc::new(Engine::new(Scope::User).map_err(|e| e.to_string())?);
    let located = match locator::discover() {
        None => None,
        Some(l) => {
            let lm = if l.text.is_empty() {
                // URL from the environment: fetch it.
                engine.load_manifest(&l.origin.describe())
            } else {
                engine.parse_manifest(&l.text, l.origin.clone())
            };
            match lm {
                Ok(lm) => Some(finish_load(&engine, lm)?),
                Err(e) => return Err(format!("The bundled app description is invalid: {e}")),
            }
        }
    };
    Ok(Boot {
        needs_pick: located.is_none(),
        engine,
        located,
    })
}

fn load_spec(engine: &Engine, spec: &str) -> Result<Loaded, String> {
    let lm = engine.load_manifest(spec).map_err(|e| e.to_string())?;
    finish_load(engine, lm)
}

fn finish_load(engine: &Engine, lm: LoadedManifest) -> Result<Loaded, String> {
    let icon = engine.load_icon(&lm);
    let status = engine.status(&lm.manifest).map_err(|e| e.to_string())?;
    let default_dir = engine.default_install_dir(&lm.manifest).display().to_string();
    let _ = ManifestOrigin::Embedded; // keep import used on all platforms
    Ok(Loaded {
        manifest: Arc::new(lm),
        icon,
        status,
        default_dir,
    })
}
