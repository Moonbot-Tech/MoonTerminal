# MoonTerminal Architecture

Дата актуализации: 2026-08-09.

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

## Ручная торговля: группа задаёт видимые параметры

У ручного ордера два источника настроек с намеренно разной областью владения:

- группа окна (`GroupConfig.trade`) хранит всё, что трейдер видит в toolbar: шесть размеров
  F1-F6 в USD-эквиваленте, выбранный размер, основной TP и его режим, S1-S6 и выбранный слот,
  SL с флагом включения и Stop Market;
- ядро хранит невидимые и зависящие от аккаунта параметры: leverage, manual strategy и прочие
  настройки исполнения.

Этот контракт одинаков для Main, вкладок AddToChart и вынесенных chart-окон. При клике на любом
чарте терминал определяет группу по целевому ядру, берёт именно групповые значения, переводит
USD-сайз в базовую валюту через текущий base/USD rate и при отсутствии корректного курса
отказывается ставить ордер. Переключение активного ядра не меняет toolbar.

Moonbot применяет TP/SL к новому ордеру из полного `ClientSettings`, поэтому терминал
синхронизирует групповой exit-набор во все ядра группы. Все полные изменения
`ClientSettings` — включая manual strategy и blacklist — проходят через один per-core
последователь. Ручной ордер является барьером: сначала ядро должно прислать echo нужного
группового поколения, затем уходит order command; следующее поколение не может его обогнать.
`use_market_stop` дополнительно передаётся прямо в `NewOrderParams`.

Схема v16 намеренно не переносит старые per-core размеры: те значения были в базовой монете
(например, `0.01 BTC`) и не могут безопасно стать долларами. Каждая группа начинает с явных
USD-пресетов `50, 100, 250, 500, 1000, 2500`, F3; toolbar обозначает их как `Size, USDT eq.`.
Схема v17 сразу создаёт для каждой группы нейтральное локальное поколение выходов: TP `0%` в
Scalp-режиме, S1-S6 `0%`, SL `0%` выключен, stop-market выключен. Настройки ядра никогда не
засеивают эти поля. Открытый Settings preview получает каждую toolbar-правку одновременно с live
config, поэтому последующий Save не откатывает видимые значения.

## Порядок ядер (core order)

Набор ядер показывается пользователю в десятке мест — селектор в шапке, фильтры «Ордеров»,
«Активов», «Состояния ядер», «Отчёта», «Аналитики», источник «Скринера» и «Лога», дерево
«Стратегий», дерево и списки в настройках. Порядок во всех этих местах ОДИН и задаётся
пользователем: `CoreSortMode` (`moon-core/src/config/servers.rs`) — по имени (алфавит, **режим
по умолчанию**), по добавлению «сначала старые» или по добавлению «сначала новые»; хранится в
`settings.toml`.

Правила, которые легко нарушить незаметно:

- **Любой список ядер строится через `core_order`** (`moon-ui-gpui/src/core_order.rs`):
  `CoreOrder::{from_sessions, from_db}` возвращают `OrderedCores`, чьё поле приватно — собрать
  такой список в обход модуля нельзя. Если строки списка богаче пары `(id, имя)` —
  `CoreOrder::sort_by`. Функция ранга приватна намеренно: с ней порядок можно было бы забыть
  применить, и ничто бы не упало.
- **Порядок НЕ кэшируется** — ранжирование идёт на рендере. Кэшированный порядок протухает в
  открытом окне при смене режима, а инвалидировать нечего, если он нигде не хранится. Панели,
  которые кэшируют СТРОКИ (`assets`, `core_status`), подмешивают `CoreId` в сигнатуру кэша.
- **`SessionManager::sessions()` — всегда порядок КОНФИГА**, не порядок подключения: вставка
  идёт по рангу (`session/lifecycle.rs`), поэтому ядро, выключенное и включённое обратно,
  возвращается на своё место. Режим сортировки сюда не проникает — его применяет UI.
- **`uid` выдаётся из durable-счётчика** `SettingsFile::next_uid`, а не как «максимум + 1»:
  иначе uid удалённого сервера достаётся новому вместе с его историей.
