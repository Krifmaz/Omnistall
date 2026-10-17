# Manifest reference (schema 1)

A manifest is a TOML file that tells Omnistall how to obtain, verify, install
and integrate one application. Use the `.omni` extension when you distribute
it (double-clickable once Omnistall is on the machine); `.toml` works too.
Validate one with `cargo xtask validate path/to/app.omni`.

Placeholders expanded in URLs, tags, asset names, commands and paths:
`{version}`, `{name}`, `{os}` (`linux|macos|windows`), `{arch}`
(`x86_64|aarch64`), `{platform}` (`linux-x86_64`…), `{triple}`
(`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`…).

## `schema`

```toml
schema = 1
```

## `[app]`

| Key | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Machine id: lowercase letters, digits, `.`, `_`, `-`. Folder and registry key. |
| `display_name` | no | Human name shown in the UI and shortcuts (default: `name`). |
| `version` | yes | Version string; semver recommended. Drives update detection. |
| `description` | no | One or two sentences shown on the welcome screen. |
| `publisher` | no | Shown under the title. |
| `homepage` | no | Clickable link. |
| `license` | no | Informational. |
| `icon` | no | PNG/SVG/ICO. A URL, a path relative to the manifest, or a path inside the unpacked payload. Used in the window, shortcuts and `.desktop` files. |
| `kind` | no | `gui` (default) or `cli`. CLI apps go on PATH and get no shortcuts by default; GUI apps get shortcuts. |

## `[[dependencies]]`

Other manifests to install first (recursively, with cycle detection).

```toml
[[dependencies]]
name = "runtime-x"            # expected app.name of the dependency
manifest = "runtime-x.toml"   # URL, absolute path, or relative to this manifest
version = ">=2.1"             # optional semver requirement on the installed copy
```

Dependencies are installed with menu shortcuts only for GUI apps and never with
Desktop shortcuts. Uninstalling an app that others depend on is refused.

## `[sources.<key>]`

One table per platform. Keys, most specific first:
`linux-x86_64`, `linux-aarch64`, `macos-x86_64`, `macos-aarch64`,
`windows-x86_64`, `windows-aarch64`, then OS-only `linux`, `macos`, `windows`,
then `any`. On Apple Silicon and Windows-on-ARM a `*-x86_64` source is used as
a last resort (emulated; the UI warns).

Common keys for every source:

| Key | Meaning |
| --- | --- |
| `type` | `url`, `github` or `git` (see below). |
| `sha256` | Hex SHA-256 of the downloaded file. Strongly recommended; the UI warns when missing. |
| `format` | Override detection: `zip`, `tar`, `tar.gz`, `tar.xz`, `tar.bz2`, `tar.zst`, `gz`, `deb`, `dmg`, `pkg`, `msi`, `exe`, `appimage`, `raw`. A downloaded `.exe` is treated as a *raw program* unless `format = "exe"` marks it as a setup program. |
| `strip_components` | Drop N leading path components when extracting (like `tar --strip-components`). |
| `build` | `true` if the download is a *source* archive that must be built with `[build]`. |

### `type = "url"`

```toml
[sources.linux-x86_64]
type = "url"
url = "https://example.com/app-{version}-{triple}.tar.gz"   # http(s), file://, or a path
sha256 = "…"
strip_components = 1
```

### `type = "github"`

```toml
[sources.windows-x86_64]
type = "github"
repo = "owner/name"
tag = "v{version}"              # default "v{version}"; "latest" also works
asset = "app-*-windows-x64.zip" # exact name or * ? wildcards; must match exactly one asset
checksum_asset = "SHA256SUMS"   # optional: GNU/BSD sums file or "<asset>.sha256"; used when sha256 is absent
```

`GITHUB_TOKEN` / `GH_TOKEN` in the environment is used for the API call when
present (private repos, rate limits).

### `type = "git"` (always built)

```toml
[sources.any]
type = "git"
url = "https://github.com/owner/name.git"
rev = "v{version}"      # branch, tag or commit; optional
submodules = false
```

## `[build]`

Required when any source builds. Per-OS overrides `[build.linux]`,
`[build.macos]`, `[build.windows]` replace `toolchains`, `commands` and
`outputs` when non-empty and merge `env`.

```toml
[build]
toolchains = ["rust", { name = "cmake", version = ">=3.20" }, "cc"]
env = { CARGO_NET_GIT_FETCH_WITH_CLI = "true" }
commands = ["cargo build --release --locked"]
outputs = [{ path = "target/release/app", install_as = "bin/app" }]
[build.windows]
outputs = [{ path = "target/release/app.exe", install_as = "bin/app.exe" }]
```

Built-in toolchain names: `rust`, `go`, `node`, `python`, `cmake`, `git`,
`make`, `ninja`, `pkg-config`, `cc` (C/C++ compiler), `zig`, `java`,
`dotnet`. Missing ones are installed only after the user agrees, via rustup,
winget, Homebrew, apt, dnf, pacman, zypper or apk (root steps on Linux use
`pkexec` for a graphical password prompt). Unknown tools can be declared:

```toml
[toolchains.bun]
binaries = ["bun"]
version_command = "bun --version"
[toolchains.bun.install]
linux = "curl -fsSL https://bun.sh/install | bash"
macos = "brew install oven-sh/bun/bun"
windows = "powershell -c \"irm bun.sh/install.ps1 | iex\""
```

