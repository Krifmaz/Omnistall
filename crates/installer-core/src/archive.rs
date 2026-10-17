//! Payload format detection and archive extraction.
//!
//! Supported: zip, tar, tar.gz, tar.xz, tar.bz2, tar.zst, gz (single file),
//! deb (data.tar.* extracted), plus pass-through formats handled by the OS
//! integration layer: dmg, pkg, msi, exe installers, AppImage, raw binaries.

use crate::error::{InstallError, IoCtx, Result};
use crate::progress::{CancelToken, Frontend, FrontendExt};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarBz2,
    TarZst,
    Gz,
    Deb,
    Dmg,
    Pkg,
    Msi,
    Exe,
    AppImage,
    Raw,
}

impl Format {
    pub fn from_hint(s: &str) -> Option<Format> {
        Some(match s.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
            "zip" => Format::Zip,
            "tar" => Format::Tar,
            "tar.gz" | "tgz" => Format::TarGz,
            "tar.xz" | "txz" => Format::TarXz,
            "tar.bz2" | "tbz2" | "tbz" => Format::TarBz2,
            "tar.zst" | "tzst" => Format::TarZst,
            "gz" => Format::Gz,
            "deb" => Format::Deb,
            "dmg" => Format::Dmg,
            "pkg" => Format::Pkg,
            "msi" => Format::Msi,
            "exe" | "installer" => Format::Exe,
            "appimage" => Format::AppImage,
            "raw" | "binary" | "bin" => Format::Raw,
            _ => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Format> {
        let lower = name.to_ascii_lowercase();
        let table: &[(&str, Format)] = &[
            (".tar.gz", Format::TarGz),
            (".tgz", Format::TarGz),
            (".tar.xz", Format::TarXz),
            (".txz", Format::TarXz),
            (".tar.bz2", Format::TarBz2),
            (".tbz2", Format::TarBz2),
            (".tar.zst", Format::TarZst),
            (".tzst", Format::TarZst),
            (".tar", Format::Tar),
            (".zip", Format::Zip),
            (".deb", Format::Deb),
            (".dmg", Format::Dmg),
            (".pkg", Format::Pkg),
            (".msi", Format::Msi),
            (".appimage", Format::AppImage),
            (".gz", Format::Gz),
        ];
        table.iter().find(|(ext, _)| lower.ends_with(ext)).map(|(_, f)| *f)
    }

    /// Sniff magic bytes. `.exe` is deliberately *not* classified as an
    /// installer here: a Windows CLI binary is also a PE file.
    pub fn from_magic(path: &Path) -> Option<Format> {
        let mut f = File::open(path).ok()?;
        let mut head = [0u8; 8];
        let n = f.read(&mut head).ok()?;
        let head = &head[..n];
        if head.starts_with(b"PK\x03\x04") {
            return Some(Format::Zip);
        }
        if head.starts_with(&[0x1f, 0x8b]) {
            return Some(Format::TarGz);
        }
        if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
            return Some(Format::TarXz);
        }
        if head.starts_with(b"BZh") {
            return Some(Format::TarBz2);
        }
        if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            return Some(Format::TarZst);
        }
        if head.starts_with(b"!<arch>") {
            return Some(Format::Deb);
        }
        if head.starts_with(&[0xd0, 0xcf, 0x11, 0xe0]) {
            return Some(Format::Msi);
        }
        // ustar magic at offset 257
        if f.seek(SeekFrom::Start(257)).is_ok() {
            let mut m = [0u8; 5];
            if f.read_exact(&mut m).is_ok() && &m == b"ustar" {
                return Some(Format::Tar);
            }
        }
        None
    }

    pub fn is_archive(self) -> bool {
        matches!(
            self,
            Format::Zip
                | Format::Tar
                | Format::TarGz
                | Format::TarXz
                | Format::TarBz2
                | Format::TarZst
                | Format::Gz
                | Format::Deb
        )
    }
}

