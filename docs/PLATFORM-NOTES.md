# Platform notes and gotchas

## macOS: Gatekeeper, signing, notarization

* **Why a `.app` in a `.dmg`.** Double-clicking a bare Mach-O binary opens
  Terminal and runs it there. Only an application bundle (`Something.app`
  with `Contents/Info.plist` and `Contents/MacOS/<exe>`) opens as a window.
  The `.dmg` is the conventional container; ours contains the bundle and a
  link to `/Applications`, but the installer runs fine straight from the
  mounted image.
* **Quarantine.** Files downloaded by a browser get the
  `com.apple.quarantine` attribute. On first open Gatekeeper checks the
  signature and notarization ticket:
  * *Signed with a Developer ID + notarized + stapled*: opens normally.
  * *Ad-hoc signed or unsigned*: "cannot be opened because the developer
    cannot be verified". The user must right-click → **Open** → **Open**
    once (macOS 15 moved this to *System Settings → Privacy & Security →
    Open Anyway*). The download page explains this.
  * Apps **installed by Omnistall** inherit no quarantine flag from the
    installer, but the downloaded archive content keeps whatever the
    publisher shipped: a notarized app inside a zip/dmg stays notarized.
    Omnistall does not strip quarantine attributes.
* **Apple Silicon requires a signature.** arm64 macOS refuses to execute
  *unsigned* code at all. `lipo` keeps the per-slice ad-hoc signatures the
  Rust linker added, but the workflow re-signs the whole bundle
  (`codesign --force --deep --sign -`) to be safe. With a Developer ID
  certificate it signs with `--options runtime --timestamp` (hardened
  runtime, required for notarization).
* **Notarization.** Requires an Apple Developer Program membership
  ($99/year). The workflow uses `xcrun notarytool submit --wait` with an
  app-specific password and `xcrun stapler staple` so the ticket travels
  with the `.dmg` and works offline. Secrets: `MACOS_CERT_P12_BASE64`,
  `MACOS_CERT_PASSWORD`, `MACOS_SIGN_IDENTITY`, `APPLE_ID`,
  `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`.
* **Rosetta.** On Apple Silicon a manifest without a `macos-aarch64` source
  falls back to `macos-x86_64`; the installer warns. Rosetta 2 may need to
  be installed (`softwareupdate --install-rosetta`); the universal
  installer itself never needs it.
* **`.pkg` installers** often only support `-target /`, which needs admin.
  Omnistall tries the per-user target first, then asks and uses
  `osascript … with administrator privileges` for the standard password
  dialog. `.pkg` has no uninstaller; provide `uninstall_command` or expect
  files to stay.
* **`hdiutil attach`** can prompt for an EULA; `-noautoopen -nobrowse
  -readonly` keep it quiet and off the Desktop. Always detach, even on
  failure (the code does).
* **Launch Services** can lag behind: `lsregister -f <bundle>` makes new
  bundles appear in Spotlight/Launchpad immediately.
* **PATH for CLI tools.** Terminal uses zsh and reads `~/.zprofile`
  (login) / `~/.zshrc`; `.profile` alone is not enough, so both are written.

## Windows: SmartScreen, UAC, code signing

* **SmartScreen** shows "Windows protected your PC" for executables without
  *reputation*. Reputation accrues per file hash and per signing
  certificate:
  * *Unsigned*: warning every time until that exact file is downloaded by
    many users. Each new release starts over.
  * *OV certificate*: reputation accrues on the certificate and carries over
    between releases after a while (weeks).
  * *EV certificate* or **Azure Trusted Signing**: immediate reputation, no
    warning. EV keys live on hardware tokens / HSMs, which complicates CI;
    Azure Trusted Signing (`azure/trusted-signing-action`) is the CI-friendly
    option today.
  The workflow signs with `signtool … /fd SHA256 /tr <RFC3161> /td SHA256`
  when `WINDOWS_CERT_PFX_BASE64` / `WINDOWS_CERT_PASSWORD` exist.
* **Sign after branding.** `xtask brand` appends the manifest to the `.exe`.
  Authenticode hashes the entire file except the certificate table, so
  appending bytes to a signed file *invalidates* the signature. Order:
  build → brand → sign → publish.
