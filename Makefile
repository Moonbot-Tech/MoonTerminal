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

ifeq ($(OS),Windows_NT)
  TARGET := --target x86_64-pc-windows-msvc
  BIN := target\x86_64-pc-windows-msvc\debug\moonterminal.exe
else
  TARGET :=
  BIN := target/debug/moonterminal
endif

.PHONY: run build release check fmt clean update-moon-ui update-forks help

help:
	@echo "make run | build | release | check | fmt | clean | update-moon-ui"
	@echo "bin: $(BIN)"

run:
	cargo run $(PKG) $(TARGET)

build:
	cargo build $(PKG) $(TARGET)

release:
	cargo build --release $(PKG) $(TARGET)

check:
	cargo check $(PKG) $(TARGET)

fmt:
	cargo fmt

clean:
	cargo clean

# Cargo.lock локальный и не коммитится. Fresh checkout резолвит текущий MoonUI master.
# В уже собранной рабочей копии этот target обновляет локальный lock до HEAD зависимостей.
update-moon-ui:
	cargo update
	@echo ">> Локальный Cargo.lock обновлён. Теперь: make build"

# Backward-compatible alias for old local scripts.
update-forks: update-moon-ui
