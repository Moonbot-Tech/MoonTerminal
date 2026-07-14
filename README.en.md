<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

<p align="center">
  <a href="README.md">Русский</a> · <b>English</b>
</p>

Development repository for the cross-platform trading terminal for the Moonbot kernel.

MoonTerminal is not a finished product yet. This is the active development workspace for the
desktop terminal: GPUI shell, MoonUI integration, MoonProto live feed, chart rendering, debug
tooling, and platform work for Windows, macOS, and Linux.

<p align="center">
  <img src="assets/img/screenshot-main.png" alt="MoonTerminal main window" width="900">
</p>

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
| `make run` | build and run the debug terminal |
| `make build` | debug build |
| `make release` | release build |
| `make check` | type check |
| `make update-moon-ui` | refresh the local ignored `Cargo.lock` for rolling Git dependencies |

The Makefile selects the MSVC target on Windows and the native target on macOS/Linux.

---

## Configuration

Servers are configured in the application UI:

```text
Settings -> Connections
```
Per-core connection setup.

<p align="center">
  <img src="assets/img/screenshot-settings-connections.png" alt="Settings — connections / Moonbot cores" width="640">
</p>

Runtime config lives next to the executable. Server credentials are stored in encrypted config
using the OS secure storage/keyring where available. Local config files and logs are ignored by
Git.

---

Useful docs:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/FIRETEST.md](docs/FIRETEST.md)
- [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md)
- [docs/WINDOWING.md](docs/WINDOWING.md)

---

<p align="center">
  Moonbot / Advanced terminal for cryptocurrency trading / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
