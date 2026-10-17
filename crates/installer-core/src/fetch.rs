//! Downloads (HTTP, `file://`, local paths), GitHub Releases lookups and
//! git clones.

use crate::error::{InstallError, IoCtx, Result};
use crate::process;
use crate::progress::{CancelToken, Frontend, FrontendExt};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Fetcher {
    #[cfg(feature = "net")]
    agent: ureq::Agent,
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher {
    pub fn new() -> Self {
        #[cfg(feature = "net")]
        {
            let config = ureq::Agent::config_builder()
                .user_agent(concat!("omnistall/", env!("CARGO_PKG_VERSION")))
                .timeout_connect(Some(Duration::from_secs(30)))
                .timeout_recv_response(Some(Duration::from_secs(60)))
                .max_redirects(10)
                .build();
            Self {
                agent: config.new_agent(),
            }
        }
        #[cfg(not(feature = "net"))]
        {
            Self {}
        }
    }

    #[cfg(not(feature = "net"))]
    fn offline(url: &str) -> InstallError {
        InstallError::Download {
            url: url.to_string(),
            reason: "this build has no network support".into(),
        }
    }

    #[cfg(feature = "net")]
    fn github_headers(
        &self,
        req: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
        url: &str,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let mut req = req;
        if url.starts_with("https://api.github.com/") {
            req = req
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Ok(tok) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
                if !tok.is_empty() {
                    req = req.header("Authorization", format!("Bearer {tok}"));
                }
            }
        }
        req
    }

    /// Fetch a small text resource (manifest, checksum file). Also accepts
    /// `file://` URLs and plain filesystem paths.
    pub fn get_text(&self, url: &str) -> Result<String> {
        if let Some(path) = local_path(url) {
            return std::fs::read_to_string(&path).at(path);
        }
        #[cfg(feature = "net")]
        {
            let req = self.github_headers(self.agent.get(url), url);
            let mut resp = req.call().map_err(|e| download_err(url, e))?;
            resp.body_mut()
                .with_config()
                .limit(16 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| download_err(url, e))
        }
        #[cfg(not(feature = "net"))]
        Err(Self::offline(url))
    }

    pub fn get_json<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        let text = self.get_text(url)?;
        serde_json::from_str(&text).map_err(|e| InstallError::Download {
            url: url.to_string(),
            reason: format!("invalid JSON response: {e}"),
        })
    }

    /// Fetch a small binary resource into memory (icons).
    pub fn get_bytes(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        if let Some(path) = local_path(url) {
            return std::fs::read(&path).at(path);
        }
        #[cfg(feature = "net")]
        {
            let mut resp = self.agent.get(url).call().map_err(|e| download_err(url, e))?;
            resp.body_mut()
                .with_config()
                .limit(limit)
                .read_to_vec()
                .map_err(|e| download_err(url, e))
        }
        #[cfg(not(feature = "net"))]
        {
            let _ = limit;
            Err(Self::offline(url))
        }
    }

    /// Download `url` to `dest`, reporting byte progress.
    pub fn download(&self, url: &str, dest: &Path, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        frontend.log(format!("downloading {url}"));
        if let Some(path) = local_path(url) {
            let total = std::fs::metadata(&path).at(&path)?.len();
            let mut src = std::fs::File::open(&path).at(&path)?;
            let mut out = std::fs::File::create(dest).at(dest)?;
            copy_with_progress(&mut src, &mut out, Some(total), frontend, cancel, dest)?;
            return Ok(());
        }
        #[cfg(not(feature = "net"))]
        {
            Err(Self::offline(url))
        }
        #[cfg(feature = "net")]
        {
            let req = self.github_headers(self.agent.get(url), url);
            let resp = req.call().map_err(|e| download_err(url, e))?;
            let (_, body) = resp.into_parts();
            let total = body.content_length();
            let mut reader = body.into_reader();
            let tmp = dest.with_extension("part");
            {
                let mut out = std::fs::File::create(&tmp).at(&tmp)?;
                copy_with_progress(&mut reader, &mut out, total, frontend, cancel, &tmp)?;
                out.flush().at(&tmp)?;
            }
            if let Some(t) = total {
                let got = std::fs::metadata(&tmp).at(&tmp)?.len();
                if got != t {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(InstallError::Download {
                        url: url.to_string(),
                        reason: format!("incomplete download ({got} of {t} bytes)"),
                    });
                }
            }
            std::fs::rename(&tmp, dest).at(dest)?;
            Ok(())
        }
    }

    /// Resolve a GitHub release asset to a concrete download URL.
    pub fn resolve_github_asset(&self, repo: &str, tag: &str, asset_pattern: &str) -> Result<GithubAsset> {
        let url = if tag == "latest" {
            format!("https://api.github.com/repos/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
        };
        let release: GithubRelease = self.get_json(&url)?;
        let matched: Vec<&GithubAsset> = release
            .assets
            .iter()
            .filter(|a| wildcard_match(asset_pattern, &a.name))
            .collect();
        match matched.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(InstallError::Download {
                url,
                reason: format!(
                    "no asset in release {} matches {asset_pattern:?}; available: {}",
                    release.tag_name,
                    release
                        .assets
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
            many => Err(InstallError::Download {
                url,
                reason: format!(
                    "pattern {asset_pattern:?} is ambiguous; matches: {}",
                    many.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
                ),
            }),
        }
    }

    pub fn github_release_assets(&self, repo: &str, tag: &str) -> Result<Vec<GithubAsset>> {
        let url = if tag == "latest" {
            format!("https://api.github.com/repos/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repo}/releases/tags/{tag}")
        };
        let release: GithubRelease = self.get_json(&url)?;
        Ok(release.assets)
    }
}

#[cfg(feature = "net")]
fn download_err(url: &str, e: impl std::fmt::Display) -> InstallError {
    InstallError::Download {
        url: url.to_string(),
        reason: e.to_string(),
    }
}

/// `file:///x/y`, `C:\x`, `/x/y`, `./x` → local path.
pub fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        #[cfg(windows)]
        let rest = rest.trim_start_matches('/');
        return Some(PathBuf::from(rest));
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return None;
    }
    Some(PathBuf::from(url))
}

