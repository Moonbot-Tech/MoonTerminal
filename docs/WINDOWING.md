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

## Auto workspace: rail, dock и окна групп

Auto — полноценное рабочее пространство внутри уже существующего группового окна, а не новый тип
OS-window. У каждой активной группы остаётся свой `Shell`, свой единственный `DockArea` и свои
локальные panel instances.
Левая `MoonVirtualList` rail при этом показывает все настроенные ядра приложения: секции идут в
алфавитном порядке бирж из live market metadata, неизвестная биржа остаётся отдельной первой
секцией, а участники сохраняют порядок канонического `core_order::CoreOrder`. Имя ядра не
используется для угадывания биржи. Неактивные и недоступные строки не исчезают, а показывают
состояние и не принимают клик.

Ядро selectable, когда активны оно и его группа, существует live session и окно группы находится в
состоянии `Opening` или `Live`. Per-core `show_window` здесь не участвует: headless core доступен
через общее живое окно своей группы. Клик внутри текущей группы только меняет её Auto scope. Клик
по ядру другой группы атомарно сохраняет для destination `AutoTrading` + core, передаёт ему
singleton focus и активирует уже существующее group window; panels между окнами не reparent'ятся и
новое параллельное окно не создаётся.

Rail имеет Overview только для текущей группы, но её header-сводка считает configured/ready/problem
по всему приложению. Общая стартовая ширина `340 px` хранится в `layout.toml`, ограничивается
диапазоном `52..560 px` и после drag-resize сразу распространяется на все открытые Auto-окна.
Разделитель между rail и единственным `DockArea` использует `MoonResizablePanelGroup`;
Full/Compact/Icon presentation следует за фактической шириной. На compact имена обрезаются, на icon
остаётся короткая метка, а полное имя и status доступны в tooltip. Ready обозначается увеличенным
зелёным dot без повторяющей подписи; ошибки и недоступность имеют видимую подпись в Full и tooltip в
Compact/Icon. При сужении окна rail локально уступает место dock, не перезаписывая общую сохранённую
ширину; при обратном расширении предпочтение восстанавливается.

Auto dock использует одну общую topology-only схему из `auto_dock.json`. Она хранит split tree,
стороны, размеры и порядок вкладок по стабильным именам, но не переносит между окнами panel payload,
group IDs, active tab, zoom или `Rc`. Каждый Shell применяет схему к собственным живым панелям;
изменение в одном Auto-окне через Backend revision синхронно доходит до остальных и не создаёт
feedback loop. `ChartTabs` всегда стоит первым, визуально отделён, закреплён от drag и защищён от
вставки вкладок перед ним. Остальные панели можно reorder'ить, делить и resize'ить. Detach обычной
панели или chart tab в Auto запрещён до создания окна и до изменения persistence.

Отсутствующий `auto_dock.json` означает первый запуск и разрешает сохранить стартовый preset.
Нечитаемый или невалидный файл — отдельное recovery-состояние: безопасный preset показывается
только в памяти и не перезаписывает файл, пока пользователь явно не изменит topology. Запись
Live-сохранение `layout.toml`, общей Auto topology и пары Classic
`docks.json`/`detached.json` проходит через один serial persistence worker. GPUI передаёт ему только
immutable snapshots и опрашивает acknowledgements; файловые open/write/flush/sync не выполняются в
UI tick. Classic-пара сохраняется как одна логическая транзакция через
`window-state.pending.json`: journal записывается до обоих public-файлов и удаляется только после
двух успешных atomic replace. После аварийной остановки startup сначала replay'ит journal целиком.
После принятого enqueue соответствующий dirty-флаг снимается; новая мутация или failed
acknowledgement выставляет его снова, поэтому временная ошибка файловой системы повторяется на
следующем flush вместо тихой потери layout. На quit последний полный snapshot
ставится за уже выполняющейся записью, worker join'ится, а недоступный или упавший worker получает
единственный синхронный fallback именно на границе завершения. Повторяющиеся detached-spec с
одинаковым `(group, panel)` схлопываются до создания native windows.

