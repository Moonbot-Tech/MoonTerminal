# macOS / Linux Build Notes

Last updated: 2026-11-12.

This file describes the current public stack: `MoonTerminal` + `Moonbot-Tech/MoonUI`.

## General rules

Public dependencies in `crates/moon-ui-gpui/Cargo.toml` must remain git dependencies:

```toml
gpui = { package = "moon-gpui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui_platform = { package = "moon-gpui-platform", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master", features = ["font-kit", "runtime_shaders"] }
moon-ui = { package = "moon-ui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui-component = { package = "moon-ui-components", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
```

`Cargo.lock` is committed: third-party versions move only by a deliberate commit. MoonUI
stays rolling — CI updates its pin on every run; locally that is `make update-moon-ui`. For
diagnosis check the build stamp logged at startup:

```text
build: moonterminal=<rev>[+dirty] release_base=<tag> moonui=<rev>
```

`moonterminal` is `git rev-parse --short=12` of this repo, with `+dirty` when the worktree is
dirty. `release_base` is the greatest stable `v*` tag reachable from HEAD (`vMAJOR.MINOR` or
`vMAJOR.MINOR.PATCH`). `moonui` is the full git revision of `moon-gpui` in `Cargo.lock`, or
`local:<rev>[+dirty]` taken from a sibling `../MoonUI` checkout when the lock has no such pin.
A field that cannot be read is `unknown`.

The build is reproducible for third-party dependencies at any moment; MoonUI freshness is a separate step.

Local development through a neighbouring `MoonUI` checkout is done only through the ignored
`.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }
moon-ui-components = { path = "../MoonUI/crates/moon-ui-components" }

[patch."https://github.com/Moonbot-Tech/MoonProtoBeta"]
moonproto = { path = "../MoonProtoBeta" }
```

Do not use top-level `paths`: that changes the shape of the dependencies and Cargo already warns that
such an override will become an error.

Check where the dependencies actually came from:

```bash
git rev-parse HEAD
cargo tree -i moon-gpui
cargo tree -i moon-ui
```

## macOS

`crates/moon-ui-gpui/Cargo.toml` enables `gpui_platform`'s `runtime_shaders` feature on every
build. With that feature, `moon-gpui-macos` stitches shader source at build time and compiles it
at runtime; it calls `xcrun metal` only when the feature is off, which this crate never does.
Command Line Tools are enough for the shader step. A full Xcode or the Metal toolchain is not
required to compile.

### Canonical Mac Check

This is the main path for a normal Mac stand. It is the same check as on Linux, and it does not
compile shaders through `xcrun metal`:

```bash
cargo check -p moon-ui-gpui --bin moonterminal
```

For a live check it is better to launch the `.app`, not a bare binary from SSH/CLI: macOS Keychain binds
access to the binary/bundle identity, and a remote CLI session easily hits
`User interaction is not allowed`.

```bash
FEATURES=debug-tools \
./tools/macos/macos-bundle.sh

open -n target/macos/MoonTerminal.app
```

`tools/macos/macos-bundle.sh` defaults `TOOLCHAINS` to `com.apple.dt.toolchain.Metal` before the
build. That variable does not turn `runtime_shaders` off, so the build still does not invoke
`xcrun metal`.

`tools/macos/macos-bundle.sh` does a release build, `.app`, stable bundle id `pro.moonbot.terminal`,
an ad-hoc signature by default and `codesign --verify --deep --strict`.

This is **a local dev bundle, not what goes to Releases**: it is built for the machine's own
architecture, with bundle id `pro.moonbot.terminal` and `LSMinimumSystemVersion 13.0`. The distribution is made by CI —
`.github/scripts/make-dmg.sh` in the `macOS .dmg (Apple silicon)` job: an Apple-silicon-only
`arm64` binary, bundle id `com.moonbot.moonterminal`, minimum macOS 11. The binary is compiled
earlier in the `macOS slice (arm64)` job and arrives at packaging as an artifact. The Intel slice
was dropped because its cross-build does not fit the runner's memory. Before the build, the arm64
job also runs `.github/scripts/free-macos-disk.sh` to remove extra Xcode and simulators. The data
directory is ONE for both bundles: `paths.rs` holds `APP_ID` as a baked-in constant
`com.moonbot.moonterminal` and does not read `CFBundleIdentifier` — only the identity of the bundle
itself diverges, which Keychain and Launch Services see. The local bundle script does not reproduce
the release packaging.

### Fresh Mac Live Smoke

On macOS the data root is `~/Library/Application Support/com.moonbot.moonterminal/`, not the
directory of the executable (`paths::data_dir`, `APP_ID`). `servers.enc` is written in that
root. `settings.toml`, `theme.toml` and `orders.toml` are written under its `cfg/` directory.
On Windows the data root is the executable's directory, and the same `cfg/` layout applies there.