/// Decide the format of a downloaded payload.
pub fn detect(path: &Path, hint: Option<&str>, name_hint: &str) -> Result<Format> {
    if let Some(h) = hint {
        return Format::from_hint(h)
            .ok_or_else(|| InstallError::UnsupportedFormat(format!("unknown format hint {h:?}")));
    }
    if let Some(f) = Format::from_name(name_hint) {
        return Ok(f);
    }
    if let Some(f) = Format::from_magic(path) {
        return Ok(f);
    }
    // No extension and no known magic: a raw executable.
    Ok(Format::Raw)
}

/// Extract `archive` into `dest` (created if needed), dropping the first
/// `strip` path components.
pub fn extract(
    archive: &Path,
    format: Format,
    dest: &Path,
    strip: u32,
    frontend: &dyn Frontend,
    cancel: &CancelToken,
) -> Result<()> {
    std::fs::create_dir_all(dest).at(dest)?;
    let staging = if strip > 0 {
        let s = dest.with_file_name(format!(
            "{}.unstripped",
            dest.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "x".into())
        ));
        let _ = std::fs::remove_dir_all(&s);
        std::fs::create_dir_all(&s).at(&s)?;
        s
    } else {
        dest.to_path_buf()
    };

    frontend.log(format!(
        "extracting {} ({format:?}) -> {}",
        archive.display(),
        dest.display()
    ));
    let size = std::fs::metadata(archive).at(archive)?.len();
    let file = File::open(archive).at(archive)?;
    let counted = ProgressReader::new(BufReader::with_capacity(1 << 20, file), size, frontend, cancel);

    match format {
        Format::Zip => extract_zip(archive, &staging, frontend, cancel)?,
        Format::Tar => untar(counted, &staging)?,
        Format::TarGz => untar(flate2::read::MultiGzDecoder::new(counted), &staging)?,
        Format::TarBz2 => untar(bzip2::read::MultiBzDecoder::new(counted), &staging)?,
        Format::TarZst => {
            let dec = ruzstd::decoding::StreamingDecoder::new(counted)
                .map_err(|e| InstallError::Extract(format!("zstd: {e}")))?;
            untar(dec, &staging)?
        }
        Format::TarXz => {
            let tmp = decompress_xz_to_temp(counted, dest)?;
            untar(BufReader::new(File::open(tmp.path()).at(tmp.path())?), &staging)?
        }
        Format::Gz => {
            // single compressed file: name = archive name minus .gz
            let name = archive
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".into());
            let out = staging.join(name);
            let mut dec = flate2::read::MultiGzDecoder::new(counted);
            let mut f = File::create(&out).at(&out)?;
            std::io::copy(&mut dec, &mut f).at(&out)?;
            make_executable(&out)?;
        }
        Format::Deb => extract_deb(archive, &staging, frontend, cancel)?,
        other => {
            return Err(InstallError::UnsupportedFormat(format!("{other:?} is not an archive")));
        }
    }

    if strip > 0 {
        apply_strip(&staging, strip, dest)?;
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(())
}

fn untar<R: Read>(reader: R, dest: &Path) -> Result<()> {
    let mut ar = tar::Archive::new(reader);
    ar.set_preserve_permissions(true);
    ar.set_preserve_mtime(true);
    ar.set_overwrite(true);
    ar.unpack(dest).map_err(|e| InstallError::Extract(format!("tar: {e}")))
}

fn decompress_xz_to_temp<R: Read>(reader: R, near: &Path) -> Result<tempfile::NamedTempFile> {
    let dir = near.parent().unwrap_or(near);
    let mut tmp = tempfile::Builder::new()
        .prefix(".omnistall-xz-")
        .tempfile_in(dir)
        .at(dir)?;
    let mut br = BufReader::new(reader);
    lzma_rs::xz_decompress(&mut br, &mut tmp).map_err(|e| InstallError::Extract(format!("xz: {e}")))?;
    Ok(tmp)
}