- **Счётчик поднимается до «пола» на каждом старте** — максимального uid, который когда-либо
  видело любое долговременное хранилище: `reports.sqlite` и `strategies.sqlite` (по три таблицы
  с ключом `core_uid`), плюс `layout.toml`, `charts.json` и `figures.json`. Счётчик появился
  только в схеме v15, а до неё засеивался ВЫЖИВШИМИ серверами, тогда как строки удалённого
  никто не чистит. Пол считает `startup::observed_uid_floor` и передаёт в `AppConfig::load`
  числом — зависимости «конфиг → БД» не возникает. Что легко сломать:
  - **Применять пол надо ВНУТРИ загрузки, а не после неё.** `AppConfig::load` сама раздаёт uid
    записям с `uid == 0` и тут же сохраняет их, поэтому поднятие после возврата опаздывает
    ровно на то столкновение, которое предотвращает. По той же причине пол применяется на
    ВСЕХ пяти ветках загрузки, а не только на `servers.enc`: легаси-миграции и «конфиг не
    найден» — как раз те случаи, когда счётчика нет, а хранилища полны.
  - **Забыть пол на новой ветке нельзя — не соберётся.** Счётчик это тип `config::UidCounter`
    с приватным полем и единственным конструктором `UidCounter::new(persisted, uid_floor)`;
    `AppConfig` намеренно НЕ выводит `Default`, его место занял `AppConfig::blank(uid_floor)`.
    Оба закрыты в `config`, так что снаружи конфиг без пола не построить вовсе. Гарантия ровно
    в том, что пол НАЗВАН, а не в том, что он полон: нечитаемое хранилище даёт `None` и
    неотличимо от отсутствующего. Соседний `uid_counter/tests.rs` держит запреты, которые
    система типов выразить не может (никаких `Default`, serde, `From<u64>`, `Deref`, `Copy`),
    читая исходник крейта.
  - **Читать хранилища надо ПОСЛЕ миграций путей.** `layout.toml` и `charts.json` переезжают в
    `cfg/` через `migrate_flat_to_cfg`, поэтому `startup` вызывает миграции до чтения.
  - **Пробник отчётов открывает базу только на чтение** (`db::open_readonly`, не `open_reader`,
    который открывает на запись): он работает до `spawn_writer` и был бы единственным
    соединением, а закрытие пишущего соединения запускает чекпоинт WAL — на главном потоке,
    до первого окна.
  - Удаление ядра историю НЕ чистит: на пенсию уходит номер, а не данные. Нечитаемое хранилище
    вклада не даёт — пол только растёт, так что это best-effort, а не гарантия.
- **Повреждённая реплика отчётов восстанавливается только после uid-floor и до writer.**
  `db::report_recovery::prepare` сначала берёт межпроцессный lease, затем использует ограниченный
  полный `integrity_check`. Подтверждённая порча никогда не «чинится» на месте:
  `reports.sqlite`, `reports.sqlite-wal` и `reports.sqlite-shm` копируются в staging, проверяются
  по размеру и SHA-256, получают versioned metadata и публикуются одним rename каталога в
  `data/damaged-reports/`. Только опубликованный снимок разрешает атомарно переименовать оригиналы
  в его подпапку `originals/` и создать чистую реплику; исходные байты не удаляются. Приватный
  `ReportWritePermit` не даёт вызвать `spawn_writer` в обход этого порядка, а тот же lease
  закрывает доступ readers и ручному VACUUM из второго процесса. Само владение lease ещё не
  разрешает доступ: gate открывается только после результата `Ready` или полностью завершённого
  `Recovered`, поэтому `Blocked`/`Failed` не оставляют обходного read-write пути.
  - Маркер `finalized` делает перенос оригиналов возобновляемой транзакцией: после сбоя следующий
    запуск находит уже перенесённые члены, сверяет их с опубликованными хешами и заканчивает
    операцию. Отличающийся файл остаётся сохранённым, writer остаётся выключенным.
  - Новая подтверждённая порча в течение 24 часов после успешной замены включает circuit breaker:
    текущий комплект остаётся на месте, чтобы файловая или синхронизационная проблема не создала
    бесконечную цепочку копий.
  - Фоновая проверка остаётся после запуска. Если она, reader или ошибка writer публикует порчу уже
    во время сессии, общий barrier не позволяет начать последующий ACK: writer прекращает
    retry-loop, а ядро сможет прислать неподтверждённый batch снова после чистого восстановления.
    Не-коррупционные постоянные ошибки тоже ограничены по числу retry-rounds и не держат канал
    заблокированным бесконечно.
  - `CheckFailed` не равен `Damaged`: неубедительная проверка ничего не удаляет. Базы стратегий,
    предупреждений и кэша рынка в этот протокол не входят.
- **Оба режима «по добавлению» делят один `insertion_key`** и потому являются точными зеркалами
  друг друга. Ключ инъективен (`uid`, а при `uid == 0` — «новейший из возможных», плюс `id` как
  тай-брейк), так что разворот не опирается на стабильность сортировки, а строка, которую сейчас
  добавляют в Настройках, оказывается первой в «сначала новые» и последней в «сначала старые».
- Смена режима — презентационная: она нейтрализована в `AppConfig::structural_sig`, реконнекта
  ядер не вызывает. Порядок `Vec<ServerConfig>` при этом остаётся в сигнатуре — из него строится
  `SessionManager::config_order`, решающий, куда вставить переподнятую сессию.

## Рабочие пространства Classic и Auto

Workspace mode is selected independently for each group window. `layout.toml` owns the durable
per-group `workspace_mode_by_group`, `auto_workspace_core_by_group`, and
`auto_workspace_tab_by_group` maps, plus one application-wide Auto rail width. An absent core UID
means Overview. A legacy or malformed mode resolves to `Classic`; a stale UID remains part of
`WindowLayout::max_core_uid` but resolves to Overview until a live core returns to that group.

Runtime-состояние намеренно отделено от layout:

- `WorkspaceFocus` хранит последнее живое Auto-окно, с которым взаимодействовал пользователь или
  из которого открыл `Analytics`/`Strategies`. Это process-lifetime ownership singleton-инструментов,
  а не настройка для следующего запуска; уход владельца в Classic, закрытие или перестройка окна
  очищают невалидный focus.
