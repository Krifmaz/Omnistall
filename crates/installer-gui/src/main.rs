//! Omnistall GUI installer.
//!
//! Double-click → window. No console, no command line. The app to install is
//! described by a manifest found by `installer_core::locator` (embedded in the
//! binary, next to it, or in the bundle); if none is found the user is asked
//! for one.

// Hide the console window on Windows release builds so double-clicking the
// .exe opens only the installer window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod screens;
mod style;
mod worker;

fn main() -> iced::Result {
    init_logging();

    // Renderer selection: Iced tries wgpu (GPU) first and falls back to the
    // tiny-skia software renderer automatically when no usable GPU adapter is
    // found. `OMNISTALL_SOFTWARE_RENDER=1` forces the software path for
    // machines where GPU init hangs instead of failing.
    if std::env::var_os("OMNISTALL_SOFTWARE_RENDER").is_some() && std::env::var_os("ICED_BACKEND").is_none() {
        // SAFETY: called before any other thread is spawned.
        unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia") };
    }

    app::run()
}

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("OMNISTALL_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Log to a file in the installer's data dir so "Copy details" and support
    // have something to look at even without a terminal.
    let log_dir = dirs::data_local_dir().map(|d| d.join("omnistall").join("logs"));
    if let Some(dir) = log_dir {
        if std::fs::create_dir_all(&dir).is_ok() {
            let name = format!("installer-{}.log", chrono::Local::now().format("%Y%m%d-%H%M%S"));
            if let Ok(file) = std::fs::File::create(dir.join(name)) {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(filter)
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .try_init();
                return;
            }
        }
    }
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