fn extract_zip(archive: &Path, dest: &Path, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
    let file = File::open(archive).at(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| InstallError::Extract(format!("zip: {e}")))?;
    let total = zip.len();
    for i in 0..total {
        cancel.check()?;
        let mut entry = zip
            .by_index(i)
            .map_err(|e| InstallError::Extract(format!("zip entry {i}: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            frontend.warn(format!("skipping unsafe zip path {:?}", entry.name()));
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).at(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        #[cfg_attr(not(unix), allow(unused_variables))]
        let mode = entry.unix_mode();
        #[cfg(unix)]
        if let Some(m) = mode {
            if m & 0o170000 == 0o120000 {
                let mut target = String::new();
                entry.read_to_string(&mut target).at(&out)?;
                let _ = std::fs::remove_file(&out);
                std::os::unix::fs::symlink(&target, &out).at(&out)?;
                continue;
            }
        }
        let mut f = File::create(&out).at(&out)?;
        std::io::copy(&mut entry, &mut f).at(&out)?;
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(m & 0o7777));
        }
        if i % 50 == 0 {
            frontend.bytes(i as u64, Some(total as u64));
        }
    }
    frontend.bytes(total as u64, Some(total as u64));
    Ok(())
}

/// Extract a `.deb`'s `data.tar.*` into `dest` (gives `usr/bin/...` layout).
fn extract_deb(archive: &Path, dest: &Path, frontend: &dyn Frontend, cancel: &CancelToken) -> Result<()> {
    let file = File::open(archive).at(archive)?;
    let mut ar = ar::Archive::new(BufReader::new(file));
    while let Some(entry) = ar.next_entry() {
        cancel.check()?;
        let mut entry = entry.map_err(|e| InstallError::Extract(format!("deb: {e}")))?;
        let id = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if !id.starts_with("data.tar") {
            continue;
        }
        let fmt = Format::from_name(&id).unwrap_or(Format::Tar);
        let tmp_dir = dest.parent().unwrap_or(dest);
        let mut tmp = tempfile::Builder::new()
            .prefix(".omnistall-deb-")
            .tempfile_in(tmp_dir)
            .at(tmp_dir)?;
        std::io::copy(&mut entry, &mut tmp).at(tmp.path())?;
        let size = tmp.as_file().metadata().at(tmp.path())?.len();
        let reopened = File::open(tmp.path()).at(tmp.path())?;
        let counted = ProgressReader::new(BufReader::new(reopened), size, frontend, cancel);
        match fmt {
            Format::TarGz => untar(flate2::read::MultiGzDecoder::new(counted), dest)?,
            Format::TarXz => {
                let t = decompress_xz_to_temp(counted, dest)?;
                untar(BufReader::new(File::open(t.path()).at(t.path())?), dest)?
            }
            Format::TarZst => {
                let dec = ruzstd::decoding::StreamingDecoder::new(counted)
                    .map_err(|e| InstallError::Extract(format!("zstd: {e}")))?;
                untar(dec, dest)?
            }
            Format::TarBz2 => untar(bzip2::read::MultiBzDecoder::new(counted), dest)?,
            _ => untar(counted, dest)?,
        }
        return Ok(());
    }
    Err(InstallError::Extract("deb has no data.tar member".into()))
}

/// Read the `Package:` field from a .deb's control file.
pub fn deb_package_name(archive: &Path) -> Option<String> {
    let file = File::open(archive).ok()?;
    let mut ar = ar::Archive::new(BufReader::new(file));
    while let Some(entry) = ar.next_entry() {
        let mut entry = entry.ok()?;
        let id = String::from_utf8_lossy(entry.header().identifier()).to_string();
        if !id.starts_with("control.tar") {
            continue;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        let reader: Box<dyn Read> = match Format::from_name(&id) {
            Some(Format::TarGz) => Box::new(flate2::read::MultiGzDecoder::new(std::io::Cursor::new(buf))),
            Some(Format::TarXz) => {
                let mut out = Vec::new();
                lzma_rs::xz_decompress(&mut std::io::Cursor::new(buf), &mut out).ok()?;
                Box::new(std::io::Cursor::new(out))
            }
            Some(Format::TarZst) => Box::new(ruzstd::decoding::StreamingDecoder::new(std::io::Cursor::new(buf)).ok()?),
            _ => Box::new(std::io::Cursor::new(buf)),
        };
        let mut tar = tar::Archive::new(reader);
        for e in tar.entries().ok()? {
            let mut e = e.ok()?;
            if e.path().ok()?.file_name().is_some_and(|n| n == "control") {
                let mut text = String::new();
                e.read_to_string(&mut text).ok()?;
                return text
                    .lines()
                    .find_map(|l| l.strip_prefix("Package:"))
                    .map(|s| s.trim().to_string());
            }
        }
    }
    None
}

/// Move everything below depth `strip` in `staging` into `dest`.
fn apply_strip(staging: &Path, strip: u32, dest: &Path) -> Result<()> {
    fn walk(dir: &Path, remaining: u32, dest: &Path) -> Result<()> {
        for entry in std::fs::read_dir(dir).at(dir)? {
            let entry = entry.at(dir)?;
            let path = entry.path();
            if remaining == 0 {
                let target = dest.join(entry.file_name());
                if target.exists() {
                    if target.is_dir() && path.is_dir() {
                        // merge
                        walk(&path, 0, &target)?;
                        continue;
                    }
                    let _ = std::fs::remove_file(&target);
                }
                std::fs::rename(&path, &target).at(&target)?;
            } else if path.is_dir() {
                walk(&path, remaining - 1, dest)?;
            }
            // files above the strip depth are dropped, like tar --strip-components
        }
        Ok(())
    }
    walk(staging, strip, dest)
}

pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).at(path)?;
        let mut perm = meta.permissions();
        perm.set_mode(perm.mode() | 0o755);
        std::fs::set_permissions(path, perm).at(path)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Wraps a reader and reports consumed bytes.
struct ProgressReader<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    last: std::time::Instant,
    frontend: &'a dyn Frontend,
    cancel: &'a CancelToken,
}

