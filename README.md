<p align="center">
  <a href="https://moonbot.pro">
    <img src="assets/moonbot-logo-full.svg" alt="Moonbot" height="44">
  </a>
</p>

<h1 align="center">MoonTerminal</h1>

<p align="center">
  <b>Кроссплатформенный десктопный торговый терминал для ядра Moonbot</b><br>
  GPU-рендеринг графиков · живой поток MoonProto · Windows · macOS · Linux
</p>

<p align="center">
  <a href="https://github.com/Moonbot-Tech/MoonTerminal/actions/workflows/build.yml"><img src="https://github.com/Moonbot-Tech/MoonTerminal/actions/workflows/build.yml/badge.svg" alt="Build"></a>
  <a href="https://github.com/Moonbot-Tech/MoonTerminal/releases"><img src="https://img.shields.io/github/v/release/Moonbot-Tech/MoonTerminal?label=release&color=4C6EF5" alt="Release"></a>
  <img src="https://img.shields.io/badge/status-in%20development-F59E0B" alt="Статус: в разработке">
  <img src="https://img.shields.io/badge/platform-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-4C6EF5" alt="Платформы">
  <img src="https://img.shields.io/badge/built%20with-Rust-DEA584?logo=rust&logoColor=white" alt="Сделано на Rust">
  <img src="https://img.shields.io/badge/GPU-DX11%20%C2%B7%20Metal%20%C2%B7%20wgpu-8B5CF6" alt="GPU-бэкенды">
</p>

<p align="center">
  <b>Русский</b> · <a href="README.en.md">English</a>
  &nbsp;&nbsp;|&nbsp;&nbsp;
  <a href="#возможности">Возможности</a> ·
  <a href="#скриншоты">Скриншоты</a> ·
  <a href="#архитектура">Архитектура</a> ·
  <a href="#сборка">Сборка</a> ·
  <a href="#конфигурация">Конфигурация</a> ·
  <a href="#документация">Документация</a>
</p>

<p align="center">
  <a href="https://moonbot-tech.github.io/MoonTerminal/"><b>Инструкция для пользователей</b></a> — интерактивный тур по интерфейсу терминала
</p>