- `WorkspaceRevision` публикует изменения режима, выбранного ядра, singleton-владельца, состава
  групповых окон и конфигурации. Кэшированные и асинхронные потребители наблюдают именно этот
  revision, а не надеются на случайную перерисовку Shell.

Classic сохраняет прежнюю семантику полностью. Локальные фильтры панелей и singleton-окон остаются
их собственностью и не переписываются при входе в Auto. Карта
`active_trade_core_by_group` тоже не меняется: выбранное в Auto живое ядро лишь временно становится
`active_trade_core`, а Overview использует сохранённый Classic core или прежний fallback. Поэтому
ручные команды всегда имеют одно ядро и Overview не превращает их в broadcast; header/chart
producers в Auto не записывают временный выбор обратно в Classic.

Для групповой панели effective scope вычисляется на чтении:

- Classic — её сохранённые All/explicit core filters;
- Auto Overview — все доступные ядра этой группы;
- Auto core — ровно одно выбранное доступное ядро.

Auto показывает соответствующий selector как pinned/disabled, но не копирует scope в локальное
поле. Валидность сохранённых Classic-фильтров проверяется по полной конфигурации/живому составу, а
не по временно узкому Auto scope. `Detects`, например, продолжает ingest и cursoring по всей группе
и фильтрует только представление, чтобы переключение ядра не теряло и не переигрывало события.

`Analytics` и `Strategies` используют effective scope последнего живого `WorkspaceFocus`; без него
они возвращаются к своим сохранённым фильтрам. Выбор, детали и изменяющие команды `Strategies`
ограничены видимым effective core, а group-owned `open_goto` повторно проверяет текущий Auto scope
и отказывает, не меняя rail selection. Исключения намеренные: standalone `Report`, открытый из Analytics,
сохраняет свой явный `ReportScope`; global `Assets`, Profit Monitor и Screener остаются
application-wide aggregate surfaces. Групповой status bar и общие счётчики rail тоже не сужаются
выбранным ядром.

Смена scope может произойти между кликом, ответом фонового запроса, native picker/dialog и отправкой
команды. Поэтому асинхронный результат обязан нести identity текущего scope/revision и приниматься
только при совпадении: Report query сравнивает собственные sequence и filter, Analytics
инвалидирует прежние query identities, а Report export после выбора пути сравнивает generation
группового workspace и effective core IDs. Отложенные, изменяющие и destructive пути
(`Strategies`, tuner Save/Copy, order edit, Assets Market Sell, wallet transfer, Report export,
Report delete/restore, Analytics purge) повторно проверяют effective core непосредственно перед
записью или отправкой;
длинный purge делает ту же проверку
между ожиданиями и каждой следующей командой. Отказ не переносит действие на новое ядро: stale
Market Sell не отправляет команду, показывает warning и закрывает confirmation; stale wallet
transfer очищает pending dialog без команды; stale Report export пишет cancellation в лог и не
создаёт файл; purge остаётся в видимом failed state.

Многоцелевые действия не сужаются молча до оставшихся видимыми ядер. `Strategies` сохраняет точный
план Start/Stop, Apply или Delete, а Analytics Tuner Save/Copy — generation и полный ordered target
set. Если хотя бы одна цель, payload или scope изменились до dispatch, отменяется весь batch до
первой команды; сохранённые Classic drafts и staging при этом не очищаются.

Each group keeps one `DockArea`, but Classic and Auto have separate authorities. Classic persists
`docks.json` and `detached.json` as one transaction through `window-state.pending.json`; startup
replays an incomplete snapshot before creating windows and deduplicates repeated `(group, panel)`
detached specs. On Auto entry, Shell retains the local Classic `DockNamedLayout`: name-based
topology plus active-tab, zoom, and split-size metadata. It does not store opaque panel `Rc`
identities; ordinary exact identities survive in the live `DockArea` while the name-only layout is
transformed. Auto applies the shared topology from `auto_dock.json` to those live instances. The
shared file contains only split structure, sides, sizes, and tab order: it never transfers group
IDs, panel payload, active tabs, zoom, or `Rc`s between windows.

Auto entry selection is a third, deliberately narrow authority. `auto_workspace_tab_by_group` in
`layout.toml` remembers the last activated eligible top surface per group: `ChartTabs`, `Report`,
`Assets`, `CoreStatus`, `Log`, `Alerts`, or `Detects`. `Orders` is the separate lower surface and
`News` is Classic-only. Missing, unknown, or stale `News` values resolve to `Report` without being
rewritten merely because fallback was needed. Real Auto activations, including a programmatic
`ChartTabs` reveal, update this preference; Classic activity remains in its retained
`DockNamedLayout` and `docks.json`. During mode or shared-topology application, Shell holds its
suppression guard until a deferred callback runs after queued Dock events, so programmatic
`PanelActivated` and `LayoutChanged` delivery cannot overwrite either Auto preference or topology.

Without `auto_dock.json`, the first shared topology is a vertical operations preset: the flexible
upper tab stack contains pinned-leading `ChartTabs`, `Report`, `Log`, and the other eligible
operational surfaces, while the single lower `Orders` surface receives four extra table rows of
height. Auto entry activates the saved eligible per-group tab after installing the topology, with
`Report` as the deterministic fallback. The first user reorder, split, or resize replaces the
preset with the shared topology; the Classic layout never participates in that update.

