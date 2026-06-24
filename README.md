<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

Development repository for the Moonbot cross-platform trading terminal.

MoonTerminal is not a finished product yet. This repository is the active desktop terminal
workspace: GPUI shell, MoonUI integration, MoonProto live feed, chart rendering, debug tooling,
and platform work for Windows, macOS, and Linux.

---

## Repository Status

- Active development branch: `main`.
- Runtime/UI dependency: [`Moonbot-Tech/MoonUI`](https://github.com/Moonbot-Tech/MoonUI).
- Protocol/client dependency: [`Moonbot-Tech/MoonProtoBeta`](https://github.com/Moonbot-Tech/MoonProtoBeta).
- `Cargo.lock` is intentionally ignored during this development phase.

The terminal currently tracks rolling Git heads for MoonUI and MoonProtoBeta. A fresh checkout
builds against the current public state of those repositories. Existing checkouts can refresh
their local lock with:

```bash
make update-moon-ui
```

For stabilization or release branches we can switch to pinned revisions/tags; `main` is kept
rolling to make terminal/component/protocol development move together.

---

## Clone

```bash
git clone https://github.com/Moonbot-Tech/MoonTerminal.git
cd MoonTerminal
```

## Windows Build

Requirements:

- Git
- Rust via `rustup`
- Visual Studio 2022 Build Tools with the C++ toolchain and Windows SDK
- Optional: `make`

PowerShell:

```powershell
cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
```

Debug executable:

```text
target\x86_64-pc-windows-msvc\debug\moonterminal.exe
```

## macOS Build

Requirements:

- Xcode or a working Metal toolchain
- Rust via `rustup`

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

For canonical Metal validation see [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md).

## Linux Build

Ubuntu/Debian baseline:

```bash
sudo apt update && sudo apt install -y git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev
```

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

Linux encrypted config uses Secret Service in the user GUI/DBus session. Details:
[docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md).

---

## Common Commands

| Command | Purpose |
|---|---|
| `make run` | build and run debug terminal |
| `make build` | debug build |
| `make release` | release build |
| `make check` | type check |
| `make update-moon-ui` | refresh local ignored `Cargo.lock` for rolling Git dependencies |

The Makefile selects the MSVC target on Windows and the native target on macOS/Linux.

---

## Local MoonUI Development

Keep sibling checkouts:

```text
workspace/
  MoonTerminal/
  MoonUI/
  MoonProtoBeta/
```

Use ignored `MoonTerminal/.cargo/config.toml` for local source replacement:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }

[patch."https://github.com/Moonbot-Tech/MoonProtoBeta"]
moonproto = { path = "../MoonProtoBeta" }
```

Use `[patch]`, not Cargo top-level `paths`: the local checkout must replace the same Git source
without changing dependency shape. Do not commit `.cargo/config.toml`.

---

## FireTest

FireTest is the built-in debug scenario runner for catching expensive UI/chart regressions.

Windows example:

```powershell
Remove-Item -ErrorAction SilentlyContinue firetest.log, render_diag.log, panic.log
.\target\x86_64-pc-windows-msvc\debug\moonterminal.exe --debug-script chart-smoke
Get-Content -Encoding UTF8 firetest.log | Select-Object -Last 40
```

Success requires an explicit `FIRETEST PASS` in `firetest.log`. See
[docs/FIRETEST.md](docs/FIRETEST.md).

---

## Configuration

Servers are configured in the application UI:

```text
Settings -> Connections
```

Runtime config lives next to the executable. Server credentials are stored in encrypted config
using the OS secure storage/keyring where available. Local config files and logs are ignored by
Git.

---

## Structure

```text
crates/
  moon-core      feed, session, market data, config, report storage
  moon-chart     chart view math: time/price scale, pan/zoom, axes, order geometry
  moon-ui-gpui   executable: GPUI shell, panels, windows, chartdx integration
```

Useful docs:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/FIRETEST.md](docs/FIRETEST.md)
- [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md)
- [docs/WINDOWING.md](docs/WINDOWING.md)

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
