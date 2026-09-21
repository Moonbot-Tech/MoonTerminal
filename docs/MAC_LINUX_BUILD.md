# macOS / Linux Build Notes

Last updated: 2026-06-24.

This file describes the current public stack: `MoonTerminal` + `Moonbot-Tech/MoonUI`.

## General rules

Public dependencies in `Cargo.toml` must remain git dependencies:

```toml
gpui = { package = "moon-gpui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui_platform = { package = "moon-gpui-platform", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
moon-ui = { package = "moon-ui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
```

`Cargo.lock` is committed: third-party versions move only by a deliberate commit. MoonUI
stays rolling — CI updates its pin on every run; locally that is `make update-moon-ui`. For
diagnosis check the build stamp in the log:

```text
build: moonterminal=<git-sha>[+dirty] moonui=<git-sha|local:git-sha>[+dirty]
```

The build is reproducible for third-party dependencies at any moment; MoonUI freshness is a separate step.

Local development through a neighbouring `MoonUI` checkout is done only through the ignored
`.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }

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

A full Xcode or an installed Metal toolchain is needed, where this works:

```bash
xcode-select -p
xcrun --find metal
```

Command Line Tools alone are not enough: `moon-gpui-macos` compiles GPUI Metal shaders through
`xcrun metal`.

### Canonical Mac Check

This is the main path for a normal Mac stand and release/stabilization. It checks build-script
compilation of GPUI Metal shaders through a real `metal`:

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
cargo check -p moon-ui-gpui --bin moonterminal
```

For a live check it is better to launch the `.app`, not a bare binary from SSH/CLI: macOS Keychain binds
access to the binary/bundle identity, and a remote CLI session easily hits
`User interaction is not allowed`.

```bash
TOOLCHAINS=com.apple.dt.toolchain.Metal \
FEATURES=debug-tools \
./scripts/macos-bundle.sh

open -n target/macos/MoonTerminal.app
```

`scripts/macos-bundle.sh` does a release build, `.app`, stable bundle id `pro.moonbot.terminal`,
an ad-hoc signature by default and `codesign --verify --deep --strict`.

This is **a local dev bundle, not what goes to Releases**: it is built for the machine's own
architecture, with bundle id `pro.moonbot.terminal` and `LSMinimumSystemVersion 13.0`. The distribution is made by CI —
`.github/scripts/make-dmg.sh` in the `macOS .dmg (universal)` job: a universal binary (`arm64` +
`x86_64`, glued with `lipo`), bundle id `com.moonbot.moonterminal`, minimum macOS 11. The
slices themselves are compiled earlier, in the matrix job `macOS slice (arm64|x86_64)`, and arrive at
packaging as artifacts: the runner arrives with 40 GiB of free disk, and two `--release` builds in one
job once left no room for `hdiutil` itself. Before the build each slice also runs
`.github/scripts/free-macos-disk.sh` — it removes extra Xcode and simulators, freeing ~100 GiB. The
data directory is ONE for both bundles: `paths.rs` holds `APP_ID` as a baked-in constant
`com.moonbot.moonterminal` and does not read `CFBundleIdentifier` — only the identity
of the bundle itself diverges, which Keychain and Launch Services see. Nobody repeats a universal build
locally — that is a run of the release workflow.

### Fresh Mac Live Smoke

The first migration of an old `config.toml` reads the file from the current working directory, and the new
`servers.enc/settings.toml` are written next to the executable. Therefore on a fresh Mac the first live-run with
a legacy `config.toml` is done from `Contents/MacOS`, through a GUI session:

```bash
cd "$HOME/MoonTerminal/target/macos/MoonTerminal.app/Contents/MacOS"
MOON_RENDER_DIAG=1 ./MoonTerminal
```

If macOS shows the Keychain prompt `MoonTerminal wants to access key "moon-terminal"`:

```text
Password: <login password>
Button: Always Allow
```

After that these should appear:

```text
servers.enc
settings.toml
theme.toml
orders.toml
```

Then the ordinary packaging smoke:

```bash
open -n "$HOME/MoonTerminal/target/macos/MoonTerminal.app"
```

For env-driven debug smoke (`MOON_RENDER_DIAG_OPEN_10_BTC=1`) on rented Macs it is simpler to
launch the binary again from GUI Terminal/`.command`; `launchctl asuser ... setenv` may be forbidden
by the provider. An SSH-run is not a valid Keychain/live smoke: it can fail with
`User interaction is not allowed` even though the `.app` works in the GUI.

### CLT / Rented Mac Fallback

Some rented Macs give only Command Line Tools without a working `xcrun metal`. Such a stand
is good for checking the Rust/Metal backend code, but does NOT replace the canonical Mac check above.

Fallback commands:

```bash
cargo check -p moon-ui-gpui --bin moonterminal --features gpui_platform/runtime_shaders
cargo build -p moon-ui-gpui --bin moonterminal --features gpui_platform/runtime_shaders
```

What this fallback covers:
- compilation of terminal + MoonUI on the macOS target;
- Metal backend types, `RawGpuAccess::Metal`, command buffer / encoder path;
- linking of macOS dependencies.

What it does NOT cover:
- build-script compilation of GPUI Metal shaders through `xcrun metal`;
- `.app` packaging/codesign;
- Keychain GUI ACL;
- a visual live check of the chart with your eyes.

### Fast Remote Dev Loop

To check local unpushed `MoonTerminal` + `MoonUI` on a remote Mac copy both trees
side by side and keep the ignored `.cargo/config.toml`, so the terminal takes the local `../MoonUI`:

```bash
rm -rf "$HOME/MoonTerminal" "$HOME/MoonUI"
tar -xzf /tmp/moon-src-check.tgz -C "$HOME"
cd "$HOME/MoonTerminal"
cargo check -p moon-ui-gpui --bin moonterminal --features gpui_platform/runtime_shaders
```

If the Mac has a full Metal toolchain, run the canonical command with
`TOOLCHAINS=com.apple.dt.toolchain.Metal` instead of the fallback command, and for live smoke launch the `.app` through a GUI session.

## Linux

Minimum set for Ubuntu/Debian:

```bash
sudo apt update && sudo apt install -y \
  git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev \
  dbus-user-session gnome-keyring libsecret-tools
```

Build:

```bash
cargo check -p moon-ui-gpui --bin moonterminal
cargo build --release -p moon-ui-gpui --bin moonterminal --features debug-tools
```

Encrypted config on Linux needs a Secret Service backend in the same GUI/DBus session where the
terminal is launched. Without it there will be an error of the form:

```text
keyring get: Platform secure storage failure:
DBus error: The name org.freedesktop.secrets was not provided by any .service files
```

For a headless/Xvfb test session:

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