A one-time migration still reads the oldest plaintext `config.toml` from the process working
directory (`legacy_toml_path` is the relative path `config.toml`) and `config.enc` from beside
the executable. Launch from the directory that contains `config.toml`, in a GUI session so
Keychain can prompt. The `cd` below only matters when that file sits next to the bundled binary:

```bash
cd "$HOME/MoonTerminal/target/macos/MoonTerminal.app/Contents/MacOS"
MOON_RENDER_DIAG=1 ./MoonTerminal
```

If macOS shows the Keychain prompt `MoonTerminal wants to access key "moon-terminal"`:

```text
Password: <login password>
Button: Always Allow
```

After that, look in Application Support, not in `Contents/MacOS`:

```text
servers.enc
cfg/settings.toml
cfg/theme.toml
cfg/orders.toml
```

Then the ordinary packaging smoke:

```bash
open -n "$HOME/MoonTerminal/target/macos/MoonTerminal.app"
```

For env-driven debug smoke (`MOON_RENDER_DIAG_OPEN_10_BTC=1`) on rented Macs it is simpler to
launch the binary again from GUI Terminal/`.command`; `launchctl asuser ... setenv` may be forbidden
by the provider. An SSH-run is not a valid Keychain/live smoke: it can fail with
`User interaction is not allowed` even though the `.app` works in the GUI.

### Command Line Tools

Command Line Tools are the normal macOS build. Passing
`--features gpui_platform/runtime_shaders` does not select another shader path: that feature is
already enabled on the `gpui_platform` dependency, and Cargo cannot turn a dependency feature
off from the command line.

A CLT `cargo check -p moon-ui-gpui --bin moonterminal` type-checks the terminal and MoonUI on
the macOS target, including Metal backend types (`RawGpuAccess::Metal`) and the command buffer /
encoder path. It does not link a binary, and it does not cover build-script `xcrun metal`
(that path exists in MoonUI only when `runtime_shaders` is off), `.app` packaging, Keychain
GUI ACL, or a visual check of the chart.

### Fast Remote Dev Loop

To check local unpushed `MoonTerminal` + `MoonUI` on a remote Mac copy both trees
side by side and keep the ignored `.cargo/config.toml`, so the terminal takes the local `../MoonUI`:

```bash
rm -rf "$HOME/MoonTerminal" "$HOME/MoonUI"
tar -xzf /tmp/moon-src-check.tgz -C "$HOME"
cd "$HOME/MoonTerminal"
cargo check -p moon-ui-gpui --bin moonterminal
```

For live smoke, launch the `.app` through a GUI session.

## Linux

Releases carry the terminal for x86_64 Linux as one executable, `MoonTerminal-linux-x86_64`,
beside the `.exe` and the `.dmg`, in every release newer than `v0.66.0`
(the `moon-station-*` files there are a different program, the server-side station). The release
builds it in a bare `ubuntu:22.04` container and starts it on a virtual display before anything
is published. The diagnostic `Linux probe` workflow (`.github/workflows/linux-probe.yml`) does
the same on every pull request and every push to `main`: it builds the terminal in a bare
`ubuntu:22.04` container, runs the tests only a Linux machine can, and starts the result on a
virtual display in bare `ubuntu:22.04`, `ubuntu:24.04` and `debian:13` containers. The package
lists below are the ones those containers install — a contract test
(`the_build_guide_and_the_linux_probe_install_the_same_packages`) fails when the two differ.

What neither of them covers: any distribution outside the Debian/Ubuntu family, a Wayland
session (the virtual display is X11), a real GPU (the container renders through Mesa's
software Vulkan driver), and the terminal's own use of the Secret Service (the run uses
`--fixture`, which configures its core in plaintext). Other distributions need the same
libraries under their own package names.

Known limit of old software rendering, measured on 2026-11-10 with one binary: where no GPU
driver is in use and Vulkan falls back to Mesa's software rasterizer, Mesa 22.3.6 (Debian 12)
and 23.2.1 (Ubuntu 22.04) leave the terminal's window blank, while Mesa 25.0.7 draws it — on
Debian 13, on Ubuntu 24.04, and on Ubuntu 22.04 with only Mesa replaced. That is a virtual
machine or a remote desktop without GPU acceleration. A machine on its real GPU driver takes a
different presentation path, which nobody has tested on those two older releases yet.

### Prebuilt binary

x86_64 only. Linked against Ubuntu 22.04's glibc, so it needs glibc 2.35 or newer — Ubuntu 22.04,
Debian 12, or anything more recent.

1. Download `MoonTerminal-linux-x86_64` from the latest release on the repository's Releases
   page; `v0.66.0` and older carry none. For a commit that is not released yet, a `Linux probe`
   run on `main` (Actions tab)
   carries the same build as the `moonterminal-linux-x86_64` artifact: a zip holding a tarball
   with the file `moonterminal`, which needs a signed-in GitHub account and is kept for 90 days.
   Pick a run whose `Linux probe` and `Linux smoke` jobs passed — those jobs are diagnostic, so
   the run itself shows green even when they fail.
