# Windowing Contract

Док фиксирует текущий контракт окон MoonTerminal поверх MoonUI/GPUI. Это не
визуальный референс, а инженерное правило: как открывать окна, чтобы не ломать
taskbar/dock semantics, restore и chart own-pass.

## Где создавать окна

Новые окна терминала открывать через `crates/moon-ui-gpui/src/windowing.rs`.

Нельзя собирать `gpui::WindowOptions` руками в панелях, настройках или
редакторе стратегий без явной причины. Иначе легко забыть `app_id`,
decorations, owner, taskbar policy или min size, и снова получить окно, которое
ведет себя как отдельное приложение.

Текущие фабрики:

- `trading_window_options` - основное окно группы/терминала.
- `tool_window_options` - owned tool/secondary окна вроде настроек,
  стратегий и активов.
- `detached_panel_window_options` - открепленные non-chart панели
  (`Orders`, `Assets`, `Log`, `Report`).
- `detached_chart_window_options` - открепленные chart windows; намеренно
  independent от main/group окна.
- `debug_window_options` - debug/perf/chart diagnostic windows.
- `profit_monitor_window_options` - independent desktop Profit Monitor without its own taskbar
  button; minimizing a Main/group window never minimizes it.

## MoonUI native contract

В MoonUI `WindowOptions` расширен двумя полями:

- `WindowRelationship` - независимое окно или owned window с owner handle.
- `WindowTaskbarVisibility` - показывать ли отдельную кнопку taskbar там, где
  платформа поддерживает per-window taskbar entries.

Default policy: owned windows скрываются из taskbar, independent windows
показываются.

Backend mapping:

- Windows: `WindowRelationship::Owned` превращается в Win32 owned window через
  owner `HWND`, без modal блокировки parent. Dialog остается отдельной modal
  логикой. `AppUserModelID` ставится как `MoonTerminal`.
- macOS: owned floating window добавляется как AppKit child window над owner и
  исключается из native Windows menu.
- Wayland: owner превращается в xdg parent.
- X11: owner превращается в transient parent, а hidden taskbar policy ставит
  `_NET_WM_STATE_SKIP_TASKBAR`.

## Window chrome: закрытый API

Финальный контракт: терминал не использует `MoonWindowChrome` напрямую и не
рисует самодельные `x`, `-`, `[]` в отдельных окнах. Для визуального chrome и
native hit-зон используется `MoonWindowFrame` из MoonUI.

Старый `MoonWindowChrome` удален из публичного API `moon_ui`. Он был слишком
низкоуровневым: давал hit-зоны и window-control areas, но не владел визуальной
семантикой окна - брендом, title cluster, цветами, hover state и тем, какое
лого допустимо для конкретного типа окна. Именно из-за этого debug/tool окно
могло снова получить большой wordmark как у главного окна. Теперь экран не
собирает chrome из частей, а выбирает тип окна через `MoonWindowFrameKind`.

`MoonWindowFrame` одновременно задает:

- тип окна: `Main`, `Tool`, `Popup`, `DetachedPanel`, `DetachedChart`, `Debug`;
- набор window controls: `None`, `Close`, `MinimizeClose`,
  `MinimizeMaximizeClose`;
- visual controls: символы, цвета, hover state, размер кнопок из MoonTheme;
- native control areas: `Min`, `Max`, `Close`;
- drag handle: `WindowControlArea::Drag`, double click -> native titlebar
  double click, mouse down -> native window move;
- hit overlay для тех окон, где drag-зона должна быть отдельной прозрачной
  областью поверх header.

## Визуальные типы окон

MoonTerminal использует три визуальных класса окон:

- Главное окно: одно основное окно терминала. Только оно имеет полный wordmark
  `Moonbot` в header. В API это `MoonWindowFrameKind::Main`.
- Tool/secondary окна: настройки, стратегии, debug, detached chart и другие
  вспомогательные окна. Они имеют маленький mark без надписи Moonbot. В API это
  `Tool`, `DetachedPanel`, `DetachedChart`, `Debug`.
