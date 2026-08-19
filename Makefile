# MoonTerminal — build the sole `moonterminal` binary (crates/moon-ui-gpui).
#
#   make run            build and run (debug)
#   make build          build (debug)
#   make release        build (release)
#   make check          quick type check
#   make fmt            cargo fmt
#   make clean          clean target
#   make update-moon-ui refresh the MoonUI pin in the committed Cargo.lock
#   make update-moonproto refresh the MoonProto pin (deliberate; its own commit)
#   make update-all    move EVERY dependency, forks included — defeats the freeze
#
# Windows: the target is ALWAYS MSVC (x86_64-pc-windows-msvc), not GNU, as required by GPUI/DirectX/
# chartdx. Run `make` from the "x64 Native Tools Command Prompt for VS 2022" (where vcvars is
# configured); otherwise, the C dependency linker will not find link.exe.
# macOS (Metal) / Linux: use the native target; no separate configuration is required.

PKG := -p moon-ui-gpui --bin moonterminal

# cargo may not be in PATH (common on macOS: it is in ~/.cargo/bin, but Terminal does not
# load ~/.cargo/env). Search PATH first, then fall back to rustup's default path.
CARGO := $(shell command -v cargo 2>/dev/null || echo $(HOME)/.cargo/bin/cargo)
MOONUI ?= ../MoonUI
MOONUI_THEME := $(MOONUI)/crates/moon-ui-components/themes/moon-terminal.toml
TOUR_SNAPSHOT := tools/tour/theme.snapshot.toml

# Windows ships a `python3` STUB in WindowsApps that opens the Store instead of
# running anything, and it shadows the real interpreter on PATH. So the name is
# chosen by platform rather than discovered.
ifeq ($(OS),Windows_NT)
  PYTHON ?= python
else
  PYTHON ?= python3
endif
# tools/ is the import root: the generator is the package tools/tour.
TOUR := PYTHONPATH=tools $(PYTHON) -m tour

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

.PHONY: run build release check fmt clean update-moon-ui update-moonproto update-all update-forks codesign-setup help tour tour-check tour-theme tour-test

help:
	@echo "make run | build | release | check | fmt | clean | codesign-setup"
	@echo "deps: update-moon-ui (MoonUI pin) | update-moonproto (deliberate) | update-all (moves EVERYTHING)"
	@echo "docs: tour (regenerate the user tour) | tour-check (verify it is current) | tour-theme (refresh the palette)"
	@echo "bin: $(BIN)"

# macOS: create the self-signed code-signing certificate once (otherwise it will be created
# automatically during the first build). On other operating systems, the script is a no-op.
codesign-setup:
	./scripts/macos-codesign-setup.sh

# run depends on build → it launches the already SIGNED binary (not the freshly built unsigned one from
# `cargo run`), so Keychain does not ask for the password again.
run: build
	$(BIN)

# `--locked` is deliberately absent from the build targets below: a sibling MoonUI checkout
# patched in through .cargo/config.toml legitimately rewrites the lock, and a locked build would
# refuse it. The lock is tracked, so an unintended re-resolution shows up in `git status` instead.
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

# Cargo.lock is COMMITTED, and third-party versions move only in a deliberate commit. This target
# moves the three MoonUI crates and nothing else — the same refresh CI performs on every run.
update-moon-ui:
	$(CARGO) update -p moon-gpui -p moon-gpui-platform -p moon-ui
	@echo ">> Cargo.lock обновлён (MoonUI). Это отслеживаемый файл: закоммитьте осознанно или git checkout -- Cargo.lock"

# MoonProto moves ONLY here, never automatically and never in CI. Commit the result on its own.
update-moonproto:
	$(CARGO) update -p moonproto
	@echo ">> Cargo.lock обновлён (MoonProto). Закоммитьте отдельным коммитом"

# Moves EVERY dependency including the pinned third-party forks — i.e. defeats the freeze this
# repository relies on. Deliberate, occasional, and reviewed like any other dependency change.
update-all:
	$(CARGO) update
	@echo ">> Обновлено ВСЁ, включая сторонние пины. Прочитайте diff Cargo.lock целиком"

# Backward-compatible alias for old local scripts. It has always meant "move everything", and it
# still does — which now also means it lifts the version freeze. Prefer naming `update-all`.
update-forks: update-all

# --- the user tour (docs/tour/index.html) ------------------------------------
# The page is GENERATED from locales/*.yml plus a committed snapshot of MoonUI's
# theme, and the generated file is committed. Needs PyYAML:
#   pip install -r tools/requirements.txt

# Regenerate the page. Always renders from the COMMITTED snapshot, so the output
# never depends on whether a MoonUI checkout happens to sit beside this one —
# otherwise CI, which has none, could not reproduce what a developer committed.
tour:
	$(TOUR)

# Fail if the committed page is not what the sources produce. This is the CI gate.
tour-check:
	$(TOUR) --check

tour-test:
	$(PYTHON) -m unittest discover -s tools/tour/tests -v

# Refresh the palette from a MoonUI checkout, recording the revision it came
# from. Deliberately SEPARATE from `tour`: folding it in would let one
# developer's sibling revision rewrite the palette inside an unrelated pull
# request. Expect this to be its own commit, like `update-moonproto`.
tour-theme:
	@test -f "$(MOONUI_THEME)" || { 	  echo "no MoonUI theme at $(MOONUI_THEME)"; 	  echo "set MOONUI=<path to a MoonUI checkout>"; exit 1; }
	@{ 	  echo "# moonui-rev: $$(git -C $(MOONUI) rev-parse HEAD)"; 	  echo "# moonui-blob: $$(git hash-object $(MOONUI_THEME))"; 	  echo "# source: crates/moon-ui-components/themes/moon-terminal.toml"; 	  echo "#"; 	  echo "# Snapshot of MoonUI's terminal theme, committed so the tour generator can run"; 	  echo "# without a MoonUI checkout (CI has none - MoonUI is a cargo git dependency)."; 	  echo "# Refreshed ONLY by \`make tour-theme\`, deliberately and in its own commit."; 	  echo "# DO NOT hand-edit: the upstream file is the source of truth."; 	  cat "$(MOONUI_THEME)"; 	} > $(TOUR_SNAPSHOT)
	@echo "[OK] $(TOUR_SNAPSHOT) refreshed - now run: make tour"