2. Install what the binary needs at run time:

   ```bash
   sudo apt update && sudo apt install -y \
     libfontconfig1 libfreetype6 \
     libwayland-client0 libxkbcommon0 libxkbcommon-x11-0 \
     libx11-6 libxtst6 libxcb1 \
     libdbus-1-3 \
     libvulkan1 mesa-vulkan-drivers \
     gnome-keyring libsecret-tools
   ```

   `mesa-vulkan-drivers` is the Vulkan driver for Intel and AMD graphics; a machine on the
   proprietary NVIDIA driver already has its own. A desktop install usually carries most of this
   list already.
3. Make it executable — a browser download is not — and start it from a terminal inside the
   graphical session:

   ```bash
   chmod +x MoonTerminal-linux-x86_64
   ./MoonTerminal-linux-x86_64
   ```

Data and logs live under `~/.config/com.moonbot.moonterminal/`.

The terminal updates itself in place, as on Windows: when a newer release is published the
header button appears, the download is verified against the release's SHA-256, and the
executable is replaced with a rollback if the new one does not start. Keep the file in a
directory you can write to — the update works beside it, in `.moonterminal-update/`.

### Build from source

Build-time packages for Ubuntu/Debian, plus Rust from [rustup](https://rustup.rs) (the repository's
`rust-toolchain.toml` selects the version):

```bash
sudo apt update && sudo apt install -y \
  ca-certificates curl git build-essential pkg-config \
  libfontconfig-dev libfreetype-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-dev libxtst-dev libxcb1-dev \
  libdbus-1-dev
```

Build:

```bash
cargo check -p moon-ui-gpui --bin moonterminal
cargo build --release -p moon-ui-gpui --bin moonterminal
```

The binary is `target/release/moonterminal`. Add `--features debug-tools` for the perf smoke
further down. Running it needs the run-time list from the previous section as well: Vulkan is
opened at run time, so a build succeeds on a machine with no Vulkan driver at all.

### Secret Service

Encrypted config on Linux needs a Secret Service backend in the same GUI/DBus session where the
terminal is launched. Without it there will be an error of the form:

```text
keyring get: Platform secure storage failure:
DBus error: The name org.freedesktop.secrets was not provided by any .service files
```

For a headless/Xvfb test session — `.github/scripts/linux-gui-smoke.sh` is the maintained form of
these lines, run by the probe:

```bash
eval "$(dbus-launch --sh-syntax)"
printf '%s\n' 'Moon' | gnome-keyring-daemon --unlock --components=secrets
eval "$(gnome-keyring-daemon --start --components=secrets)"
secret-tool store --label=moonterminal-test service moon-terminal-test key ping
secret-tool lookup service moon-terminal-test key ping
openbox --sm-disable &
```

If `secret-tool store` raises `org.gnome.keyring.SystemPrompter` or hangs on a GUI password
prompt, the current user keyring is not unlocked with this password. For a one-off
test VPS where old secrets are not needed, it is simpler to reset the user keyring and create a new one:

```bash
mv ~/.local/share/keyrings ~/.local/share/keyrings.bak.$(date +%s) 2>/dev/null || true
mkdir -p ~/.local/share/keyrings
chmod 700 ~/.local/share/keyrings

dbus-run-session -- bash -lc '
  set -euo pipefail
  export DISPLAY=:1
  export XAUTHORITY="$HOME/.Xauthority"
  printf "%s\n" Moon | gnome-keyring-daemon --unlock --components=secrets
  eval "$(gnome-keyring-daemon --start --components=secrets)"
  printf secret | secret-tool store --label=moon-test service moon-terminal-test key ping
  secret-tool lookup service moon-terminal-test key ping
'
```

On Linux the terminal must show only our window header. X11 check:

```bash
xwininfo -root -tree | grep -i MoonTerminal
```

Check Wayland with a separate live/perf run in a real Wayland session: the code path uses
`gpu_canvas_frame_timer`, but the resulting numbers depend on compositor/driver/session.

## Perf / Smoke

For a cross-platform smoke use release + debug-tools:

```bash
MOON_RENDER_DIAG=1 \
MOON_RENDER_DIAG_OPEN_10_BTC=1 \
./target/release/moonterminal
```

Look at `logs/render_diag.log` (next to the application's other logs; the path no longer depends on
the working directory). Good signs:

```text
orders_render and shell_render do not jump to monitor/mouse rate
chart_present is active during live scroll / cursor movement
chart_input_notify stays near zero during pure mousemove
```

For runtime regressions use the built-in FireTest: `docs/FIRETEST.md`.
Keep actual stand perf results in issue/PR/check artifacts, not in this build guide.
