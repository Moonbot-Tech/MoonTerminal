# macOS / Linux Build Notes

Last updated: 2026-09-25.

This file describes the current public stack: `MoonTerminal` + `Moonbot-Tech/MoonUI`.

## General rules

Public dependencies in `crates/moon-ui-gpui/Cargo.toml` must remain git dependencies:

```toml
gpui = { package = "moon-gpui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
gpui_platform = { package = "moon-gpui-platform", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master", features = ["font-kit", "runtime_shaders"] }
moon-ui = { package = "moon-ui", git = "https://github.com/Moonbot-Tech/MoonUI", branch = "master" }
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
./scripts/macos-bundle.sh

open -n target/macos/MoonTerminal.app
```

`scripts/macos-bundle.sh` defaults `TOOLCHAINS` to `com.apple.dt.toolchain.Metal` before the
build. That variable does not turn `runtime_shaders` off, so the build still does not invoke
`xcrun metal`.

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