Если `auto_dock.json` ещё не существует, первый Auto workspace получает вертикальный operations
preset: гибкий верхний tab stack с активным «Отчётом», закреплёнными слева «Чартами» и «Логом» среди
остальных верхних вкладок; единственный нижний блок «Ордера» получает высоту ещё четырёх строк
таблицы. После первого пользовательского reorder/split/resize эта схема заменяется общей
сохранённой topology и применяется ко всем Auto-окнам; Classic topology при этом не читается и не
меняется.

При входе в Auto полный named-layout Classic остаётся локальным runtime-состоянием Shell. Обычные
панели, уже откреплённые в Classic, временно закрываются без удаления их specs/geometry и получают
Auto-only экземпляры в главном dock; общий `respawn_all` пропускает Auto-группы. Закрытие ранее
откреплённой owned-панели сначала снимает точный live handle, затем уведомляет Shell, чтобы Auto мог
вернуть вкладку без смены режима. После возврата в Classic временные экземпляры удаляются, точный
layout с active/zoom/sizes восстанавливается, а detached windows открываются снова по прежним specs:
перед каждым native create выполняется настоящий timer yield, после которого повторно проверяются
режим, ownership и shutdown. В Auto у dock-панелей нет close-кнопок: полный
набор поверхностей сохраняется, при этом reorder/split/resize остаются доступными. Уже существующие
detached chart windows не закрываются, но новые из Auto создать нельзя. `docks.json` и
`detached.json` остаются исключительно
Classic authority и не переписываются Auto-событиями.

Существующий detached chart может оставаться на другом мониторе как контекст, но каждое его
торговое действие и переход на Main повторно проверяет актуальный rail-scope группы. График старого
ядра остаётся видимым, однако не может отправить команду или обойти выбор сервера через левую панель.

Все прежние `Backend::open_on_main` routes остаются действующими. Shell при создании запоминает уже
существующий request revision, поэтому вкладку `ChartTabs` программно активируют только новые
revisions, замеченные этим Shell после его создания и адресованные его Auto-группе; request,
появившийся до окна, не отбирает стартовую вкладку. В Classic reveal не отбирает активную dock tab.
Флаг request `activate` по-прежнему отдельно решает, поднимать ли OS-window. Перед observer
signature и consume target core заново разрешается через live session. Group-owned request несёт
неизменяемую authority исходного окна: если ядро сменило группу, исчезло или вышло из текущего Auto
scope, request отменяется вместо переноса действия. Только явно unscoped internal/global request
может следовать за живым ядром в новую группу. Detached chart windows остаются independent по OS
ownership и не получают owner только из-за Auto.

Конкретный клик по rail становится единственным способом сменить ядро Auto: все scoped panels
перечитывают effective scope, header показывает тот же core как пассивный indicator, а текущий Main
chart заменяется или фокусируется без накопления новых вкладок. Рынок сохраняется exact-match,
затем через match-key/quote fallback; если подходящего рынка нет, chart не меняется. Overview не
выбирает произвольное ядро и не ретаргетит Main chart. Classic selection и Classic dock при этом не
меняются.

Логическое владение singleton scope следует за реальным взаимодействием: native activation
group window или detached panel обновляет `WorkspaceFocus`. Для detached chart каждое native
activation делает это без зависимости от inactivity-close настройки, а существующий путь
активного chart interaction/polling повторно подтверждает группу, пока пользователь работает в
окне. Для charts это не OS owner/transient relationship — только маршрутизация
`Analytics`/`Strategies`. Native focus/taskbar behaviour и визуальное попадание трёх responsive
ступеней требуют ручной проверки в собранном приложении; статические и unit-тесты проверяют лишь
программные решения и инварианты.

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
взводится при открытии окна и повторно на каждой активации (`observe_window_activation`); прежний
burst отменяется до запуска нового. COM, sleep и retries выполняются вне GPUI, а bounded burst не
превращается в постоянную работу.
Настоящее место для этой логики - Windows-бэкенд форка MoonUI (там доступны wndproc и broadcast
`TaskbarCreated`); пока `Hidden` там ничего не гарантирует, компенсация живёт здесь.

Profit Monitor использует один monotonic pending create request: startup restore не активирует
окно, а пользовательский open может повысить уже ожидающий request до foreground, но не создать
второе окно. Native create выполняется только после настоящего timer yield и повторной проверки
shutdown; manual close очищает только совпадающий `WindowId`, сохраняя reopen-флаг во время quit.

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