Auto permits reorder, split, and resize. `ChartTabs` is pinned first, visually separated from the
operational tabs, and cannot be moved or bypassed by a drop. Ordinary panel close/detach and chart
detach remain disabled independently of structural editing. Auto contains every operational
surface except `News`. Before applying even a stale topology that names `News`, Shell extracts every
docked `News` occurrence and holds the first exact identity in `Shell::classic_only_panels`;
therefore the name is absent from Auto and cannot be resurrected by the shared topology. Returning
to Classic supplies that retained identity while applying the saved name-only `DockNamedLayout`,
restoring its selection, zoom, split placement, and local view state. Classic detached panel specs
and geometry are preserved while Auto is active, but detached `News` is specifically excluded from
temporary Auto-only panel construction, so no dock clone appears. Existing detached chart windows
remain open. Auto never writes any of this Classic state to `docks.json`.

The Auto rail and `DockArea` use the standard `MoonResizablePanelGroup`. Backend publishes and
persists one draggable global rail width, while each window applies its own fit clamp without
rewriting that preference. The virtualized rail is a tree: Overview is the root, exchanges are
section headings, and canonically ordered cores are indented leaves with branch connectors and
status dots. Sections are keyed by venue identity — see the venue directory below — not by the name
a core reported, so two cores on one venue always share a heading however their builds spell it.
Logos appear only on known exchange headings; unknown exchanges keep their text label
without a placeholder, and core rows never inherit a logo. Entering Auto starts process-wide
single-flight logo prewarm on the background executor. Each Shell publishes its own UI-thread ready
edge and repaint; before that edge the rail performs no loading resolution, and afterward it
resolves each distinct exchange once while flattening the list, outside the virtual row closure.
Full and Compact retain truncating labels, while Icon uses a known exchange logo or an unknown-brand
short label and a reduced core-leaf spacing budget. Tooltips preserve complete exchange, core, and
status meaning at every density. The terminal group remains the authority for selecting its core or
activating another group window, not a visual grouping key.

### The venue directory

A core publishes two things about the exchange it is connected to: a platform ordinal
(`ServerInfo::exchange_code`, the MoonBot `TBotPlatform` value) and a free-form UI name whose
spelling belongs to that core build — `Binance Quarterly`, `FBybit`, `Gate.io`. The ordinal is the
identity; the name is a caption. `moon_core::venue` is the one table that turns the ordinal into
everything the terminal decides about a venue: its brand (which logo), its market kind
(Spot/Futures/Quarterly), the naming family `symbol::parse` spells its markets with, and which order
book its provider pulls. `Exchange::from_code` and `session::orderbook_kind_for_exchange` are
projections of that table rather than tables of their own.

A core's venue arrives once per connection, on `FeedMsg::Identity { id, dex, reported }`, and
`SessionManager` retains one `CoreVenue` per identified core. `core_venues()` hands that map out by
reference, so no consumer rebuilds it — or reaches back into a client snapshot for a caption — while
rendering. Every core list (the pickers, the Profit Monitor, the rail, Log sources, Connections,
detection cards) groups by `ExchangeId` and captions through `controls::venue_section_label`.

`reported` is carried only for an ordinal newer than this build, the single case the directory
cannot name. A venue with neither a known ordinal nor a caption of its own is identified but not
`is_nameable()`: it still elects a market-data provider — the synthetic feed is exactly that case —
while every core list folds it into the shared "not identified" section rather than giving it a
section of its own headed with that same wording. Recognizing an exchange by matching its reported
name is what left Binance COIN-M (code 6, reports `Binance Quarterly`) with no brand; a contract
test in `tests/theme_contract/naming.rs` sweeps the whole crate to keep `reported` out of every
surface but the formatter.

Все три layout-класса — `layout.toml`, общая `auto_dock.json` и совместный Classic snapshot —
сериализуются одним persistence worker. Live GPUI-контур только передаёт immutable snapshots и
принимает acknowledgements: accepted enqueue снимает dirty, а новая мутация или failed ack
выставляет его снова. Quit ставит
финальный полный snapshot за уже выполняющейся записью, join'ит worker и использует синхронный
fallback только для классов, которые worker не смог подтвердить.

Интерактивные таблицы без собственного хранилища сохраняют выбранную колонку и направление в
`WindowLayout.table_sorts` через `persistence/table_persist.rs`. Ключ включает устойчивый id
таблицы и host-контекст (`:dock` / `:win`); каждое сохранённое имя валидирует сам panel по своим
текущим sortable/visible колонкам, а отсутствие или устаревший ключ оставляет исторический default.
Orders, Assets, Core Status Flat/By IP, Screener и Analytics Tuner By coin используют этот общий
контракт. Alerts сохраняет sort вместе с dock state, Report — в SQLite, а Analytics strategy list,
Profit Monitor и Settings core order сохраняют свои отдельные layout/settings preferences; общий
map не дублирует эти authorities. Все compare-then-dirty записи уходят через существующий
debounced layout worker и финальный quit flush.

