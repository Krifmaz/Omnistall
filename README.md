# Omnistall

A universal, cross-platform GUI installer written in Rust. One installer
binary can install **any** application; a small TOML **manifest** describes
the app. For the person installing, the experience is one double-click and
one **Install** button.

| Platform | You ship | Runs on |
| --- | --- | --- |
| Windows | one `OmnistallSetup-windows-<arch>.exe` | x86_64, ARM64 (Windows 10+) |
| macOS | `Omnistall-macos-universal.dmg` containing `Omnistall.app` | Intel and Apple Silicon (macOS 11+) |
| Linux | `Omnistall-linux-<arch>.AppImage`, or a `.deb` | x86_64, ARM64 (glibc 2.35+) |

The release workflow also publishes a one-page download site (`site/`) that
detects the visitor's OS and CPU and serves the right file.

## What the installer does

1. Opens a window immediately (no terminal, no command line).
2. Shows the app's name, icon, description and a large **Install** button.
   Sensible defaults are preselected: per-user location, Desktop and
   Start Menu / Launchpad / app-menu shortcuts. An **Options** section
   (collapsed) lets people change the location and shortcuts.
3. Detects the OS and CPU and picks the matching source from the manifest:
   direct URL, GitHub Release asset, or a git repository to build from.
   Prebuilt downloads are preferred; building from source is the fallback
   (or an explicit option).
4. Verifies SHA-256 checksums and stops with a clear message on mismatch.
5. Unpacks `.zip`, `.tar.gz`, `.tar.xz`, `.tar.bz2`, `.tar.zst`, `.deb`
   (as a payload), raw binaries, and runs native installers:
   `.dmg` / `.pkg` (macOS), `.msi` / setup `.exe` (Windows), `.AppImage`
   (Linux).
6. Installs to the right per-user place, never needing admin rights by
   default:
   * Windows: `%LOCALAPPDATA%\Programs\<App>` + Start Menu / Desktop `.lnk`
     + user `PATH`
   * macOS: `~/Applications/<App>.app`, or `~/.local/bin` for CLI tools
   * Linux: `~/.local/share/omnistall/apps/<app>` + `~/.local/bin` symlinks +
     `.desktop` entry + icon + MIME associations
   * System-wide locations are used only when already running as admin/root
     and `scope = system` is requested through the core API.
7. Detects missing build toolchains (Rust, Go, Node, Python, CMake, Git, C
   compiler, …) and offers to install them (rustup / winget / Homebrew /
   apt / dnf / pacman / zypper / apk) after asking.
8. Runs the manifest's pre/post install hooks only after the user agrees.
9. Records everything in a local registry so that re-running the installer
   offers **Update**, **Repair** or **Uninstall**.
10. Shows a progress bar with plain-language status, hides technical logs
    behind **Show details**, offers **Launch** at the end, and on failure
    shows a friendly explanation with **Try again** and **Copy details**.
11. Renders with the GPU (wgpu) and falls back to the tiny-skia software
    renderer automatically, so it opens on machines without GPU support.
12. Keyboard accessible: Tab / arrow keys move focus (visible focus ring),
    Enter activates, Esc cancels/closes. Text sizes are generous.

## Repository layout

```
Cargo.toml                 workspace
crates/installer-core/     the engine (library, no UI code)
crates/installer-gui/      the Iced 0.14 GUI (binary `omnistall`)
crates/xtask/              dev tool: brand installers, build AppDir/.app/.deb
manifests/                 example manifests (CLI, GUI, build-from-source)
site/index.html            auto-detecting download page (GitHub Pages)
assets/                    installer icon (PNG sizes + .ico)
docs/MANIFEST.md           manifest schema reference
docs/PLATFORM-NOTES.md     Gatekeeper, SmartScreen, Linux distro notes
.github/workflows/         ci.yml (fmt/clippy/test) and release.yml
```

### `installer-core` modules