fn copy_with_progress(
    src: &mut dyn Read,
    dst: &mut dyn Write,
    total: Option<u64>,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
    dst_path: &Path,
) -> Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last_report = std::time::Instant::now();
    frontend.bytes(0, total);
    loop {
        cancel.check()?;
        let n = src.read(&mut buf).map_err(|e| InstallError::Download {
            url: dst_path.display().to_string(),
            reason: format!("read error: {e}"),
        })?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).at(dst_path)?;
        done += n as u64;
        if last_report.elapsed() >= Duration::from_millis(50) {
            frontend.bytes(done, total);
            last_report = std::time::Instant::now();
        }
    }
    frontend.bytes(done, total.or(Some(done)));
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GithubAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// Minimal `*` / `?` glob matching.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    fn rec(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..])),
            (Some('?'), Some(_)) => rec(&p[1..], &t[1..]),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &t[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    rec(&p, &t)
}

/// Clone a git repository (shells out to `git`).
pub fn git_clone(
    url: &str,
    rev: Option<&str>,
    submodules: bool,
    dest: &Path,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<()> {
    if which::which("git").is_err() {
        return Err(InstallError::MissingToolchains(vec!["git".into()]));
    }
    let mut cmd = process::command("git");
    cmd.args(["clone", "--depth", "1"]);
    if submodules {
        cmd.arg("--recurse-submodules").arg("--shallow-submodules");
    }
    if let Some(r) = rev {
        cmd.arg("--branch").arg(r);
    }
    cmd.arg(url).arg(dest);
    let status = process::run_logged(&mut cmd, frontend, cancel)?;
    if status.success() {
        return Ok(());
    }
    // `--branch` only accepts branches/tags; fall back to a full clone + checkout.
    let Some(r) = rev else {
        return Err(InstallError::Build {
            command: format!("git clone {url}"),
            code: status.code(),
        });
    };
    frontend.log("shallow clone failed; retrying with a full clone");
    let _ = std::fs::remove_dir_all(dest);
    let mut cmd = process::command("git");
    cmd.arg("clone");
    if submodules {
        cmd.arg("--recurse-submodules");
    }
    cmd.arg(url).arg(dest);
    let status = process::run_logged(&mut cmd, frontend, cancel)?;
    if !status.success() {
        return Err(InstallError::Build {
            command: format!("git clone {url}"),
            code: status.code(),
        });
    }
    let mut cmd = process::command("git");
    cmd.current_dir(dest).args(["checkout", "--quiet", r]);
    let status = process::run_logged(&mut cmd, frontend, cancel)?;
    if !status.success() {
        return Err(InstallError::Build {
            command: format!("git checkout {r}"),
            code: status.code(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(wildcard_match(
            "rg-*-x86_64-*.tar.gz",
            "rg-14.1.1-x86_64-unknown-linux-musl.tar.gz"
        ));
        assert!(!wildcard_match(
            "rg-*-aarch64-*.tar.gz",
            "rg-14.1.1-x86_64-unknown-linux-musl.tar.gz"
        ));
        assert!(wildcard_match("exact.zip", "exact.zip"));
        assert!(wildcard_match("a?c", "abc"));
    }

    #[test]
    fn local_paths() {
        assert_eq!(local_path("file:///tmp/x").unwrap(), PathBuf::from("/tmp/x"));
        assert!(local_path("https://a/b").is_none());
        assert_eq!(local_path("./rel").unwrap(), PathBuf::from("./rel"));
    }
}
