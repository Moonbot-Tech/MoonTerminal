# MoonTerminal Architecture

Дата актуализации: 2026-06-24.

Этот документ описывает текущую публичную архитектуру терминала и намеренно не включает старые
экспериментальные планы миграции.

## Состав

- `moon-core` — UI-независимое ядро: подключения, конфиг, сессии, market state, отчёты.
- `moon-chart` — математика чарта: time/price view, phase-clean default scale, pan/zoom, оси.
- `moon-ui-gpui` — бинарь `moonterminal`: GPUI shell, панели, debug-tools, chart integration.
- `Moonbot-Tech/MoonUI` — внешний git dependency: standalone GPUI runtime + Moon UI components.

Legacy UI/runtime packages не являются активными зависимостями. Новые общие UI/runtime изменения
должны идти через MoonUI.

## Рендер

Чарт рисуется через GPU own-pass поверх MoonUI/GPUI:

- Windows: DX11/HLSL.
- macOS: Metal/MSL.
- Linux: нативный GPUI wgpu backend/WGSL.

Это не старый `egui + wgpu offscreen + readback` и не shared-texture bridge между разными
рендерерами. CPU readback для живого чарта не используется.

Ключевой контракт: график сам принимает решение, нужен ли кадр (`gpu_canvas.frame()`), и может
подготовить данные к этому же кадру без top-down `cx.notify()` всего окна. Shell/Orders не должны
перерисовываться на частоте live-scroll, mousemove или present.

## Data Path

Текущий live-path ушёл от старого постоянного polling и top-down переноса chart data:

- MoonProto события приходят через event sink с waker.
- Backend loop ждёт реальные события/команды через waker, а не будится таймером.
- Видимый chart подтягивает market data через `MarketDataSource` внутри `gpu_canvas.frame()`.
- `MarketDataSource` читает `snapshot_versioned()` и двигает consumer cursor только для реально
  видимого chart path.
- `SharedMarketStore` остаётся core-owned совместимым read-model для остальных потребителей; это
  не GPUI entity и не причина top-down render.

Push-события остаются для UI-виджетов и редких уведомлений. Chart data path — pull на frame tick.
Ордерные события — отдельный важный контракт: MoonProto `OrderEvent::Created/Updated/Removed`
несёт Arc-backed строку ордера на момент события. Терминал строит order-lines из текущего
snapshot плюс captured event rows, чтобы короткий terminal status (`Cancel`, `Fail`, `Done`) не
терялся, даже если latest snapshot уже убрал uid из live-list.

## UI Components

Приложение зависит от `Moonbot-Tech/MoonUI` и использует компоненты через `moon_ui::*` /
`moon_ui::components::*`. Прикладные панели терминала не должны заново рисовать общие UI-паттерны
вручную, если в MoonUI уже есть подходящий компонент или близкий Longbridge-наследник.

Правило адаптации:

- если компонент Longbridge уже даёт нужную механику, но тема/геометрия/состояния не соответствуют
  MoonBot design, править или оборачивать его нужно внутри MoonUI;
- терминал после этого использует MoonUI API, а не прямой Longbridge API и не локальный ad-hoc
  виджет в конкретной панели;
- если в MoonUI не хватает публичного hook/API для терминального сценария, сначала добавить этот
  hook в MoonUI, затем заменить экранный ручной код;
- временные исключения должны быть явно помечены в коде или docs с причиной и планом удаления;
- chart renderer не является UI-компонентом: chart host может использовать MoonUI chrome/overlays,
  но собственный GPU render остаётся в `chartdx`.

Практический пример: popup/menu/dialog механика должна идти через `moon_ui::components`
(`WindowExt`, `Root` dialog/sheet/context-menu/notification layers, Moon menu wrappers). Если
базовый Longbridge `ContextMenuExt` рисует в чужой теме, его надо привести к Moon-теме в MoonUI
или использовать Moon-обёртку. В терминале нельзя рендерить открытое контекст-меню как child
панели: открывать через `window.open_moon_context_menu(...)`, чтобы z-order, dismiss и future
portal-поведение оставались ответственностью MoonUI Root.

Root overlay layers не являются внешними render hooks для приложения. Приложение открывает dialog,
sheet, context menu и notification через `WindowExt`/Moon wrappers; сам `Root::render` решает, где
и в каком порядке эти слои оказываются относительно основного view. Это важно для chart
UnderScene/z-order и для одинакового поведения на Windows/macOS/Linux.

FireTest не читает исходники и не проверяет архитектуру статически. Встроенный
`--debug-script chart-smoke` проверяет живое поведение: открытие графика, реальные bounds,
native input, counters/CPU/GPU/RAM. Статические запреты вида "не рендерить меню как child
панели" живут в `tests/theme_contract.rs`, а не внутри runtime-сценария.

## Окна

Терминал использует собственную шапку и borderless/CSD поведение. Проверять отдельно:

- Windows: restore bounds на multi-monitor/DPI.
- macOS: Metal toolchain и `.app` запуск из GUI session.
- Linux X11/Wayland: отсутствие второй системной шапки, Secret Service для encrypted config,
  стабильность surface/present.

## Локальная Разработка

Публичные `Cargo.toml` держат git-зависимости на `Moonbot-Tech/MoonUI` `branch = "master"`.
`Cargo.lock` в терминале не коммитится намеренно: dev-ветка живёт в режиме rolling integration,
чтобы свежий checkout собирался против актуального MoonUI master. Это упрощает синхронную работу
терминала и компонентов, но не является strict reproducible build по дате.

Каждый бинарь пишет в лог build stamp:

```text
build: moonterminal=<git-sha>[+dirty] moonui=<git-sha|local:git-sha>[+dirty]
```

Для release/stabilization можно отдельно зафиксировать tag/rev или вернуть коммитимый lock; сейчас
это сознательно не делается.

Для локальной разработки рядом должны лежать:

```text
workspace/
  MoonTerminal/
  MoonUI/
  MoonProtoBeta/
```

Локальная подмена делается только в ignored `MoonTerminal/.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }

[patch."https://github.com/Moonbot-Tech/MoonProtoBeta"]
moonproto = { path = "../MoonProtoBeta" }
```

Не использовать top-level `paths`: он меняет форму dependency graph и уже сейчас даёт Cargo warning,
который в будущих версиях Cargo может стать ошибкой.
