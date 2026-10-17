//! Pre/post install and uninstall hooks (shell commands from the manifest).

use crate::error::{InstallError, Result};
use crate::manifest::Manifest;
use crate::platform::Platform;
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt, Question};
use std::path::Path;

pub fn run(
    phase: &str,
    commands: &[String],
    manifest: &Manifest,
    platform: &Platform,
    cwd: &Path,
    install_dir: &Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }
    let expanded: Vec<String> = commands.iter().map(|c| manifest.expand(c, platform)).collect();
    let q = Question {
        title: format!("Allow {} setup commands?", manifest.app.display_name()),
        body: format!(
            "The publisher's manifest asks to run these {phase} commands:\n{}",
            expanded
                .iter()
                .map(|c| format!("  • {c}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        accept_label: "Allow".into(),
        decline_label: "Cancel".into(),
    };
    if !frontend.confirm(&q) {
        return Err(InstallError::Declined(format!("{phase} hooks")));
    }
    let env = vec![
        ("OMNISTALL_APP".to_string(), manifest.app.name.clone()),
        ("OMNISTALL_VERSION".to_string(), manifest.app.version.clone()),
        ("OMNISTALL_INSTALL_DIR".to_string(), install_dir.display().to_string()),
        ("OMNISTALL_OS".to_string(), platform.os.as_str().to_string()),
        ("OMNISTALL_ARCH".to_string(), platform.arch.as_str().to_string()),
    ];
    for cmd in &expanded {
        frontend.log(format!("{phase} hook: {cmd}"));
        process::run_shell_checked(cmd, cwd, &env, frontend, cancel, |code| InstallError::Hook {
            command: cmd.clone(),
            code,
        })?;
    }
    Ok(())
}
