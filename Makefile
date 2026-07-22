# MoonTerminal — build the sole `moonterminal` binary (crates/moon-ui-gpui).
#
#   make run            build and run (debug)
#   make build          build (debug)
#   make release        build (release)
#   make check          quick type check
#   make fmt            cargo fmt
#   make clean          clean target
#   make update-moon-ui update the local Cargo.lock to the dependency HEAD revisions
#
# Windows: the target is ALWAYS MSVC (x86_64-pc-windows-msvc), not GNU, as required by GPUI/DirectX/
# chartdx. Run `make` from the "x64 Native Tools Command Prompt for VS 2022" (where vcvars is
# configured); otherwise, the C dependency linker will not find link.exe.
# macOS (Metal) / Linux: use the native target; no separate configuration is required.

PKG := -p moon-ui-gpui --bin moonterminal

# cargo may not be in PATH (common on macOS: it is in ~/.cargo/bin, but Terminal does not
# load ~/.cargo/env). Search PATH first, then fall back to rustup's default path.
CARGO := $(shell command -v cargo 2>/dev/null || echo $(HOME)/.cargo/bin/cargo)

ifeq ($(OS),Windows_NT)
  TARGET := --target x86_64-pc-windows-msvc
  BIN := target\x86_64-pc-windows-msvc\debug\moonterminal.exe
  RELEASE_BIN := target\x86_64-pc-windows-msvc\release\moonterminal.exe
else
  TARGET :=
  BIN := target/debug/moonterminal
  RELEASE_BIN := target/release/moonterminal
endif

# macOS: after building, sign the binary with a STABLE self-signed identity. Otherwise,
# the ad-hoc signature changes with every build, and macOS Keychain (which stores the configuration
# encryption key through the keyring/apple-native crate) asks for the password again on every launch.
# See scripts/macos-sign.sh. This is a no-op on Windows/Linux (which do not have this issue).
ifeq ($(OS),Windows_NT)
  SIGN = @echo ">> codesign: пропуск (Windows)"
else ifeq ($(shell uname -s),Darwin)
  SIGN = ./scripts/macos-sign.sh
else
  SIGN = @true
endif

.PHONY: run build release check fmt clean update-moon-ui update-forks codesign-setup help

help:
	@echo "make run | build | release | check | fmt | clean | codesign-setup | update-moon-ui"
	@echo "bin: $(BIN)"

# macOS: create the self-signed code-signing certificate once (otherwise it will be created
# automatically during the first build). On other operating systems, the script is a no-op.
codesign-setup:
	./scripts/macos-codesign-setup.sh

# run depends on build → it launches the already SIGNED binary (not the freshly built unsigned one from
# `cargo run`), so Keychain does not ask for the password again.
run: build
	$(BIN)

build:
	$(CARGO) build $(PKG) $(TARGET)
	$(SIGN) "$(BIN)"

release:
	$(CARGO) build --release $(PKG) $(TARGET)
	$(SIGN) "$(RELEASE_BIN)"

check:
	$(CARGO) check $(PKG) $(TARGET)

fmt:
	$(CARGO) fmt

clean:
	$(CARGO) clean

# Cargo.lock is local and is not committed. A fresh checkout resolves the current MoonUI master.
# In an existing working copy, this target updates the local lockfile to the dependency HEAD revisions.
update-moon-ui:
	$(CARGO) update
	@echo ">> Локальный Cargo.lock обновлён. Теперь: make build"

# Backward-compatible alias for old local scripts.
update-forks: update-moon-ui
