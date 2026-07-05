# MoonTerminal — сборка единственного бинаря `moonterminal` (crates/moon-ui-gpui).
#
#   make run            собрать и запустить (debug)
#   make build          собрать (debug)
#   make release        собрать (release)
#   make check          быстрая проверка типов
#   make fmt            cargo fmt
#   make clean          очистить target
#   make update-moon-ui обновить локальный Cargo.lock до HEAD зависимостей
#
# Windows: таргет ВСЕГДА MSVC (x86_64-pc-windows-msvc), не GNU — его ожидают GPUI/DirectX/
# chartdx. Запускать `make` из «x64 Native Tools Command Prompt for VS 2022» (там настроен
# vcvars), иначе линковка C-зависимостей не найдёт link.exe.
# macOS (Metal) / Linux: нативный таргет, отдельная настройка не нужна.

PKG := -p moon-ui-gpui --bin moonterminal

# cargo может быть не в PATH (частый случай на macOS: он в ~/.cargo/bin, но Terminal
# не подхватывает ~/.cargo/env). Ищем в PATH, иначе берём дефолтный путь rustup.
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

# macOS: после сборки подписываем бинарь СТАБИЛЬНОЙ самоподписанной подписью. Иначе
# ad-hoc подпись меняется каждую сборку, и macOS Keychain (в нём лежит ключ шифрования
# конфига, крейт keyring/apple-native) при каждом запуске заново требует пароль.
# См. scripts/macos-sign.sh. На Windows/Linux — no-op (этой проблемы там нет).
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

# macOS: один раз создать самоподписанный code-signing сертификат (иначе он создастся
# автоматически при первой сборке). На других ОС скрипт сам делает no-op.
codesign-setup:
	./scripts/macos-codesign-setup.sh

# run зависит от build → запускается уже ПОДПИСАННЫЙ бинарь (не свежий unsigned из
# `cargo run`), поэтому Keychain не спрашивает пароль повторно.
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

# Cargo.lock локальный и не коммитится. Fresh checkout резолвит текущий MoonUI master.
# В уже собранной рабочей копии этот target обновляет локальный lock до HEAD зависимостей.
update-moon-ui:
	$(CARGO) update
	@echo ">> Локальный Cargo.lock обновлён. Теперь: make build"

# Backward-compatible alias for old local scripts.
update-forks: update-moon-ui
