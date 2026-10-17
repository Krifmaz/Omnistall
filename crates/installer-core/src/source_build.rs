//! Build-from-source: run the manifest's commands and collect outputs.

use crate::error::{InstallError, IoCtx, Result};
use crate::manifest::{BuildSpec, Manifest};
use crate::platform::Platform;
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt, Stage};
use crate::toolchain;
use std::path::{Path, PathBuf};

/// Run `spec.commands` in `src_dir`; return `(absolute_output, install_as)` pairs.
pub fn build(
    manifest: &Manifest,
    spec: &BuildSpec,
    src_dir: &Path,
    install_dir: &Path,
    platform: &Platform,
    extra_paths: &[PathBuf],
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<Vec<(PathBuf, String)>> {
    frontend.stage(Stage::Building, manifest.app.display_name());
    let mut env: Vec<(String, String)> = vec![
        (
            "PATH".into(),
            toolchain::search_path(extra_paths).to_string_lossy().to_string(),
        ),
        ("OMNISTALL_APP".into(), manifest.app.name.clone()),
        ("OMNISTALL_VERSION".into(), manifest.app.version.clone()),
        ("OMNISTALL_INSTALL_DIR".into(), install_dir.display().to_string()),
        ("OMNISTALL_OS".into(), platform.os.as_str().into()),
        ("OMNISTALL_ARCH".into(), platform.arch.as_str().into()),
        ("CARGO_TERM_COLOR".into(), "never".into()),
    ];
    for (k, v) in &spec.env {
        env.push((k.clone(), manifest.expand(v, platform)));
    }

    let total = spec.commands.len();
    for (i, raw) in spec.commands.iter().enumerate() {
        let cmd = manifest.expand(raw, platform);
        frontend.stage(Stage::Building, format!("step {} of {total}", i + 1));
        process::run_shell_checked(&cmd, src_dir, &env, frontend, cancel, |code| InstallError::Build {
            command: cmd.clone(),
            code,
        })?;
    }

    let mut outputs = Vec::new();
    for out in &spec.outputs {
        let rel = manifest.expand(&out.path, platform);
        let abs = src_dir.join(&rel);
        if !abs.exists() {
            return Err(InstallError::Build {
                command: format!("expected build output {rel} was not produced"),
                code: None,
            });
        }
        let install_as = out
            .install_as
            .clone()
            .map(|s| manifest.expand(&s, platform))
            .or_else(|| abs.file_name().map(|n| n.to_string_lossy().to_string()))
            .ok_or_else(|| InstallError::Manifest(format!("build output {rel:?} has no file name")))?;
        outputs.push((abs, install_as));
    }
    if outputs.is_empty() {
        return Err(InstallError::Manifest("[build] has no outputs".into()));
    }
    Ok(outputs)
}

/// Copy build outputs into the install directory.
pub fn place_outputs(outputs: &[(PathBuf, String)], install_dir: &Path) -> Result<()> {
    for (src, rel) in outputs {
        let dst = install_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        if src.is_dir() {
            crate::fsutil::copy_dir(src, &dst)?;
        } else {
            std::fs::copy(src, &dst).at(&dst)?;
            crate::archive::make_executable(&dst)?;
        }
    }
    Ok(())
}