MoonTerminal — нативный десктопный торговый терминал для ядра криптотрейдинга **[Moonbot](https://moonbot.pro)**. Он показывает живые графики, стаканы, ордера, отчёты и стратегии для одного или нескольких ядер Moonbot в едином GPU-ускоренном окне под Windows, macOS и Linux.

> **В разработке.** Это рабочее пространство активной разработки терминала: оболочка на GPUI, интеграция MoonUI, живой поток данных MoonProto, платформенный GPU-рендеринг графиков и отладочный инструментарий. Это ещё не готовый упакованный продукт.

<p align="center">
  <img src="assets/img/screenshot-main.png" alt="Главное окно MoonTerminal" width="900">
</p>

## Возможности

- **GPU-рендеринг графиков** — собственный GPU-проход на каждой платформе (DirectX 11 под Windows, Metal под macOS, нативный `wgpu` под Linux). Без CPU-readback, поэтому live-скролл и зум остаются плавными и не перерисовывают всё окно.
- **Живой поток MoonProto** — событийные рыночные данные через backend-цикл на waker'е. Видимый график подтягивает данные на кадровом тике, а не постоянным поллингом.
- **Торговые панели** — ордера и редактирование ордеров, стакан, отчёты, активы и кошельки, рыночный скринер и дерево стратегий для ваших ядер Moonbot.
- **Ордерные линии на графике** — строятся из текущего снапшота плюс захваченных событий, поэтому короткие терминальные статусы (`Cancel` / `Fail` / `Done`) не теряются.
- **Открепляемые окна** — вкладки графиков и панели дока выносятся в отдельные окна; состояние дока и раскладки сохраняется между сессиями.
- **Локализация интерфейса** — русский, английский и испанский через `rust-i18n`.
- **Шифрованная конфигурация** — учётные данные серверов хранятся в защищённом хранилище / кейринге ОС (Secret Service под Linux).
- **Мелочи для повседневной работы** — звуки алертов, иконки монет, свои хоткеи и темы, а также встроенный `chart-smoke` FireTest, который проверяет живые bounds графика, нативный ввод и счётчики CPU/GPU/RAM.

## Скриншоты

<table>
  <tr>
    <td width="50%"><img src="assets/img/screenshot-charts.png" alt="Рабочая область графиков"></td>
    <td width="50%"><img src="assets/img/screenshot-settings-connections.png" alt="Настройки — подключения / ядра Moonbot"></td>
  </tr>
  <tr>
    <td align="center"><sub>Графики — GPU-рендеринг свечей, стакан, ордерные линии</sub></td>
    <td align="center"><sub>Настройки → Подключения — настройка по каждому ядру</sub></td>
  </tr>
</table>

## Архитектура

MoonTerminal — виртуальный workspace на Rust: UI-независимое ядро, поверх которого работает оболочка на GPUI.

| Крейт | Ответственность |
|---|---|
| [`moon-core`](crates/moon-core) | UI-независимое ядро — подключения, конфиг, сессии, market state, отчёты. |
| [`moon-chart`](crates/moon-chart) | Математика графика — time/price view, дефолтный масштаб, pan/zoom, оси (без wgpu). |
| [`moon-ui-gpui`](crates/moon-ui-gpui) | Бинарь `moonterminal` — GPUI shell, панели, debug-инструменты, интеграция графика. |
| [`Moonbot-Tech/MoonUI`](https://github.com/Moonbot-Tech/MoonUI) | Внешняя Git-зависимость — standalone GPUI runtime + компоненты Moon UI. |

**Рендер.** График рисуется отдельным GPU-проходом поверх MoonUI/GPUI — DX11/HLSL под Windows, Metal под macOS, нативный `wgpu`/WGSL-бэкенд GPUI под Linux. График сам решает, нужен ли кадр, и готовит данные к этому же кадру, поэтому оболочка и панели ордеров не перерисовываются на частоте live-скролла или движения мыши.

**Data path.** События MoonProto приходят через event sink с waker'ом; backend-цикл будится реальными событиями и командами, а не таймером. Видимый график тянет рыночные данные на кадровом тике, а core-owned общий read-model обслуживает остальных потребителей.

Полная картина — в [документе об архитектуре](docs/ARCHITECTURE.md).

## Сборка

### Клонирование

```bash
git clone https://github.com/Moonbot-Tech/MoonTerminal.git
cd MoonTerminal
```

### Windows

Требования:

- Git
- Rust через `rustup`
- Visual Studio 2022 Build Tools с C++-тулчейном и Windows SDK
- Опционально: `make`

```powershell
cargo build -p moon-ui-gpui --bin moonterminal --target x86_64-pc-windows-msvc
```

Отладочный исполняемый файл:

```text
target\x86_64-pc-windows-msvc\debug\moonterminal.exe
```

### macOS

Требования:

- Xcode или рабочий Metal-тулчейн
- Rust через `rustup`

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

Каноничная проверка Metal — см. [гайд по сборке под macOS и Linux](docs/MAC_LINUX_BUILD.md).

### Linux

Базовый набор для Ubuntu/Debian:

```bash
sudo apt update && sudo apt install -y git build-essential pkg-config \
  libfontconfig-dev libwayland-dev libxkbcommon-dev libvulkan-dev libssl-dev
```

```bash
cargo build -p moon-ui-gpui --bin moonterminal
```

Шифрованный конфиг под Linux использует Secret Service в пользовательской GUI/DBus-сессии.
Подробности — в [гайде по сборке под macOS и Linux](docs/MAC_LINUX_BUILD.md).

### Основные команды

| Команда | Назначение |
|---|---|
| `make run` | собрать и запустить отладочный терминал |
| `make build` | отладочная сборка |
| `make release` | релизная сборка |
| `make check` | проверка типов |
| `make fmt` | `cargo fmt` |
| `make clean` | очистить `target` |
| `make update-moon-ui` | сдвинуть пин MoonUI в коммитимом `Cargo.lock` |
| `make update-moonproto` | сдвинуть пин MoonProto (отдельным осознанным коммитом) |
| `make update-all` | сдвинуть ВСЁ, включая сторонние пины — снимает заморозку версий |

Makefile выбирает MSVC-таргет на Windows и нативный таргет на macOS/Linux.

## Конфигурация

Серверы настраиваются в интерфейсе приложения:

```text
Настройки → Подключения
```

Настройка подключений по каждому ядру — см. [скриншот «Настройки → Подключения»](#скриншоты) выше.

Рантайм-конфиг лежит рядом с исполняемым файлом. Учётные данные серверов хранятся в шифрованном конфиге через защищённое хранилище/кейринг ОС, где доступно. Локальные конфиг-файлы и логи игнорируются Git.

## Документация

| Документ | Что внутри |
|---|---|
| [Инструкция для пользователей](https://moonbot-tech.github.io/MoonTerminal/) | Интерактивный тур: карта главного окна, первое подключение, панели, хоткеи |
| [Архитектура](docs/ARCHITECTURE.md) | Крейты, платформенный GPU-рендеринг и живой data path |
| [FireTest](docs/FIRETEST.md) | Встроенный live-зонд `chart-smoke` |
| [Сборка под macOS и Linux](docs/MAC_LINUX_BUILD.md) | Проверка Metal и настройка Linux |
| [Окна](docs/WINDOWING.md) | Своя шапка окна и borderless/CSD-поведение |

---

<p align="center">
  Moonbot · Продвинутый терминал для торговли криптовалютой · <a href="https://moonbot.pro">moonbot.pro</a>
</p>
