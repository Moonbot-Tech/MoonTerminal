<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="43">
  </a>
</p>

# MoonTerminal

<p align="center">
  <b>Русский</b> · <a href="README.en.md">English</a>
</p>

Репозиторий разработки кроссплатформенного торгового терминала для Moonbot kernel.

MoonTerminal ещё не готовый продукт. Это рабочее пространство активной разработки десктопного
терминала: оболочка на GPUI, интеграция MoonUI, живой поток данных MoonProto, рендеринг графиков,
отладочный инструментарий и платформенная работа под Windows, macOS и Linux.

<p align="center">
  <img src="assets/img/screenshot-main.png" alt="Главное окно MoonTerminal" width="900">
</p>
---


## Клонирование

```bash
git clone https://github.com/Moonbot-Tech/MoonTerminal.git
cd MoonTerminal
```

## Сборка под Windows

Требования:

- Git
- Rust через `rustup`
- Visual Studio 2022 Build Tools с C++-тулчейном и Windows SDK
- Опционально: `make`

PowerShell:

```powershell
cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
```

Отладочный исполняемый файл:

```text
target\x86_64-pc-windows-msvc\debug\moonterminal.exe
```

## Сборка под macOS

Требования:

- Xcode или рабочий Metal-тулчейн
- Rust через `rustup`

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

Каноничная проверка Metal — см. [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md).

## Сборка под Linux

Базовый набор для Ubuntu/Debian:

```bash
sudo apt update && sudo apt install -y git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev
```

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

Шифрованный конфиг под Linux использует Secret Service в пользовательской GUI/DBus-сессии.
Подробности: [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md).

---

## Основные команды

| Команда | Назначение |
|---|---|
| `make run` | собрать и запустить отладочный терминал |
| `make build` | отладочная сборка |
| `make release` | релизная сборка |
| `make check` | проверка типов |
| `make update-moon-ui` | обновить локальный игнорируемый `Cargo.lock` для «плавающих» Git-зависимостей |

Makefile выбирает MSVC-таргет на Windows и нативный таргет на macOS/Linux.

---

## Конфигурация

Серверы настраиваются в интерфейсе приложения:

```text
Настройки -> Подключения
```
Настройка подключений по каждому ядру.

<p align="center">
  <img src="assets/img/screenshot-settings-connections.png" alt="Настройки — подключения / ядра MoonBot" width="640">
</p>

Рантайм-конфиг лежит рядом с исполняемым файлом. Учётные данные серверов хранятся в шифрованном
конфиге через защищённое хранилище/кейринг ОС, где он доступен. Локальные конфиг-файлы и логи
игнорируются Git.

---

Полезные доки:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
- [docs/FIRETEST.md](docs/FIRETEST.md)
- [docs/MAC_LINUX_BUILD.md](docs/MAC_LINUX_BUILD.md)
- [docs/WINDOWING.md](docs/WINDOWING.md)

---

<p align="center">
  Moonbot / Продвинутый терминал для торговли криптовалютой / <a href="https://moonbot.pro">moonbot.pro</a>
</p>
