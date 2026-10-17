//! Running external commands with their output forwarded to the frontend.

use crate::error::{InstallError, Result};
use crate::progress::{CancelToken, Frontend, FrontendExt};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

/// Build a `Command` that never pops up a console window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Run `script` through the platform shell (`sh -c` / `cmd /C`).
pub fn shell(script: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = command("cmd");
        c.arg("/C").arg(script);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = command("sh");
        c.arg("-c").arg(script);
        c
    }
}

/// Run a command, streaming stdout/stderr lines to the frontend log.
pub fn run_logged(cmd: &mut Command, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<ExitStatus> {
    cancel.check()?;
    let desc = describe(cmd);
    frontend.log(format!("$ {desc}"));
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| InstallError::Other(anyhow::anyhow!("could not start `{desc}`: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut handles = Vec::new();
    for stream in [
        stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(|l| l.ok()) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx);

    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(line) => frontend.log(line),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(InstallError::Cancelled);
        }
    }
    for h in handles {
        let _ = h.join();
    }
    let status = child
        .wait()
        .map_err(|e| InstallError::Other(anyhow::anyhow!("waiting for `{desc}`: {e}")))?;
    frontend.log(format!(
        "  -> exit {}",
        status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
    ));
    Ok(status)
}

/// Run a shell script in `cwd` with extra env, failing on non-zero exit.
pub fn run_shell_checked(
    script: &str,
    cwd: &Path,
    env: &[(String, String)],
    frontend: &dyn Frontend,
    cancel: &CancelToken,
    on_fail: impl FnOnce(Option<i32>) -> InstallError,
) -> Result<()> {
    let mut cmd = shell(script);
    cmd.current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = run_logged(&mut cmd, frontend, cancel)?;
    if status.success() {
        Ok(())
    } else {
        Err(on_fail(status.code()))
    }
}

pub fn describe(cmd: &Command) -> String {
    let mut s = cmd.get_program().to_string_lossy().to_string();
    for a in cmd.get_args() {
        let a = a.to_string_lossy();
        if a.contains(' ') {
            s.push_str(&format!(" \"{a}\""));
        } else {
            s.push(' ');
            s.push_str(&a);
        }
    }
    s
}

/// Capture stdout of a command quietly (for version probes).
pub fn capture(program: &str, args: &[&str]) -> Option<String> {
    let out = command(program).args(args).stdin(Stdio::null()).output().ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(text)
}