- Popup/overlay окна: компактные окна без брендинга. В API это `Popup`.

Экран не выбирает логотип сам. Нельзя напрямую вызывать terminal helpers вроде
`logo_sized`, `logo_mark` или рисовать SVG/logo руками в titlebar. Branding
выбирает `MoonWindowFrame` по `MoonWindowFrameKind`:

- `Main` -> full logo;
- `Tool` / `DetachedPanel` / `DetachedChart` / `Debug` -> small mark;
- `Popup` -> no logo.

Для titlebar-зоны использовать:

- `MoonWindowFrame::brand_cluster(cx)` - brand + separator без title;
- `MoonWindowFrame::title_cluster(title, cx)` - brand + separator + title;
- `MoonWindowFrame::visual_controls(cx)` - OS-кнопки;
- `MoonWindowFrame::drag_handle()` / `hit_overlay()` - native drag/hit зоны.

Правильная композиция:

- `windowing.rs` открывает OS-window и задает owner/taskbar/app_id/decorations;
- header визуально рисует прикладное содержимое окна: brand, title, метрики,
  кнопки терминала;
- `MoonWindowFrame::brand_cluster(...)` / `title_cluster(...)` рисуют правильный
  brand для типа окна;
- `MoonWindowFrame::visual_controls(...)` рисует OS-кнопки окна;
- `MoonWindowFrame::drag_handle()` ставится на spacer зоны;
- `MoonWindowFrame::hit_overlay()` ставится последним child только там, где
  нужен отдельный прозрачный drag overlay.

Если в экране хочется "просто поставить логотип" или "просто нарисовать x",
это значит, что в MoonUI не хватает нужного `MoonWindowFrameKind` или helper в
`MoonWindowFrame`. Исправлять надо MoonUI-контракт, а не конкретный экран.

Прямые использования в terminal UI запрещены:

- `MoonWindowChrome::new`;
- `MoonWindowChromeButton`;
- `WindowControlArea::Drag`;
- `start_window_move`;
- `titlebar_double_click`;
- `logo_sized` / `logo_mark` вне самого brand/helper слоя;
- `WindowOptions { ... }` вне `windowing.rs`.

Запрет закреплен тестом `terminal_windows_use_closed_window_frame_api` в
`crates/moon-ui-gpui/tests/theme_contract/`.

Если понадобится новый вид окна, например нестандартный круглый titlebar или
controls в центре, добавлять новый `MoonWindowFrameKind`/layout в MoonUI и
одну фабрику в `windowing.rs`, а не править отдельные экраны.

Generic detached panels (`Orders`, `Assets`, `Log`, `Report`) тоже считаются
`DetachedPanel`, а не "просто отдельным окном с контентом". Они обязаны иметь
custom titlebar через `MoonWindowFrame::detached_panel(...)` и открываться
через `detached_panel_window_options(...)`; иначе получаем четвертый
визуальный/поведенческий тип окна, которого нет в дизайне.

## Owner и taskbar policy

Нельзя вызывать `cx.window_handle()` из `Context` view/entity. У
`gpui::Context<'_, T>` такого API нет, и при restore сохраненных окон текущего
`Window` физически нет.

Owner используется только для owner-aware типов окон:

- `tool_window_options`;
- `debug_window_options`;
- `detached_panel_window_options`.

Для них правильная схема:

- live UI click: взять `window.window_handle()` в callback, где есть `Window`,
  и передать `Some(owner)`;
- restore/startup: сначала попытаться найти живое окно группы через
  `Backend.group_windows`; если owner не найден, передать `None`.

Если detached panel восстанавливается без owner, `detached::spawn` пытается
найти окно группы через `Backend.group_windows`. Если owner не найден, окно
остается independent. Это нормальное поведение для restore.