| Module | Purpose |
| --- | --- |
| `manifest` | TOML schema, validation, per-platform source selection, `{version}`-style placeholders |
| `locator` | Finds the manifest a double-clicked installer should install (env var, trailer appended to the binary, sidecar file in the bundle / AppDir / `.deb`) |
| `platform` | OS/CPU detection, emulation fallbacks (Rosetta, Windows-on-ARM), elevation check |
| `paths` | Per-OS install locations for user and system scope |
| `fetch` | HTTP downloads with progress (rustls, no OpenSSL), GitHub Releases API, `git clone`, `file://` sources |
| `checksum` | SHA-256 verification; parses `SHA256SUMS`, BSD and single-hash files |
| `archive` | Format detection by extension and magic bytes; zip/tar.*/deb extraction with `strip_components`; pure-Rust decompressors |
| `toolchain` | Detect installed toolchains and versions; install recipes per package manager; asks first |
| `source_build` | Runs build commands with the right env and PATH; collects outputs |
| `hooks` | Pre/post install & uninstall commands, run only after consent |
| `registry` | `registry.json` of installed apps: files created, PATH entries, native uninstall info, cached manifest |
| `install` | The engine (`Engine`): install / update / repair / uninstall / launch; per-OS integration in `install/{linux,macos,windows}.rs` (shortcuts, PATH, file associations, native installers) |
| `progress` | `Frontend` trait (progress events + yes/no questions), `CancelToken` |
| `error` | `InstallError` with a technical `Display` and a plain-language `friendly()` text |

### `installer-gui` modules

| Module | Purpose |
| --- | --- |
| `main` | Window-only entry point (`windows_subsystem = "windows"`), logging to a file, renderer fallback |
| `app` | State machine: Loading → Welcome / Manage → Progress → Finished / Error, keyboard navigation |
| `screens` | Views for each screen |
| `worker` | Runs engine operations on a thread and streams `Progress` / `Question` events into Iced |
| `style` | Sizes and widget styles (focus ring, cards) |

## Building

```sh
cargo build --release -p installer-gui          # target/release/omnistall(.exe)
cargo test --workspace
cargo xtask validate manifests/ripgrep.toml     # check a manifest
```

Linux build machines need no extra system libraries: X11/Wayland/xkbcommon
are loaded at runtime. The resulting binary depends only on glibc.

## One file type for every OS: `.omni`

A `.omni` file is a manifest. Every Omnistall installer registers itself as
the opener for `.omni` files the first time it runs (per user, no admin), so
from then on a publisher can ship a single `MyApp.omni` for Windows, macOS
and Linux: double-click it and the installer opens for that app. Dropping a
`.omni` file onto an open installer window does the same. The per-OS Setup
files are still needed once, because no operating system will execute a
foreign binary format; they are the bootstrap, `.omni` is the everyday path.

(macOS note: the document type is declared in the bundle, so Finder opens
`.omni` files with Omnistall; if an older copy of the installer is still
around, macOS may pick that one. Drag-and-drop onto the window always works.)

## Making an installer for *your* app

1. Write a manifest (see `docs/MANIFEST.md` and the examples in `manifests/`).
   Save it as `myapp.omni` (or `.toml`; same content).
2. Give it to the installer in one of three ways:
   * **Brand the binary** (Windows `.exe`, Linux AppImage, any ELF):
     `cargo xtask brand --installer omnistall.exe --manifest myapp.omni --icon myapp.png --out MyAppSetup.exe`
     appends the manifest to the file and, for a Windows `.exe`, replaces the
     file icon. Do this *before* code signing.
   * **Sidecar file**: `manifest.toml` next to the executable, inside
     `Omnistall.app/Contents/Resources/`, inside the AppDir, or at
     `/usr/share/omnistall/manifest.toml` for the `.deb`. The `xtask appdir`,
     `deb` and `app-bundle` commands take `--manifest` and place it correctly.
   * **Environment**: `OMNISTALL_MANIFEST=<path or URL>` (handy for testing).
   If none is present the installer shows a screen asking for a manifest
   path or URL.