impl<'a, R: Read> ProgressReader<'a, R> {
    fn new(inner: R, total: u64, frontend: &'a dyn Frontend, cancel: &'a CancelToken) -> Self {
        Self {
            inner,
            done: 0,
            total,
            last: std::time::Instant::now(),
            frontend,
            cancel,
        }
    }
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        if self.last.elapsed() >= std::time::Duration::from_millis(80) {
            self.frontend.bytes(self.done, Some(self.total));
            self.last = std::time::Instant::now();
        }
        Ok(n)
    }
}

/// Locate the first `*.app` bundle in a directory tree (depth ≤ 2).
pub fn find_app_bundle(dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    for entry in walkdir::WalkDir::new(dir).max_depth(2).into_iter().flatten() {
        let p = entry.path();
        if p.is_dir() && p.extension().is_some_and(|e| e == "app") {
            found.push(p.to_path_buf());
        }
    }
    found.sort_by_key(|p| p.components().count());
    found.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::SilentFrontend;
    use std::io::Write;

    fn make_targz(dir: &Path) -> PathBuf {
        let path = dir.join("pkg.tar.gz");
        let f = File::create(&path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        let data = b"#!/bin/sh\necho hi\n";
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "pkg-1.0/bin/tool", &data[..]).unwrap();
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(3);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "pkg-1.0/README", &b"doc"[..]).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    #[test]
    fn targz_with_strip() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = make_targz(tmp.path());
        let dest = tmp.path().join("out");
        let fe = SilentFrontend::default();
        extract(&archive, Format::TarGz, &dest, 1, &fe, &CancelToken::new()).unwrap();
        assert!(dest.join("bin/tool").is_file());
        assert!(dest.join("README").is_file());
        assert!(!dest.join("pkg-1.0").exists());
    }

    #[test]
    fn zip_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.zip");
        {
            let f = File::create(&path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            z.start_file("top/app", opts).unwrap();
            z.write_all(b"bin").unwrap();
            z.finish().unwrap();
        }
        let dest = tmp.path().join("o");
        extract(
            &path,
            Format::Zip,
            &dest,
            1,
            &SilentFrontend::default(),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read(dest.join("app")).unwrap(), b"bin");
    }

    #[test]
    fn detection() {
        assert_eq!(Format::from_name("x-1.0-linux.tar.xz"), Some(Format::TarXz));
        assert_eq!(Format::from_name("Setup.msi"), Some(Format::Msi));
        assert_eq!(Format::from_name("App.AppImage"), Some(Format::AppImage));
        assert_eq!(Format::from_hint("tgz"), Some(Format::TarGz));
        assert_eq!(Format::from_name("tool"), None);
    }
}