Detached chart windows - отдельное правило. Они НИКОГДА не owned, даже при
runtime detach, потому что owned/transient связь ОС поднимает Main/group окно
при клике по графику. На мультимониторе это выглядит как прыжок основного окна
на другом экране. Поэтому chart windows открываются только через
`detached_chart_window_options(...)`: owner в их API отсутствует, окно
independent, а отдельная taskbar-кнопка подавляется общим механизмом (см. ниже).

Итоговая taskbar policy:

- `trading_window_options` - видимое основное окно приложения;
- `tool_window_options`, `debug_window_options`,
  `detached_panel_window_options` - hidden из taskbar, когда есть owner; при
  restore без owner становятся independent и могут получить taskbar entry;
- `detached_chart_window_options` - always independent, но hidden из taskbar;
- `profit_monitor_window_options` - та же комбинация: always independent, но hidden из taskbar.

Текущие окна `Настройки`, `Стратегии` и `Активы` считаются
`Tool/secondary`, поэтому открываются через `tool_window_options(...)`. Если
экран визуально использует `MoonWindowFrame::tool(...)`, но открывается через
самостоятельную `WindowOptions`-ветку, это архитектурная ошибка: окно выглядит
как часть терминала, но ОС ведет его как отдельное приложение.

Profit Monitor is the deliberate exception to the usual visual-tool ownership rule. It keeps
`MoonWindowFrame::tool(...)` chrome because it is visually part of MoonTerminal, but its product
role is a separately placeable desktop widget, so its centralized factory carries no owner: a
minimized Main window leaves the monitor on screen and FancyZones can snap it. The terminal still
presents ONE taskbar icon, so the monitor's own button is suppressed exactly as a chart window's
is. Routes back to a minimized monitor: the terminal toolbar button (its `activate_window` restores
an iconic window) and Alt+Tab, which the window keeps because hidden taskbar visibility only drops
`WS_EX_APPWINDOW` and never applies the tool-window style. Do not copy that OS policy to ordinary
tool windows.

Обе independent-ветки - detached charts и Profit Monitor - подавляют taskbar-кнопку ОДНИМ
механизмом: `WindowTaskbarVisibility::Hidden` плюс `hide_window_from_taskbar_soon`.
`WindowTaskbarVisibility::Hidden` сам по себе гарантии не даёт: он лишь снимает `WS_EX_APPWINDOW`,
который unowned top-level окну и не нужен, чтобы получить кнопку. `ITaskbarList::DeleteTab` удаляет
уже существующий элемент и НЕ является постоянным состоянием окна: оболочка публикует элемент чуть
позже показа окна и заново - при разворачивании свёрнутого окна. Поэтому burst удалений
взводится при открытии окна и повторно на каждой активации (`observe_window_activation`); burst
ограничен по числу попыток, так что перекрывающиеся взводы не превращаются в постоянную работу.
Настоящее место для этой логики - Windows-бэкенд форка MoonUI (там доступны wndproc и broadcast
`TaskbarCreated`); пока `Hidden` там ничего не гарантирует, компенсация живёт здесь.

## Chart windows и UnderScene

Chart не является GPUI UI-компонентом. Он рисуется own-pass в UnderScene через
chartdx/raw GPU path. Поэтому GPUI оболочка должна выделять место под chart, но
не должна класть непрозрачный quad поверх plot/body.

Правило:

- chart/debug/detached-chart root: `MoonBackgroundPolicy::NoFill`;
- header/chrome можно красить `.bg(...)`;
- body вокруг `ChartPanel` нельзя красить `.bg(...)`;
- обычные non-chart окна/панели могут быть opaque.

Если покрасить body вокруг `ChartPanel`, на macOS/Linux native chart может
работать по логам и counters, но визуально быть пустым: GPUI background
закрывает own-pass.

Контракт закреплен тестом `crates/moon-ui-gpui/tests/theme_contract/`.

## Debug artifacts

Скриншоты, временные логи и live-test артефакты не класть в `docs`.
Для этого использовать `tmp/`; папка должна оставаться ignored.