`ReportPanel` is a lightweight GPUI entity: its SQLite connection/schema, core list, columns, and
saved display preferences load on the background executor and publish only into a still-live panel.
Independent revision counters prevent a late result from replacing a value the user changed and
then changed back. Sort, comment-pane, and visible-column writes open a worker-owned connection
outside GPUI, serialize through the shared lock, and reject stale per-preference sequences before
writing; sort key and direction use one SQLite statement. Natural-width cache identity includes
scale, live locale, and the exact resolved font, so language or font changes cannot reuse stale
header widths.

The group-owned `AutoCore` Report retains a separate strategy-name mask beside the exact strategy
selector. It is applied only to the current effective Auto core, combines conjunctively with exact
keys, and uses full Unicode case-folded literal substring matching through the shared
`ReportFilter`, so rows,
totals, stale-result identity, and export cannot diverge. Strategy catalog discovery clears both
strategy predicates and therefore never self-locks under either filter. The Auto core trigger takes
its current full name from the live group roster, falls back to report metadata only when offline,
and exposes the complete text through a fitted trigger and tooltip. The toolbar consists of small
semantic chrome sections whose dividers travel with the following section; exact strategy, mask,
and detached range bounds wrap independently instead of forcing horizontal scrolling.

For a group-owned `AutoCore` Report only, `core_name` is contextually unavailable because every row
already belongs to the selected core. This is a display lens, not a persistence mutation: the raw
visible set, `app_meta`/`layout.toml`, sort state, and widths remain untouched. The grid, Columns
menu and its All action, selection copy, and visible-columns export all use the same effective lens;
the explicit all-columns export still uses the full runtime schema. Auto Overview, Classic, and
standalone Report expose `core_name` according to the unchanged saved preference.

## Репликация отчётов: чекпойнт и карта живых строк

Реплика `orders_rep` догоняет ядро по возрастанию `newRecID`, поэтому обычный catch-up физически
не способен увидеть, что старая строка была скрыта, восстановлена или вычищена retention'ом, пока
терминал был офлайн. Протокол закрывает это компактной **картой живых строк**, и весь порядок
операций держится на одном правиле: **чекпойнт пишет только та транзакция, которая применила
карту.**

- **Стартовое состояние ядра** — `rep::ReportStart`: `Fresh` (локальных строк нет),
  `Resume(n)` (строки есть, epoch ещё не сохранён) или `Checkpoint{epoch, next_from_rec_id}`.
  Чекпойнт лежит в `app_meta` под `rep_epoch_{core_uid}` / `rep_next_{core_uid}`, и feed стартует
  через `sync_from`, `sync(resume)` или `sync(fresh(All))` соответственно. Пара с epoch нужна
  потому, что пересозданную на ядре базу иначе не отличить: новая может уже перерасти старые
  номера, и чисто числовой курсор этого не заметит.
- `Resume(n)` — **одноразовый миграционный путь** для реплик, записанных до появления чекпойнтов.
  Полная перекачка вместо него стоила бы существующему пользователю сотни МБ. Принятый остаточный
  риск: на этом единственном проходе epoch'а ещё нет, детект пересоздания падает на эвристику
  high-water, и ядро, пересоздавшее базу офлайн так, что новая уже переросла старый максимум
  `newrecid`, останется незамеченным — строки с совпадающими id сохранят значения мёртвой базы,
  а карта живых строк это не чинит (она несёт видимость, не содержимое). Отсутствующие в новой
  базе id она всё же скроет, а начиная с первого сохранённого чекпойнта пересоздание ловится по
  epoch. Кому это важно — сброс реплики форсирует полный sync.
- **Сохранённый `next_from_rec_id` никогда не поднимают до `max(newrecid)+1`.** Живые апсерты
  штатно ложатся ВЫШЕ чекпойнта между двумя catch-up'ами, так что локальный максимум обычно
  больше; взять максимум — значит пропустить страницы между ними. Перезапрошенные строки
  идемпотентны по `(core_uid, newrecid)`. Обратное тоже верно: чекпойнт без локальных строк
  отбрасывается.
- **Сброшенный бит скрывает строку, а не удаляет её** (`deleted=1`): карта не отличает
  soft-delete от retention, а скрытую строку должен уметь вернуть последующий restore или апсерт.
  Поэтому же живой `RowUpsert` без поля `deleted` пишет `deleted=0` — ровно так его читает сам
  moonproto, накладывая живые события на карту в полёте.
- `rep::apply_alive_map` сканирует **только локальные строки ядра** в пределах `covered_up_to`
  (первичный ключ обслуживает и фильтр, и порядок — ни сортировки, ни нового индекса), копит
  потоком лишь расхождения в свёрнутые диапазоны и применяет их двумя `UPDATE`. `covered_up_to` —
  это high-water ядра, он бывает в миллионах; проход по нему заблокировал бы единственный writer.
- **Без колонки `deleted` чекпойнт не сохраняется**: записать видимость некуда, а отметка
  «карта применена» навсегда лишила бы ядро повторной попытки.
- **Любой сброс реплики чистит чекпойнт вместе со строками** (`rep::reset_replica`) — и путь
  `database_recreated` у страницы, и `DatabaseRecreated` у карты. Иначе следующий старт взял бы
  старый epoch, снова стёр частично собранную реплику и зациклил полную перекачку.