* **No UAC by default.** The embedded application manifest requests
  `asInvoker`; everything goes to `%LOCALAPPDATA%`, HKCU and the user Start
  Menu. If an app's own `.msi` needs machine scope, `msiexec` raises UAC
  itself; `.exe` setups do whatever their author decided (prefer "user
  setup" variants, as the VSCodium example does).
* **File names matter.** Heuristics in Windows treat names containing
  `setup`, `install` or `update` as installers and may request elevation
  for *unmanifested* 32-bit programs; our 64-bit, manifested binary is
  unaffected, but keep the manifest.
* **Console window.** `windows_subsystem = "windows"` hides the console.
  Child processes (PowerShell for `.lnk` creation, `msiexec`, builds) are
  started with `CREATE_NO_WINDOW` so nothing flashes.
* **PATH.** The user `Path` is REG_EXPAND_SZ under `HKCU\Environment`.
  After writing, `WM_SETTINGCHANGE` is broadcast; Explorer picks it up, but
  already-open terminals do not.
* **Symlinks** need Developer Mode or admin, so CLI tools are exposed by
  adding their folder to PATH instead of linking into a bin dir.
* **ARM64.** Windows on ARM runs x64 binaries under emulation; the installer
  falls back to `windows-x86_64` sources with a warning. The installer
  itself is built natively for ARM64.
* **Antivirus / Defender** may scan large extractions slowly or quarantine
  unsigned helper binaries; signing helps, and the installer never writes
  to `Program Files` unless elevated.
* **Long paths.** The default 260-character limit still bites some
  archives; the installer uses short per-user roots to stay well below it.

## Linux: distro differences

* **Why AppImage + `.deb`.** An AppImage is one self-contained file that runs
  on any glibc distro after `chmod +x`; the `.deb` integrates with the
  software center on Debian/Ubuntu and registers the desktop entry
  system-wide. Fedora/openSUSE users use the AppImage (an `.rpm` can be
  added with `fpm`/`cargo-generate-rpm` the same way `xtask deb` works).
* **FUSE.** AppImages mount themselves with FUSE 2. Ubuntu 22.04+ ships
  only FUSE 3 by default, so `libfuse2` (`libfuse2t64` on 24.04) may be
  missing; the file manager then shows nothing on double-click. Workarounds:
  install the package, or run with `--appimage-extract-and-run`. (The CI
  job installs `libfuse2` only to run `appimagetool`.)
* **glibc floor.** A binary runs on distros with glibc ≥ the build
  machine's. Building on Ubuntu 22.04 (glibc 2.35) covers Debian 12,
  Ubuntu 22.04+, Fedora 36+, RHEL 9+. musl would remove the floor but
  wgpu/winit stacks are not well supported on musl. Older distros should
  use the AppImage built on the oldest supported base.
* **Display servers.** Wayland (GNOME, KDE Plasma 6, Sway) and X11 are both
  supported by winit; libraries (`libwayland-client`, `libxkbcommon`,
  `libX11`) are loaded with `dlopen` at runtime, so no package dependency
  is needed, but `libxkbcommon` and, on X11, `libxkbcommon-x11` must exist
  (they do on every desktop; the `.deb` declares them as `Depends`, and
  minimal containers may need `apt install libxkbcommon-x11-0`).
  Without a desktop (SSH) the GUI cannot start; set `DISPLAY` or use
  `xvfb-run` for automation.
* **GPU vs software rendering.** wgpu needs Vulkan (or GL) drivers; VMs,
  containers and minimal installs often lack them. Iced falls back to
  tiny-skia automatically; `OMNISTALL_SOFTWARE_RENDER=1` forces it when
  Vulkan initialisation hangs rather than fails (seen with some Nvidia
  proprietary driver / Wayland combinations).
* **Desktop integration.** `.desktop` files go to
  `~/.local/share/applications`; GNOME shows Desktop launchers as untrusted
  until `gio set … metadata::trusted true` (done) and the file is
  executable (done). KDE honours them directly. `update-desktop-database`,
  `update-mime-database` and `xdg-mime` are called when available (from
  `desktop-file-utils`, `shared-mime-info`, `xdg-utils`), and their absence
  is tolerated.
* **PATH.** `~/.local/bin` is on PATH by default on most modern distros
  (via `~/.profile`), but only for login shells and only if it existed at
  login. The installer writes a marked block to `~/.profile` and existing
  `~/.bashrc` / `~/.zshrc` plus a fish snippet; new terminals pick it up.
* **Package managers for toolchains.** apt (Debian/Ubuntu/Mint), dnf
  (Fedora/RHEL), pacman (Arch), zypper (openSUSE), apk (Alpine). Root is
  obtained through `pkexec` (graphical polkit prompt) when the installer is
  not root; `sudo` is a last resort and only works with a terminal.
  Package names differ per distro and are tabled in `toolchain.rs`.
* **`.deb` payloads without root.** When a manifest ships a `.deb` and the
  installer is not root, the `data.tar.*` is unpacked into the per-user
  app directory and `usr/bin/*` is linked into `~/.local/bin`. Post-install
  scripts (`postinst`) are *not* run in that mode; as root, `dpkg -i` is
  used instead.
* **Immutable / sandboxed systems.** Flatpak-only or OSTree-based systems
  (Silverblue, SteamOS) cannot use `dpkg`; the AppImage and per-user paths
  still work. Snap-confined browsers may download into `~/snap/...`; the
  installer does not care where it is launched from.
* **SELinux / noexec.** `/tmp` mounted `noexec` breaks AppImages that
  extract there (`TMPDIR` can be pointed elsewhere). Omnistall keeps its
  temp files under its own data dir, not `/tmp`, for the same reason.