Commands run through `sh -c` (Unix) or `cmd /C` (Windows) inside the source
directory with `OMNISTALL_APP`, `OMNISTALL_VERSION`, `OMNISTALL_INSTALL_DIR`,
`OMNISTALL_OS`, `OMNISTALL_ARCH` set and PATH extended with
`~/.cargo/bin`, `~/.local/bin`, Homebrew, etc.

## `[install]`

Per-OS overrides `[install.linux]`, `[install.macos]`, `[install.windows]`
override individual keys.

| Key | Meaning |
| --- | --- |
| `bin` | Executables (relative to the payload) to expose: symlinked into `~/.local/bin` (Unix) or their folder added to the user PATH (Windows). Default: `<name>`, `bin/<name>`, `usr/bin/<name>` if present. |
| `executable` | Main program for shortcuts and **Launch** (default: first `bin`). For `.msi`/`.exe`/`.pkg` installs give the final absolute path; `%VAR%`, `$VAR` and `~` are expanded. |
| `app_bundle` | macOS: the `.app` inside the payload/DMG (auto-detected when omitted). Bundles go to `~/Applications`. |
| `add_to_path` | Default `true` for `kind = "cli"`, `false` for `gui`. The user can override in Options. |
| `installer_args` | Windows setup `.exe` arguments, e.g. `["/S"]` (NSIS), `["/VERYSILENT", "/NORESTART"]` (Inno), `["/quiet"]` (Burn). |
| `msi_properties` | Extra `NAME=VALUE` pairs for `msiexec`. Per-user mode (`MSIINSTALLPERUSER=1`) is added automatically when not elevated. |
| `uninstall_command` | Command that removes a native (`.exe`/`.pkg`) install. `.msi` installs are removed with the cached MSI automatically. |
| `files` | Explicit moves inside the payload: `[{ src = "a/b", dst = "bin/b", executable = true }]`. |

## `[[shortcuts]]`

Zero or more. When omitted, a GUI app gets one default shortcut using
`display_name`, `executable` and `icon`. Start Menu / app menu entries are
created when **Add to Start Menu** is on; Desktop shortcuts when **Create a
Desktop shortcut** is on (both on by default).

```toml
[[shortcuts]]
name = "My App"
exec = "bin/myapp"               # relative to the install dir
args = "--gui %U"                # Linux: %U/%F field codes allowed
icon = "share/icons/myapp.png"   # relative to the install dir, else app.icon
comment = "Does things"
categories = ["Graphics", "Utility"]
terminal = false
```

* Linux: `~/.local/share/applications/omnistall-<name>.desktop` (+ Desktop copy marked trusted).
* Windows: `.lnk` in the user Start Menu and Desktop (PowerShell/WScript).
* macOS: `.app` bundles appear in Launchpad automatically; a Desktop alias is created; non-bundle executables get a small wrapper `.app`.

## `[[file_associations]]`

```toml
[[file_associations]]
extension = ".myext"
mime = "application/x-myapp"      # optional, generated when omitted
description = "My App document"
```

* Linux: shared-mime-info XML + `xdg-mime default`, and `MimeType=` in the desktop entry.
* Windows: `HKCU\Software\Classes` ProgID with `shell\open\command` (no admin).
* macOS: comes from the bundle's `Info.plist` (`CFBundleDocumentTypes`); Launch Services is refreshed.

## `[hooks]`

```toml
[hooks]
pre_install = ["echo preparing"]
post_install = ["\"$OMNISTALL_INSTALL_DIR/bin/app\" --init"]
pre_uninstall = []
post_uninstall = []
[hooks.windows]
post_install = ["\"%OMNISTALL_INSTALL_DIR%\\bin\\app.exe\" --init"]
```

Per-OS tables replace the whole list for that event. The installer shows the
exact commands and asks the user before running them (a declined hook aborts
the install). Hooks run with the same `OMNISTALL_*` environment as builds;
install hooks run in the install directory, uninstall hooks in a temp dir.

## Where things end up

| | User scope (default) | System scope (only when already admin/root) |
| --- | --- | --- |
| Windows payload | `%LOCALAPPDATA%\Programs\<Display Name>` | `%ProgramFiles%\<Display Name>` |
| Windows shortcuts | `%APPDATA%\Microsoft\Windows\Start Menu\Programs`, Desktop | `%ProgramData%\…\Start Menu\Programs` |
| macOS bundles | `~/Applications` | `/Applications` |
| macOS CLI payload | `~/Library/Application Support/omnistall/apps/<name>` + `~/.local/bin` | `/usr/local/opt/omnistall/<name>` + `/usr/local/bin` |
| Linux payload | `~/.local/share/omnistall/apps/<name>` + `~/.local/bin` | `/opt/<name>` + `/usr/local/bin` |
| Linux menu / icons | `~/.local/share/applications`, `~/.local/share/icons/hicolor` | `/usr/share/applications`, `/usr/share/icons/hicolor` |
| Registry, cache, logs | `<local data dir>/omnistall/` | `/var/lib/omnistall` (Linux), `%ProgramData%\omnistall` (Windows) |

PATH changes on Unix are written as marked blocks to `~/.profile` and, when
present, `~/.bashrc`, `~/.bash_profile`, `~/.zshrc`, `~/.zprofile` (plus a
fish `conf.d` snippet); on Windows the user `Path` registry value is updated
and a `WM_SETTINGCHANGE` broadcast is sent. Uninstall reverses both, keeping
entries other installed apps still use.