- Порядок отказов: страница коммитится до своего `page_applied`, `SyncComplete` приходит после
  последнего ACK, `DbMsg::SyncComplete` и `DbMsg::AliveMap` идут одним FIFO к одному writer'у.
  Откат батча не двигает ни чекпойнт, ни опубликованное стартовое состояние — следующее
  соединение просто повторяет catch-up.

## Пересчёт в USDT: два режима

Отчёт и Аналитика умеют показывать котируемые деньги в USDT двумя способами. Режим один на всё
приложение (`SettingsFile.report_valuation_mode`, читается через `Backend::valuation_mode`), потому
что два окна с разными режимами показывали бы один и тот же период двумя одинаково «правдивыми»
итогами.

Селектор живёт в **Настройках, вкладка «Общие»**, а не на панелях Отчёта и Аналитики: значение по
умолчанию отвечает на вопрос «сколько сделка стоила, когда закрылась», и это правильный вопрос
почти для всех — экспертной настройке не место в рабочем тулбаре. Сохранение вызывает
`Backend::apply_valuation_mode`: оно поднимает флаг спроса воркера и публикует `report_revision`.
Публикация обязательна — смена режима не двигает ни одного ряда, поэтому ни одно поколение сама по
себе не сдвинется, и открытые окна продолжили бы рисовать прежние числа под новым ярлыком.

- **По курсу сделки** (по умолчанию) — `valuation.sqlite`, курс той минуты, когда сделка закрылась.
  Это исторический P&L.
- **По текущему курсу** — `db::valuation::current`, снимок «ординал валюты → цена в USDT» в памяти.
  **Никогда не персистится**: `ALGORITHM_VERSION` входит в оба первичных ключа кэша, так что общей
  строки у режимов быть не может, а сохранённый «текущий» курс после перезапуска — это устаревший
  курс под ярлыком свежего.

Обе ветки строят один и тот же `CoverageSql` через `valuation::projection(mode, attached, ...)`, так
что поколоночные значения Отчёта, итоговая строка, экспорт, сводка Аналитики, календарь, группы и
тюнер конвертируются по одному правилу. Курсы попадают в SQL литералами, а не плейсхолдерами:
готовая строка переиспользуется как FROM-фрагмент вызывающими, которые связывают только свой
диапазон дат.

Известные ограничения текущего режима — документированные свойства, не баги; оба названы в подсказке
под селектором (`general.valuation_mode_hint`):

- Курсы берутся со **спота Binance/Bybit**, а не с биржи, где шла торговля.
- **Тюнер тоже переоценивается.** Пороги подбираются по переоценённой истории, то есть оптимизируется
  не то, что фактически произошло. Это осознанное решение пользователя, а не недосмотр.
- Валюта без свежего курса не конвертируется вовсе: после `FRESHNESS_MS` (10 минут) курс перестаёт
  считаться текущим, и итог честно распадается по валютам — так же, как при неполном историческом
  покрытии. Валюта, у которой все маршруты постоянно отсутствуют, считается `unavailable`; валюта,
  которую просто ещё не успели опросить, — «в процессе».

Воркер обновляет снимок стадией `ValuationStage::CurrentRates`, **по одной валюте за оборот цикла**
(холодный проход по валюте стоит до четырёх последовательных маршрутов по 15 с) и **только пока
`ValuationHandle::set_current_wanted(true)`** — при выключенном режиме в сеть не уходит ни одного
запроса. Публикуется через `generation`/`commit_dirty` (данные), а не через `status_revision`
(здоровье): счётчик здоровья намеренно не вызывает перезапрос.

Публикация стоит пользователю дорого: на сдвиг поколения каждый открытый Отчёт и окно Аналитики
делают полный перезапрос и пересборку дерева. Поэтому обновление стоит на двух ограничителях —
`CURRENT_REFRESH_MINUTES` (5, половина окна свежести, так что один провалившийся проход не может
дать курсу протухнуть) и `CurrentRateState::renders_differently`: снимок сохраняется всегда (по нему
считается свежесть), а поколение двигается, только если изменилась цена, её происхождение или набор
недоступных валют. `fetched_at_ms` в сравнение намеренно не входит — он двигается каждый проход и не
попадает ни в одну цифру на экране, так что привязанная к доллару котировка не стоит ни одного
перезапроса. Истечение срока при этом видно как уменьшившийся набор курсов, поэтому отсечка
по-прежнему доходит до экрана вовремя.

## Резервные копии настроек и стратегий (`backups/`)

`moon_core::backup_store` — единый внутренний владелец файлового lifecycle обоих типов снимков:
он проверяет корень, выдаёт уникальные `.incoming-*` каталоги, атомарно публикует готовый каталог
и удаляет старый только после проверки полного состава файлов. Доменные модули не дублируют эту
логику: они задают содержимое, полную грамматику имён и свою политику хранения.

`backups` (`moon-core/src/backups.rs`) — один wall-clock coordinator для обоих доменов. Он привязан
к 12:00 UTC, на обычном старте сразу догоняет последний наступивший полуденный слот и затем каждый
раз заново вычисляет задержку до следующего полудня. Настройки и стратегии выполняются в отдельных
job-потоках: длительная SQLite-копия или повтор после ошибки не задерживает второй домен. FireTest
coordinator не запускает.