3. In CI, set the repository variables `OMNISTALL_MANIFEST` (manifest path)
   and optionally `OMNISTALL_ICON` (PNG path, replaces the placeholder arrow
   icon everywhere) and tag a release (`git tag v1.0.0 && git push --tags`). The
   `Release` workflow builds all six targets, packages them, signs them when
   certificates are configured, publishes a GitHub Release with `SHA256SUMS`,
   and deploys the download page to GitHub Pages.

## Packaging details

* **Windows**: a single `.exe` built with `windows_subsystem = "windows"`
  (no console), embedded icon and an `asInvoker` manifest so no UAC prompt
  appears. ARM64 is cross-compiled on the x64 runner.
* **macOS**: both architectures are built, merged with `lipo` into a
  universal binary, wrapped in `Omnistall.app` (`xtask app-bundle`), code
  signed (Developer ID when secrets exist, ad-hoc otherwise), put in a
  `.dmg` next to an `Applications` link, and notarized + stapled when Apple
  credentials are configured. A raw Mach-O would open Terminal; the bundle
  opens a window.
* **Linux**: native x86_64 and ARM64 runners build the binary;
  `xtask appdir` + `appimagetool` produce the AppImage, `xtask deb` +
  `dpkg-deb` the `.deb` (desktop entry, icon, optional manifest).

## Environment variables

| Variable | Effect |
| --- | --- |
| `OMNISTALL_MANIFEST` | Path or URL of the manifest to install |
| `OMNISTALL_NO_REGISTER=1` | Do not register the installer as the `.omni` opener |
| `OMNISTALL_SOFTWARE_RENDER=1` | Force the tiny-skia software renderer |
| `OMNISTALL_THEME=light\|dark` | Override system theme detection |
| `OMNISTALL_LOG=debug` | Log level; logs go to `<data dir>/omnistall/logs/` |
| `GITHUB_TOKEN` | Used for GitHub API calls (private releases / rate limits) |

## Example manifests

* `manifests/ripgrep.toml` – prebuilt CLI from GitHub Releases, checksums
  from the release's `.sha256` files, `git` fallback that builds with Cargo.
* `manifests/vscodium.toml` – GUI app: tarball on Linux (desktop entry,
  icon, MIME type), `.dmg` on macOS, silent per-user Inno Setup `.exe` on
  Windows.
* `manifests/hyperfine.toml` – build-from-source project: git clone, Rust
  toolchain requirement (installed via rustup if missing), post-install hook.

Versions in the examples are pinned; bump `app.version` to track new releases.

## OS-specific gotchas

See `docs/PLATFORM-NOTES.md` for the full discussion. In short:

* **macOS Gatekeeper / notarization** – unsigned or ad-hoc-signed apps are
  blocked on first open ("cannot be opened because the developer cannot be
  verified"); the user must right-click → Open once. Notarized builds open
  normally. Signing requires an Apple Developer ID; notarization uses
  `notarytool`. Apple Silicon refuses *unsigned* binaries entirely, so the
  workflow always applies at least an ad-hoc signature after `lipo`.
* **Windows SmartScreen / code signing** – unsigned `.exe` downloads show
  "Windows protected your PC" until the file builds reputation; an OV
  certificate reduces this over time, an EV certificate (or Azure Trusted
  Signing) removes it immediately. Sign *after* branding, because
  Authenticode hashes the whole file.
* **Linux distro differences** – AppImage needs FUSE 2 on older distros
  (`libfuse2`) or `--appimage-extract-and-run`; glibc versions set the
  minimum supported distro (we build on Ubuntu 22.04 → glibc 2.35);
  Wayland vs X11 is handled at runtime; `.desktop` files on the Desktop
  must be marked trusted on GNOME; package managers differ, which the
  toolchain installer handles per manager.

## License

MIT. See `LICENSE`.