`config::backup` (`moon-core/src/config/backup.rs`) складывает в
`<data_dir>/backups/settings/<UTC timestamp>/` копии двух невосстановимых файлов — `servers.enc`
(ключи API) и `cfg/settings.toml` (группы, галки ядер, счётчик uid). Кнопка «Сохранить» снимок больше
не создаёт: все обычные записи используют `AppConfig::save()`, а coordinator создаёт не более одной
канонической копии на UTC-полуденный период. Обе записи конфига и обе операции копирования защищены
одним pair-lock, поэтому снимок не смешивает поколения файлов. Миграция схемы перед перезаписью
использует тот же наступивший дневной слот как safety barrier, а не отдельную серию снимков. Старые
снимки прежних версий прямо в `backups/` остаются на месте: приложение не переносит и не удаляет их.
Перед первой из двух замен `save()` ставит `.config-pair-pending` и снимает marker только после
второй; незавершённая пара не попадает в backup, а следующий обычный запуск пересохраняет собранный
в памяти конфиг целиком. Кроме process-lock, snapshot повторно читает оба источника после сборки:
это обнаруживает замену из второго экземпляра терминала и оставляет слот на повтор вместо mixed copy.

`strat_db::backup` хранит консистентные SQLite-копии в
`<data_dir>/backups/strategies/<UTC timestamp>/strategies.sqlite`. Если существующая база содержит
хотя бы одну строку стратегии, startup catch-up разрешён сразу. На чистой базе плановый снимок ждёт,
пока writer успешно применит по одному полному набору от каждого активного ядра с включённым feed
стратегий; пустой набор тоже считается доставленным. Feed продвигает свой delivery-cursor только по
подтверждению успешного SQLite commit от writer-а: переполненная очередь или ошибка записи оставляет
тот же набор на секундный retry даже без нового MoonProto-события. Изменение состава ядер через
Настройки обновляет барьер, а финальный rename выполняется под коротким topology claim, поэтому новый
core не может появиться между проверкой поколения и публикацией. После временной ошибки готового
источника backup-job повторяет просроченный слот через пять минут. Ручная кнопка на вкладке
«Хранилище» использует тот же атомарный механизм, но всегда создаёт отдельный снимок и не подменяет
обязательный полуденный.

Оба домена сохраняют все ручные и плановые снимки последних семи UTC-полуденных периодов. Старые
`data/strategies-backup-*.sqlite` и прежние корневые settings-снимки остаются нетронутыми.

Что легко сломать незаметно:

- **Имя каталога `YYYYMMDD-HHMMSS` (UTC) — это контракт, а не оформление.** Двоеточие запрещено в
  именах файлов Windows, а фиксированная ширина даёт «лексикографический порядок = хронологический»,
  на чём и держится отсечение старых копий. Сортировка по mtime НЕ годится: копирование файла и
  облачная синхронизация его переписывают. Штамп строит `util::time::utc_stamp_compact` — из частей
  даты, а не нарезкой готовой строки, потому что `{y:04}` задаёт МИНИМАЛЬНУЮ ширину.
- **Снимок публикуется целиком.** Файлы собираются в каталоге `.incoming-*`, который намеренно не
  проходит распознавание, и лишь потом весь непустой каталог публикуется одним `fs::rename`.
  Плановый снимок имеет каноническое имя полуденного слота: если два экземпляра терминала
  одновременно собрали его, завершённый победитель считается общим успехом, а не ошибкой второго.
- **Чистка не должна уметь удалять чужое.** Корень-симлинк пропускается, тип потомка берётся
  через `DirEntry::file_type` (не разыменовывает ссылки), удаляются только ожидаемые имена файлов,
  а сам каталог сносится НЕрекурсивным `remove_dir` — поэтому посторонний файл внутри снимка
  сохраняет и себя, и снимок.
- **`backups/` НЕ входит ни в один из миграционных списков `paths.rs`.** Обе миграции работают
  через `fs::copy`/`fs::rename` и в каталоги не рекурсируют, но и не отказываются от них: имя
  каталога в списке молча увезло бы всё дерево снимков туда, где `backups_dir()` его не ищет.
  Прецедент — `logs/`, которого там нет по той же причине. Закрыто тестом.
- **Нечитаемый `settings.toml` НЕ перезаписывается.** `toml_io::ConfigLoad` отличает «файла нет»
  от «файл не прочитался»; во втором случае автоматическое пере-сохранение по устаревшей версии
  схемы отменяется. Иначе временная ошибка чтения (права, шара, невыгруженный облачный
  плейсхолдер) превращалась бы в безвозвратную замену живого конфига дефолтами.

## UI Components

Приложение зависит от `Moonbot-Tech/MoonUI` и использует компоненты через `moon_ui::*` /
`moon_ui::components::*`. Прикладные панели терминала не должны заново рисовать общие UI-паттерны
вручную, если в MoonUI уже есть подходящий компонент или близкий Longbridge-наследник.

Правило адаптации:

- если компонент Longbridge уже даёт нужную механику, но тема/геометрия/состояния не соответствуют
  Moonbot design, править или оборачивать его нужно внутри MoonUI;
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
панели" живут в `tests/theme_contract/`, а не внутри runtime-сценария.

## Окна

Терминал использует собственную шапку и borderless/CSD поведение. Проверять отдельно:

- Windows: restore bounds на multi-monitor/DPI.
- macOS: Metal toolchain и `.app` запуск из GUI session.
- Linux X11/Wayland: отсутствие второй системной шапки, Secret Service для encrypted config,
  стабильность surface/present.

## Локальная Разработка

Публичные `Cargo.toml` держат git-зависимости на `Moonbot-Tech/MoonUI` `branch = "master"`.

`Cargo.lock` **коммитится**, и это заморозка сторонних версий: скомпрометированный или просто
неожиданный релиз чужого крейта не может попасть в сборку сам собой. Политика в четырёх пунктах:

1. Сторонние версии двигаются только осознанным коммитом.
2. MoonUI остаётся rolling: CI на каждом прогоне делает точечный
   `cargo update -p moon-gpui -p moon-gpui-platform -p moon-ui`, локально это `make update-moon-ui`.
3. MoonProto двигается ТОЛЬКО вручную (`make update-moonproto`) отдельным коммитом; CI его не трогает.
4. Каждая собирающая job в CI сначала проверяет замок против манифестов (`cargo fetch --locked`),
   затем обновляет MoonUI, затем падает, если это обновление сдвинуло хоть что-то кроме MoonUI —
   точечный `cargo update` по документации консервативен, а не хирургичен, и новый MoonUI может
   потянуть за собой чужую версию. Такой случай должен быть красным PR и осознанным коммитом.

`EmbarkStudios/cargo-deny` отдельной блокирующей job сканирует коммитнутый замок: advisories,
дубли и белый список git-источников (`deny.toml`).

Reproducible build по сторонним зависимостям — да; свежесть MoonUI — отдельный явный шаг.

Каждый бинарь пишет в лог build stamp:

```text
build: moonterminal=<git-sha>[+dirty] release_base=<stable-git-tag|unknown> moonui=<git-sha|local:git-sha>[+dirty]
```

Перед тегом релиза: сдвиньте MoonUI осознанно, закоммитьте замок, дождитесь гейтов на ЭТОМ коммите
и только потом ставьте следующий канонический стабильный тег `vMAJOR.MINOR.PATCH`: новую minor-линейку
начинайте с `.0`, а исправления выпускайте увеличением PATCH. Исторические двухкомпонентные теги
вроде `v0.21` читаются updater-ом как patch zero, но новые такие теги и эквивалентный alias
`v0.21.0` запрещены. `release.yml`
собирает immutable commit этого тега строго `--locked`, проверяет GitHub SHA-256 для Windows
артефакта ещё в draft и только после этого публикует релиз как Latest. В репозитории должна быть
включена release immutability: публикация блокирует проверенные tag и assets; MoonUI там не обновляется.

### Самообновление Windows

Обычный Windows-процесс после `startup::boot` один раз в фоне просматривает ограниченное число
страниц GitHub Releases. Кнопка в шапке появляется только для наибольшего канонического стабильного
тега новее встроенного `release_base`, если релиз immutable, не draft/prerelease, содержит ровно
один `MoonTerminal.exe` и GitHub вернул обязательный `sha256:` digest. Неизвестная базовая версия,
ошибка сети или неполные metadata скрывают кнопку; установка начинается только явным кликом.

`moon_core::update` скачивает exe потоково в уникальный `.part` внутри
`.moonterminal-update/<nonce>/`, ограничивает размер, дважды проверяет SHA-256 через тот же открытый
file handle и только затем публикует staged-файл. Один `UpdateController` принадлежит `Backend` и
наблюдается всеми `Shell`, поэтому разные окна не могут начать параллельную замену.

Скачанный exe запускается скрытым helper-процессом. Helper сначала валидирует versioned manifest,
канонические пути/nonce/hash, открывает handle точного родительского процесса и публикует `ready`.
UI подтверждает прочитанный `ready` отдельным nonce-bound `commit`; без него helper завершается по
deadline и не заменяет exe. Только после `commit` UI вызывает обычный `App::quit`, сохраняя
существующий `on_app_quit`-маршрут, а helper ждёт выхода родителя, использует `ReplaceFileW` с
backup, запускает новый target и ждёт
ограниченные по времени `started` и `healthy`. Новая версия публикует `healthy` сразу после входа
в безопасную часть `startup::run`, но строго до миграций/открытия portable storage: до этой границы
любой сбой завершает только запущенного child, восстанавливает backup и повторно открывает прежнюю
версию с уведомлением; после неё откат старого exe уже запрещён, чтобы не читать новую схему старым
кодом. Helper удаляет backup только после собственного чтения `healthy`. `cfg/`, `data/`,
`logs/`, `backups/` и `servers.enc` в транзакции не участвуют. На macOS/Linux updater отключён.

Граница доверия — HTTPS, immutable GitHub Release и его SHA-256 metadata. Release workflow
сериализует публикацию по тегу; административный `RELEASE_ADMIN_TOKEN` допускается только в
последнем шаге проверки immutable-release и публикации. Это обнаруживает подмену
asset относительно опубликованного релиза, но не является code signing и не защищает от
компрометации владельца репозитория или release-публикатора.

Активная локальная подмена MoonUI через `.cargo/config.toml` переписывает отслеживаемый `Cargo.lock`
(в нём появятся `path`-записи). Восстановите его перед коммитом.

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
