# MoonTerminal Architecture

Last updated: 2026-09-21.

This document describes the terminal's current public architecture and deliberately omits old
experimental migration plans.

## Crates

- `moon-core` — UI-agnostic core: connections, config, sessions, market state, reports.
- `moon-chart` — chart math: time/price view, phase-clean default scale, pan/zoom, axes.
- `moon-ui-gpui` — the `moonterminal` binary: GPUI shell, panels, debug-tools, chart integration.
- `Moonbot-Tech/MoonUI` — external git dependency: standalone GPUI runtime + Moon UI components.

Legacy UI/runtime packages are not active dependencies. New shared UI/runtime changes
must go through MoonUI.

## Rendering

The chart is drawn via a GPU own-pass on top of MoonUI/GPUI:

- Windows: DX11/HLSL.
- macOS: Metal/MSL.
- Linux: native GPUI wgpu backend/WGSL.

This is not the old `egui + wgpu offscreen + readback` and not a shared-texture bridge between
different renderers. CPU readback is not used for the live chart.

Key contract: the chart itself decides whether a frame is needed (`gpu_canvas.frame()`), and can
prepare data for that same frame without a top-down `cx.notify()` of the whole window. Shell/Orders must not
repaint at live-scroll, mousemove, or present frequency.

## Data Path

Every Main coin-chart request carries its exact `(core, canonical market)` atomically. The chart
composes that core's current/open/retained order lines with durable closed trades read from
`reports.sqlite`; the durable query re-enforces the exact core and exact market aliases and never
consults a global active core. Entry and exit markers share the existing userdata layer with order
lines, figures, news, and warnings. Loading, ready, empty, not-ready, and failed history remain
visible without blocking live market data, and the first successful load focuses the selected or
newest closed-trade interval once.

Live/Pause preserves the chosen time zoom. History upload bounds include prefetch, but Y fitting
uses a separate visible interval for live data, fixtures and frozen replay. Candles intersecting
the visible boundary contribute at their displayed timeframe, including on cached repeat reads.

Closed-trade arrows sit at the report timestamps. `ReportStamp` plus
`ReportAxis::stamp_pair_to_utc_ms` feed `chartdx::trade_history_sync::trade_mark` with milliseconds
when the core supplied them and whole seconds otherwise; nothing moves a mark afterwards.

The current live-path left the old constant polling and top-down chart-data push:

- MoonProto events arrive through an event sink with a waker.
- The Backend loop waits for real events/commands via the waker; it is not woken by a timer.
- The visible chart pulls market data through `MarketDataSource` inside `gpu_canvas.frame()`.
- `MarketDataSource` reads `snapshot_versioned()` and advances the consumer cursor only for the
  actually visible chart path.
- `SharedMarketStore` remains a core-owned compatible read-model for other consumers; it is
  not a GPUI entity and not a reason for a top-down render.

Push events remain for UI widgets and rare notifications. The chart data path is pull on the frame tick.
Order events are a separate important contract: MoonProto `OrderEvent::Created/Updated/Removed`
carries an Arc-backed order row at the moment of the event. The terminal builds order-lines from the current
snapshot plus captured event rows, so a short terminal status (`Cancel`, `Fail`, `Done`) is not
lost even if the latest snapshot has already dropped the uid from the live-list.

Order geometry resolves `UseCustomColors`, `BuyOrderColor`, `SellOrderColor` and `OrderLineKind`
from the order's own core's confirmed strategy snapshot and kind-specific schema defaults. Retained orders keep strategy ID/name;
a nonzero ID is authoritative, while an ID-less named row may match a unique name. Missing,
disabled or invalid fields fall back to the current theme's order style. Strategy ARGB alpha
multiplies lifecycle opacity; entry/exit colors apply on both long and short orders, and the pen
pattern applies to all order segments, including protective lines and liquidation. Widths,
markers, zone colors and semantic label colors retain their existing settings. Both the chart
wake signature and the pane geometry cache track strategy and schema revisions, so late data and edits
apply without waiting for an order price change.

## Manual trading: the group owns the visible parameters

A manual order has two settings sources with deliberately different ownership:

- the window group (`GroupConfig.trade`) stores everything the trader sees in the toolbar: the six
  B1-B6 sizes in USD equivalent, the selected size, the main TP and its mode, S1-S6 and the selected slot,
  SL with its enable flag and Stop Market;
- the core stores invisible and account-dependent parameters: leverage and other execution
  settings;
- the manual-strategy choice is TERMINAL state, per core
  (`ServerConfig::manual_strategy`). The order carries the strategy as an explicit `StratID`, so the
  mode itself does not need to be switched on the core, and two terminals on one core can work with different strategies.
  A PAIR is stored: the pinned `id` is the working identifier; the name is an anchor in case the strategy
  is recreated (it keeps the name and loses the number). The name must not be re-resolved before every order:
  Moonbot swaps manual hook-strategies on the fly. The core's live selector (`use_manual_strategy`) takes
  part in neither display nor the order — it moves on its own; a core that has no local
  state yet is seeded once from its snapshot (`tick_manual_strat_seed`).
  Constraint: `id` is unique within one Moonbot, so changing the core's key clears
  the pin and leaves only the name;
- a manual order's stop is by default NOT touched by the terminal (`ManualStratState::mb_logic`, the
  "Moonbot's own logic" checkbox in the MS popup). This is Moonbot's own behaviour: the stop is taken from the strategy, and if that
  points to `UseHookStrategy` — from the hook-strategy, from which the core takes both the stop and the sell price. While
  the checkbox is on, the toolbar SL shows that source's values and is locked, and after the order
  nothing extra is sent; if the source cannot be read (schema never arrived, the named hook is not in
  the snapshot) — the field shows a dash, not a saved number the order will not carry. Clearing the checkbox restores the follow-up send: the visible stop goes out as a second packet
  (`queue_visible_stop`) and overrides what the core applied.

This contract is the same for Main, AddToChart tabs, and detached chart windows. On a click on any
chart the terminal resolves the group from the target core, takes the group values, converts
the USD size into the base currency through the current base/USD rate, and if there is no valid rate
refuses to place the order. Switching the active core does not change the toolbar.

Moonbot applies TP/SL to a new order from the full `ClientSettings`, so the terminal
syncs the group's exit set to every core in the group. All full
`ClientSettings` changes — including the blacklist — go through one per-core sequencer. A manual order is a barrier: first the core must echo the required
group generation, then the order command goes out; the next generation cannot overtake it.
`use_market_stop` is additionally passed straight into `NewOrderParams`.

Schema v16 deliberately does not migrate old per-core sizes: those values were in the base coin
(for example, `0.01 BTC`) and cannot safely become dollars. Each group starts with the explicit
USD presets `50, 100, 250, 500, 1000, 2500`, F3; the toolbar labels them `Size, USDT eq.`.
Schema v17 immediately creates a neutral local exit generation for each group: TP `0%` in
Scalp mode, S1-S6 `0%`, SL `0%` disabled, stop-market disabled. Core settings never
seed these fields. An open Settings preview receives every toolbar edit at the same time as the live
config, so a later Save does not roll the visible values back.

## Core order

The set of cores is shown to the user in a dozen places — the header selector, the Orders,
Assets, Core status, Report, and Analytics filters, the Screener and Log source, the
Strategies tree, and the tree and lists in Settings. The order in all of these places is ONE and is set by
the user: `CoreSortMode` (`moon-core/src/config/servers.rs`) — By name — alphabetical (**the default
mode**), By insertion — oldest first, or By insertion — newest first; stored in
`settings.toml`.

Rules that are easy to break unnoticed:

- **Every core list is built through `core_order`** (`moon-ui-gpui/src/core_order.rs`):
  `CoreOrder::{from_sessions, from_db}` return `OrderedCores`, whose field is private — assembling
  such a list around the module is impossible. If the list rows are richer than the pair `(id, name)` —
  `CoreOrder::sort_by`. The rank function is private on purpose: with it public, the order could be forgotten
  and nothing would fail.
- **The order is NOT cached** — ranking happens at render. A cached order goes stale in
  an open window when the mode changes, and there is nothing to invalidate if it is stored nowhere. Panels
  that cache ROWS (`assets`, `core_status`) mix `CoreId` into the cache signature.
- **`SessionManager::sessions()` is always CONFIG order**, not connection order: insertion
  follows rank (`session/lifecycle.rs`), so a core that is turned off and back on
  returns to its place. The sort mode does not reach here — the UI applies it.
- **`uid` is issued from a durable counter** `SettingsFile::next_uid`, not as "max + 1":
  otherwise a deleted server's uid would go to a new one together with its history.
- **The counter is raised to a "floor" on every start** — the maximum uid that any
  durable store has ever seen: `reports.sqlite` and `strategies.sqlite` (three tables each
  keyed by `core_uid`), `order_traces.sqlite`, plus `layout.toml`, `charts.json` and `figures.json`. The counter appeared
  only in schema v15, and before that it was seeded from SURVIVING servers, while the deleted core's rows
  are never cleaned. The floor is computed by `startup::observed_uid_floor` and passed into `AppConfig::load`
  as a number — no "config → DB" dependency arises. What is easy to break:
  - **The floor must be applied INSIDE the load, not after it.** `AppConfig::load` itself assigns uids
    to records with `uid == 0` and immediately saves them, so raising after return is late
    for exactly the collision it prevents. For the same reason the floor is applied on
    ALL five load branches, not only `servers.enc`: legacy migrations and "config not
    found" are exactly the cases where there is no counter and the stores are full.
  - **Forgetting the floor on a new branch cannot compile.** The counter is the type `config::UidCounter`
    with a private field and a single constructor `UidCounter::new(persisted, uid_floor)`;
    `AppConfig` deliberately does NOT derive `Default`; its place is taken by `AppConfig::blank(uid_floor)`.
    Both are closed inside `config`, so a config without a floor cannot be built from outside at all. The guarantee is exactly
    that the floor is NAMED, not that it is complete: an unreadable store yields `None` and
    is indistinguishable from an absent one. The neighbouring `uid_counter/tests.rs` holds the bans that
    the type system cannot express (no `Default`, serde, `From<u64>`, `Deref`, `Copy`),
    by reading the crate source.
  - **Stores must be read AFTER path migrations.** `layout.toml` and `charts.json` move into
    `cfg/` via `migrate_flat_to_cfg`, so `startup` runs the migrations before the reads.
  - **The report probe opens the database read-only** (`db::open_readonly`, not `open_reader`,
    which opens for write): it runs before `spawn_writer` and would be the only
    connection, and closing a writing connection triggers a WAL checkpoint — on the main thread,
    before the first window.
  - Deleting a core does NOT clean its history: the number retires, not the data. An unreadable store
    contributes nothing — the floor only grows, so this is best-effort, not a guarantee.
- **A damaged report replica is recovered only after the uid-floor and before the writer.**
  `db::report_recovery::prepare` first takes an inter-process lease, then uses a bounded
  full `integrity_check`. Confirmed damage is never "repaired" in place:
  `reports.sqlite`, `reports.sqlite-wal` and `reports.sqlite-shm` are copied to staging, checked
  by size and SHA-256, given versioned metadata and published by one directory rename into
  `data/damaged-reports/`. Only a published snapshot permits atomically renaming the originals
  into its `originals/` subfolder and creating a clean replica; the source bytes are not deleted. The private
  `ReportWritePermit` does not let `spawn_writer` be called around this order, and the same lease
  closes access for readers and a manual VACUUM from a second process. Holding the lease itself does not yet
  grant access: the gate opens only after a `Ready` result or a fully completed
  `Recovered`, so `Blocked`/`Failed` leave no bypassing read-write path.
  - The `finalized` marker makes the original transfer a resumable transaction: after a crash the next
    start finds already-moved members, checks them against the published hashes and finishes
    the operation. A differing file stays saved; the writer stays off.
  - A new confirmed damage within 24 hours after a successful replacement trips the circuit breaker:
    the current set stays in place, so a filesystem or sync problem does not create
    an infinite chain of copies.
  - The background check remains after start. If it, a reader, or a writer error publishes damage already
    during the session, a shared barrier does not allow a subsequent ACK to start: the writer stops
    the retry-loop, and the core can send the unacknowledged batch again after a clean recovery.
    Non-corruption persistent errors are also bounded by retry-round count and do not keep the channel
    blocked forever.
  - `CheckFailed` is not equal to `Damaged`: an inconclusive check deletes nothing. Strategy,
    warning, and market-cache databases are not in this protocol.
- **Both "by insertion" modes share one `insertion_key`** and are therefore exact mirrors
  of each other. The key is injective (`uid`, and when `uid == 0` — "newest possible", plus `id` as
  a tie-break), so the reverse does not rely on sort stability, and the row currently
  being added in Settings lands first in "By insertion — newest first" and last in "By insertion — oldest first".
- A mode change is presentational: it is neutralized in `AppConfig::structural_sig` and does not
  reconnect cores. The order of `Vec<ServerConfig>` stays in the signature — `SessionManager::config_order`
  is built from it and decides where a re-raised session is inserted.

## Classic and Auto workspaces

Workspace mode is selected independently for each group window. `layout.toml` owns the durable
per-group `workspace_mode_by_group`, `auto_workspace_core_by_group`, and
`auto_workspace_tab_by_group` maps, plus one application-wide Auto rail width. An absent core UID
means Overview. A legacy or malformed mode resolves to `Classic`; a stale UID remains part of
`WindowLayout::max_core_uid` but resolves to Overview until a live core returns to that group.

Runtime state is deliberately separated from layout:

- `WorkspaceFocus` holds the last live Auto window the user interacted with or
  opened `Analytics`/`Strategies` from. This is process-lifetime ownership of the singleton tools,
  not a setting for the next start; the owner leaving for Classic, closing, or rebuilding the window
  clears an invalid focus.
- `WorkspaceRevision` publishes changes of mode, selected core, singleton owner, group-window
  membership and configuration. Cached and async consumers observe this
  revision, rather than hoping for an incidental Shell repaint.

Classic keeps its previous semantics in full. Local panel filters and singleton-window filters remain
their own and are not rewritten on Auto entry. The
`active_trade_core_by_group` map is also unchanged: the live core selected in Auto only temporarily becomes
`active_trade_core`, and Overview uses the saved Classic core or the previous fallback. Therefore
manual commands always have one core and Overview does not turn them into a broadcast; header/chart
producers in Auto do not write the temporary choice back into Classic.

For a group panel the effective scope is computed at read time:

- Classic — its saved All/explicit core filters;
- Auto Overview — all available cores of that group;
- Auto core — exactly one selected available core.

Auto shows the matching selector as pinned/disabled, but does not copy the scope into a local
field. Validity of saved Classic filters is checked against the full configuration/live membership, not
against the temporarily narrow Auto scope. `Detects`, for example, continues ingest and cursoring over the whole group
and filters only the view, so a core switch neither loses nor replays events.

`Analytics` and `Strategies` use the effective scope of the last live `WorkspaceFocus`; without one
they fall back to their saved filters. Selection, details and mutating `Strategies` commands
are limited to the visible effective core, and group-owned `open_goto` re-checks the current Auto scope
and refuses without changing the rail selection. The exceptions are deliberate: a standalone `Report` opened from Analytics
keeps its explicit `ReportScope`; global `Assets`, Profit Monitor and Screener remain
application-wide aggregate surfaces. The group's Auto status bar and the shared rail counters are also not narrowed
by the selected core.

A scope change can happen between a click, a background-request reply, a native picker/dialog and sending
the command. Therefore an async result must carry the identity of the current scope/revision and be accepted
only on a match: a Report query compares its own sequence and filter, Analytics
invalidates previous query identities, and Report export after path selection compares the group
workspace generation and the effective core IDs. Deferred, mutating and destructive paths
(`Strategies`, tuner Save/Copy, order edit, Assets Market Sell, wallet transfer, Report export,
Report delete/restore, Analytics purge) re-check the effective core immediately before
the write or send;
a long purge does the same check
between waits and each next command. A refusal does not move the action onto a new core: a stale
Market Sell does not send the command, shows a warning and closes the confirmation; a stale wallet
transfer clears the pending dialog without a command; a stale Report export writes a cancellation to the log and does not
create a file; a purge stays in the visible failed state.

Multi-target actions are not silently narrowed to the cores that remain visible. `Strategies` keeps the exact
Start/Stop, Apply or Delete plan, and Analytics Tuner Save/Copy — the generation and the full ordered target
set. If even one target, payload or scope changed before dispatch, the whole batch is cancelled before
the first command; saved Classic drafts and staging are not cleared.

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
`Assets`, `CoreStatus`, `Log`, or `Detects`. `Orders` is the separate lower surface; `News` and
`Alerts` (the Figures tab) are Classic-only. Missing, unknown, or stale Classic-only values resolve
to `Report` without rewriting the stored value merely because fallback was needed. Real Auto
activations, including a programmatic
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
operational tabs, and cannot be moved or bypassed by a drop. Ordinary panel detach and chart
detach remain disabled independently of structural editing; panel close returns the panel to its
home strip instead of destroying it. Auto contains every operational
surface except the Classic-only `News` and `Alerts` panels. A Settings → Interface → Panels
action resets the current workspace mode's panel layout in every open group window: Auto
reinstalls the first-run shared topology, Classic rebuilds the default centre from live panel
instances. It does not touch saved servers, theme values, table column preferences, chart
persistence, or detached-window geometry. Before applying even a stale topology
that names either panel, Shell extracts each docked Classic-only panel and retains its exact
identity in `Shell::classic_only_panels`; neither name can therefore be recreated by shared Auto
topology. Returning to Classic supplies those retained identities while applying the saved
name-only `DockNamedLayout`, restoring selection, zoom, split placement, and local view state.
Classic detached panel specs and geometry are preserved while Auto is active, but detached `News`
and `Alerts` are excluded from temporary Auto-only panel construction, so no dock clone appears.
Existing detached chart windows remain open. Auto never writes any of this Classic state to
`docks.json`.

Auto and Classic use the same exact-core chart-history request and Main-tab identity
`(core, market)`. Auto may reshape or reveal its Main stack, while Classic retains its navigation
state; neither path may replace the producer-captured core with another selected or globally active
core. Main history scope is runtime-only and is not persisted into detached Add/Custom chart state.

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

All three layout classes — `layout.toml`, the shared `auto_dock.json` and the joint Classic snapshot —
are serialized by one persistence worker. The live GPUI contour only passes immutable snapshots and
accepts acknowledgements: an accepted enqueue clears dirty, and a new mutation or a failed ack
sets it again. Quit places
the final full snapshot behind the write already in flight, joins the worker and uses a synchronous
fallback only for the classes the worker could not confirm.

The Strategies gear popup resolves two optional `layout.toml` booleans: venue grouping defaults
ON and active-only visibility defaults OFF. Explicit strategy reveals clear active-only through the
same preference setter and persist that choice through the shared layout coordinator; the popup's
open state remains process-only.

Interactive tables without their own store keep the selected column and direction in
`WindowLayout.table_sorts` via `persistence/table_persist.rs`. The key includes a stable table id
and the host context (`:dock` / `:win`); each saved name is validated by the panel itself against its
current sortable/visible columns, and a missing or stale key leaves the historical default.
Orders, Assets, Core status Flat/By IP, Screener and Analytics Tuner By coin use this shared
contract. Alerts stores sort with dock state, Report — in SQLite, and the Analytics strategy list,
Profit Monitor and Settings core order keep their own layout/settings preferences; the shared
map does not duplicate those authorities. All compare-then-dirty writes go through the existing
debounced layout worker and the final quit flush.

`ReportPanel` is a lightweight GPUI entity: its SQLite connection/schema, core list, columns, and
saved display preferences load on the background executor and publish only into a still-live panel.
Independent revision counters prevent a late result from replacing a value the user changed and
then changed back. Sort, comment-pane, and visible-column writes open a worker-owned connection
outside GPUI, serialize through the shared lock, and reject stale per-preference sequences before
writing; sort key and direction use one SQLite statement. Natural-width cache identity includes
scale, live locale, and the exact resolved font, so language or font changes cannot reuse stale
header widths.

The group-owned Auto Report retains a separate strategy-name mask beside the exact strategy
selector. It is applied to every core in the effective Auto scope (the complete Overview or one
selected core), combines conjunctively with exact keys, and uses full Unicode case-folded literal
substring matching through the shared
`ReportFilter`, so rows,
totals, stale-result identity, and export cannot diverge. Strategy catalog discovery clears both
strategy predicates and therefore never self-locks under either filter. The Auto core trigger takes
its current full name from the live group roster, falls back to report metadata only when offline,
and exposes the complete text through a fitted trigger and tooltip. The toolbar consists of small
semantic chrome sections whose dividers travel with the following section; exact strategy, mask,
and detached range bounds wrap independently instead of forcing horizontal scrolling.

The Report footer's traded volume is computed in the same `query_totals` pass, under the same
`ReportFilter` and pinned read snapshot as the visible rows, count, and profit. It is an unsigned
two-sided notional: each closed non-Funding trade contributes quantity times entry price plus
quantity times exit price, regardless of long/short direction. Native totals remain split by exact
quote currency; a mixed scope is unified in USDT only when every eligible row has a trustworthy
native notional and a rate from the Report snapshot's active historical/current valuation mode.
The volume aggregate owns completeness separately from profit coverage because open and Funding
rows still belong to Report count/profit. Inverse contracts whose prices and money use different
quotes, explicit liquidations with non-economic prices, unknown quote identities, and missing or
invalid quantity/price inputs fail closed, and they do so PER ROW: such a row is excluded from the
summed money while still being counted, so every published native subtotal is dimensionally sound
and each bucket carries its own reconstructed count beside its eligible count. Completeness is
therefore a property the FOOTER states, not one the query withholds an amount for. Deciding it
inside the bucket was the earlier design and it failed on the commonest scope of all: a
single-currency Report has no second bucket to fall back on, so one liquidation among a thousand
trades blanked the figure entirely. The footer states every bucket that reconstructed at least one
trade and, when any eligible row is unaccounted for, marks the result incomplete in the same fact
that carries the figure — warn tone, its own wording, and the excluded order count with the quotes
those orders belong to in the row tooltip. What stays forbidden is the opposite: a partial amount
presented as the complete filter total, so an incomplete figure never wears the plain wording, the
unified USDT conversion remains complete-only, and a scope where not one trade reconstructed still
publishes no volume at all. A source without `sellreason` likewise cannot prove that a closed row is
not Funding, so it contributes to completeness but cannot publish volume.

The footer states realized profit as a percentage of the average order only when the loaded
snapshot's own filter names exactly one core — averaging order sizes across cores would mix
universes with different typical sizes, so the fact is absent for an All or multi-core selection.
Its currency follows the head figure exactly: the one native bucket the scope carries, or a
unified USDT total, denominated and gated the same two ways the head promotes one — never a
figure in a currency the row does not otherwise show. The unified arm carries its OWN USDT leg
computed alongside the native one rather than reading the valuation cache's `UsdtTotal::spent`,
which admits any numeric spend with no positive-spend guard and no Funding exclusion; borrowing it
would average over a wider row set than the native arm while still claiming a complete count. The
average itself is over rows with a positive numeric settled spend, excluding Funding rows exactly
as the traded-volume leg does — the same definition Analytics' own average-order figure uses. Rows
the scope cannot account for (unknown quote, Funding, non-positive or non-numeric settled spend)
are excluded from both sums and stated as a count in the fact's tooltip, in either arm: even the
unified figure can be partial, because its own completeness is judged against the counted rows,
not the scope's full row count.

For a group-owned `AutoCore` Report only, `core_name` is contextually unavailable because every row
already belongs to the selected core. This is a display lens, not a persistence mutation: the raw
visible set, `app_meta`/`layout.toml`, sort state, and widths remain untouched. The grid, Columns
menu and its All action, selection copy, and visible-columns export all use the same effective lens;
the explicit all-columns export still uses the full runtime schema. Auto Overview, Classic, and
standalone Report expose `core_name` according to the unchanged saved preference.

The published `ReportData` snapshot carries the exact `ReportFilter` that produced its visible
rows. A Report coin action therefore queues View trades with that published date/side/emulator/
deleted/strategy scope, the clicked row identity, and the row's catalog-verified exact core and
market. The chart query forces closed-only history and replaces core/coin predicates with exact
values; editing controls while an older result remains visible cannot silently retarget that row.

## Editing a strategy field: confirmation, not trust

As of MoonProto `9c7b3d73`, `strats().snapshot(id)` and `snapshots()` are STRICTLY core-confirmed
state. `sync_local_strategies` no longer rewrites the local copy optimistically, so
a sent edit lives separately, in `StratsState::strategy_edit(id)`, with status `Pending`
or `TimedOut`, and is resolved by `StratEvent::Edit{Submitted,Confirmed,Adjusted,
Superseded,TimedOut}` events in a 45 s window. **A timeout is not a refusal**: the core may have applied the edit and lost
the echo; a late reply still confirms it.

This arrives at the terminal as a typed carrier (`moon-core` cannot localize, so
enums go out, not ready-made strings): `FeedMsg::StrategyEdits` carries a FULL replacement of the open
set plus a batch of resolutions, and `CoreData` holds the open edits and the resolution ring
(`STRATEGY_EDIT_NOTE_CAP`) with TWO counters — `strategy_edit_rev` and `strategy_edit_note_rev`.
There are two counters on purpose: a surface that draws only open edits must not
repaint on every resolution, and vice versa.

**`StrategyEditNote.seq` is generated PER CORE.** Any consumer that flattens
notifications from several cores into one list or shares one cursor among them loses this binding —
a strategy id is local to the core and repeats, and a shared cursor eats other cores' notifications. The cursor
is always `HashMap<CoreId, u64>`, and matching is always by the pair "core + id".

**`TimedOut` has no paired resolution** — `StrategyEditResult` knows only
`Confirmed`/`Adjusted`/`Superseded`. Timeout exists only as a phase of an open row and
is determined by polling `CoreData::strategy_edit(id)`, never from the notification ring.

Outgoing full-list sync must overlay open edits on top of confirmed
state (`feed/live/commands.rs::overlay_pending_edits`) AND append open edits that are not in
confirmed state at all — these are created and restored strategies, which by definition have
no confirmed pair. Without the first, the next sync would put the pre-edit value back on the wire;
without the second, a just-created strategy would simply vanish from the list.
Append order is deterministic by `(submitted_at, strategy_id)`, because
`strategy_edits()` is a `HashMap` iteration.

A caveat for later: `StratsState.local_snapshots` at the top is private and has no accessor, so
`overlay_pending_edits` rebuilds this composition itself. The duplication is deliberate and temporary.
## Report replication: the checkpoint and the live-row map

The `orders_rep` replica catches up with the core by increasing `newRecID`, so ordinary catch-up is physically
unable to see that an old row was hidden, restored, or purged by retention while
the terminal was offline. The protocol closes this with a compact **live-row map**, and the whole order
of operations rests on one rule: **only the transaction that applied the map writes the checkpoint.**

- **A core's start state** is `rep::ReportStart`: `Fresh` (no local rows),
  `Resume(n)` (rows exist, epoch not yet saved) or `Checkpoint{epoch, next_from_rec_id}`.
  The checkpoint lives in `app_meta` under `rep_epoch_{core_uid}` / `rep_next_{core_uid}`, and the feed starts
  via `sync_from`, `sync(resume)` or `sync(fresh(All))` respectively. The pair with epoch is needed
  because a database recreated on the core is otherwise indistinguishable: the new one may already have grown past the old
  numbers, and a purely numeric cursor will not notice.
- `Resume(n)` is a **one-shot migration path** for replicas written before checkpoints existed.
  A full re-pump instead would cost an existing user hundreds of MB. The accepted residual
  risk: on this single pass there is still no epoch, recreate detection falls to the high-water
  heuristic, and a core that recreated the database offline such that the new one already grew past the old
  `newrecid` maximum will go unnoticed — rows with matching ids will keep the dead database's values,
  and the live-row map does not fix that (it carries visibility, not content). Ids missing from the new
  database it will still hide, and from the first saved checkpoint onward a recreate is caught by
  epoch. Anyone who cares — a replica reset forces a full sync.
- **The saved `next_from_rec_id` is never raised to `max(newrecid)+1`.** Live upserts
  routinely land ABOVE the checkpoint between two catch-ups, so the local maximum is usually
  larger; taking the maximum would skip the pages between them. Re-requested rows
  are idempotent by `(core_uid, newrecid)`. The reverse is also true: a checkpoint with no local rows
  is discarded.
- **A cleared bit hides the row, it does not delete it** (`deleted=1`): the map does not distinguish
  soft-delete from retention, and a later restore or upsert must be able to bring the hidden row back.
  That is also why a live `RowUpsert` without a `deleted` field writes `deleted=0` — exactly how
  moonproto itself reads it, overlaying live events on the map in flight.
- `rep::apply_alive_map` scans **only the core's local rows** within `covered_up_to`
  (the primary key serves both the filter and the order — no sort, no new index), accumulates
  only the mismatches as collapsed ranges in a stream, and applies them with two `UPDATE`s. `covered_up_to`
  is the core's high-water; it can be in the millions; walking it would block the only writer.
- **Without a `deleted` column the checkpoint is not saved**: there is nowhere to write visibility, and a
  "map applied" mark would permanently deny the core another attempt.
- **Any replica reset clears the checkpoint together with the rows** (`rep::reset_replica`) — both the
  `database_recreated` path of a page and `DatabaseRecreated` of the map. Otherwise the next start would take
  the old epoch, wipe the partially built replica again and loop a full re-pump.
- Failure order: a page is committed before its `page_applied`, `SyncComplete` arrives after
  the last ACK, `DbMsg::SyncComplete` and `DbMsg::AliveMap` go in one FIFO to one writer.
  Rolling back a batch moves neither the checkpoint nor the published start state — the next
  connection simply repeats catch-up.

### Report readers

Query reads go through `open_reader` / `open_reader_with` and come back as a `ReportReader`. A
process-wide permit (`db::reader_budget`) allows eight at once. The connection field is declared
before the permit, so the descriptors close before the slot returns. The permit owns the budget:
`read_snapshot` borrows a transaction from the caller's connection, so a shared worker cannot sit
in its place. A reader that finds every slot taken waits up to three seconds. If one frees, the
open continues. If the wait expires, the open fails as `FailKind::Exhausted`. A wait cancelled
because the request was superseded is `FailKind::Other`, not exhaustion.

`SQLITE_CANTOPEN` is classified as `Exhausted` too, separately from that synthetic budget
timeout. The class is retryable — the descriptor budget, or something else holding the file —
and it is not an empty period and not a damaged replica. The uid-floor probe (`open_readonly`)
and the background integrity scan open a bare read-only connection and do not take a slot.

Which companion databases are attached is a property of the query (`AttachSet`), chosen before
the snapshot because SQLite refuses `ATTACH` inside a transaction. `open_reader()` is
`AttachSet::ALL` (strategies and the valuation cache). Chart trade history never references the
valuation schema and uses `AttachSet::STRATEGIES_ONLY` (`CHART_TRADE_HISTORY_ATTACH`). A missing
strategies file, or a missing or unhealthy valuation cache, attaches nothing for that companion
and the open still returns. When every requested database does attach, one reader holds nine
descriptors for `ALL` and six for strategies only: each database in WAL is the file plus `-wal`
and `-shm`.

On Unix, `startup::open_file_limit::raise_to_hard_limit` runs once after the logger is installed
and before the install's locks, migrations, and database opens. It raises the soft
`RLIMIT_NOFILE` to the hard limit. macOS caps the request at Darwin `OPEN_MAX` (10240), because
`setrlimit` returns `EINVAL` above that even when the hard limit is infinity. A failed raise is
logged and does not stop startup. A `--fixture` run can open the relocated replica during
bootstrap, which is earlier than this raise. Windows has no such limit.

### The order-trace archive (`order_traces.sqlite`)

The core serves traces of closed trades on `request_traces(ReportUID)` from an archive
bounded by retention, and moonproto keeps no cache of its own and explicitly asks the terminal
to keep a durable store keyed by `ReportUID` and to ask only for what is missing
(`docs/reports.md`, "Archived Order Traces"). The file is separate from the replica for the same reason as
`strategies.sqlite`: the replica is rebuilt from the core, and a trace the core has already forgotten no longer
exists anywhere.

- **One writer** (`db::order_traces`, thread `order-traces-db`), three entries from any core's feed:
  the core's reply (`Answer`, empty too — as a row with no lines and a `checked_at_ms`, so the same
  old trade is not asked on every start; an empty reply older than `EMPTY_RECHECK` is again treated as
  unknown), a row close (`RowClosed` — the writer itself decides whether to ask, and
  answers the feed with a list through `AskSink`), catch-up completion (`Backfill` — the writer, through
  the replica reader with `ATTACH`, lists closed rows within `BACKFILL_DEPTH` that have no current
  reply, newest first). `TraceFailed` is never written: that is not "empty".
- **One pace** (`feed/live/trace_backfill.rs`): every `request_traces` of a connection — a window
  request, a just-closed row, a backfill — goes through one queue: no more than `WINDOW` unanswered
  and no more often than one per `MIN_INTERVAL`; a window request goes to the head. Three refusals in a row drop
  the queue — an old core is silent on the command, and moonproto turns each silence into a refusal after
  12 s.
- **One resolver** (`backend/traces.rs`): any consumer — the Trade window, a chart in lines mode
  — hands it a list of `ReportUID` with a ceiling of core requests and reads the row state after
  its own wake channel (`TracesRevision`). The resolver first reads the archive off-thread,
  asks the core for misses, and takes replies from `CoreData::report_traces` on the feed drain —
  one observer per process. Nobody else talks to the file or the core directly.
- **The live chart** — the "Trades: Marks / Moonbot lines" style (`ChartGraphicsCfg::trade_history_style`,
  per-tab, default per view via ⧉). In lines mode a trade's exit is always a line: from the archive with
  the transfer path when the core supplied it, otherwise a straight line from the report row itself (sell price,
  `SellSetDate` → `CloseDate`, `ReportExit::of_record`); the entry is a line only with its own
  archived Buy line, otherwise it stays an arrow (the report has no entry start); the engine itself ranks
  history trades by how close the close is to the right edge of each pane (`chartdx/archived_lines.rs`,
  `rank_wanted`), the panel hands the first 30 to the resolver (`panels/chart/trace_lines.rs`, no more than once per
  400 ms on a pan) and on its wake builds the map `ReportUID → lines`; the engine's second
  `build_order_geometry` pass puts the `append_archived` store into the same buffers as live orders, with the
  same relaxations as a frozen window (`hide_closed_sell_line` off, closed-trade ceiling off).
- Key `(core_uid, report_uid)`; `0` is a local replica stub and is never a key. The file
  takes part in the uid "floor" (`observed_uid_floor`), like the other stores with `core_uid`.
- Point prices are stored as `f64` (the wire docs forbid a downcast on save); they narrow to the chart's `f32`
  only when building `LineTrace`.

## USDT valuation: two modes

Report and Analytics can show quoted money in USDT in two ways. The mode is one for the whole
application (`SettingsFile.report_valuation_mode`, read through `Backend::valuation_mode`), because
two windows on different modes would show the same period as two equally "true"
totals.

The selector lives in **Settings, the General tab**, not on the Report and Analytics panels: the default
value answers "what was the trade worth when it closed", and that is the right question
for almost everyone — an expert setting does not belong on the working toolbar. Saving calls
`Backend::apply_valuation_mode`: it raises the worker demand flag and publishes `report_revision`.
The publish is mandatory — a mode change moves no row, so no generation would shift
by itself, and open windows would keep drawing the old numbers under the new label.

- **At trade time** (the default) — `valuation.sqlite`. First the close of the trade's fully
  closed minute is used. If that candle is missing, the resolver with no fixed time limit looks for
  the first later closed candle and takes its open. This is historical P&L.
- **Current rate** — `db::valuation::current`, an in-memory snapshot "currency ordinal → USDT price".
  **Never persisted**: `ALGORITHM_VERSION` is in both cache primary keys, so the modes cannot share a
  row, and a saved "current" rate after restart is a stale
  rate under a fresh label.

The historical resolver tries direct and inverse Binance, Bybit and Hyperliquid spot markets. Indexes
of the Hyperliquid spot universe come from `spotMeta`, they are not baked into the code. If there is no direct market,
a deterministic two-market route through any known `QuoteCurrency` is allowed; both of its
legs must have a candle of the same minute. Therefore, for example, `USDH -> USDC -> USDT` does not
add prices from different moments. `rates` stores separately the trade minute, the actual minute,
price basis and both legs; the full source and the delay are available in the Report column tooltip.

Transport timeouts and temporary service failures permit another provider to supply the requested
closed candle; malformed responses remain data errors. Successful fallback valuations retain their
actual provider and market. If no fallback resolves the request, it remains retryable rather than
being classified as a missing candle. Outage retry records do not advance the proven-empty horizon:
the original minute must be retried. Historical batches defer failed currency keys durably while
applying independent ready rows; outbox acknowledgements follow processing or durable deferral.

The Profit Monitor reads native currency partitions in the same snapshot as its coverage decision.
When the full scope cannot be converted, its core/exchange tables remain visible in separate currency
sections. Each section has its own monetary subtotal; the footer never adds unlike currencies.
Unknown denominations retain their core identities and counts but publish no monetary amount.

A missing candle up to the current closed horizon is not a final "rate unavailable".
`rate_searches` stores the reached horizon and the wall-clock time of the next attempt; the row stays
pending, survives a restart and is retried without a hot loop. Changing `ALGORITHM_VERSION` moves
the old derived-cache family into archive and fully rebuilds the valuations, without changing the report DBs.

Both branches build the same `CoverageSql` through `valuation::projection(mode, attached, ...)`, so
Report per-column values, the totals row, export, the Analytics summary, calendar, groups and
tuner are converted by one rule. Rates go into SQL as literals, not placeholders:
the ready string is reused as a FROM fragment by callers that bind only their
date range.

Known limits of the current-rate mode are documented properties, not bugs; both are named in the hint
under the selector (`general.valuation_mode_hint`):

- Rates are taken from **Binance/Bybit spot**, not from the exchange where the trade was made.
- **The tuner is re-valued too.** Thresholds are fitted against re-valued history, so what is optimized
  is not what actually happened. This is a conscious user choice, not an oversight.
- A currency without a fresh rate is not converted at all: after `FRESHNESS_MS` (10 minutes) the rate stops
  counting as current, and the total honestly splits by currency — the same as under incomplete historical
  coverage. A currency whose every route is permanently missing is `unavailable`; a currency
  that simply has not been queried yet is "in progress".

The worker updates the snapshot with stage `ValuationStage::CurrentRates`, **one currency per cycle turn**
(a cold pass over a currency costs up to four sequential 15 s routes) and **only while
`ValuationHandle::set_current_wanted(true)`** — with the mode off, not a single request goes to the network.
It is published through `generation`/`commit_dirty` (data), not through `status_revision`
(health): the health counter is deliberately not a reason to re-query.

A publish is expensive for the user: on a generation shift every open Report and Analytics window
does a full re-query and rebuilds the tree. Therefore the update sits on two limiters —
`CURRENT_REFRESH_MINUTES` (5, half the freshness window, so one failed pass cannot
let a rate go stale) and `CurrentRateState::renders_differently`: the snapshot is always saved (freshness
is computed from it), and the generation moves only if the price, its origin, or the set of
unavailable currencies changed. `fetched_at_ms` is deliberately not in the comparison — it moves every pass and
lands in no on-screen figure, so a dollar-pegged quote costs not a single
re-query. Expiry is still visible as a smaller set of rates, so the cut-off
still reaches the screen in time.

## Settings and strategy backups (`backups/`)

`moon_core::backup_store` is the single internal owner of the file lifecycle of both snapshot kinds:
it checks the root, issues unique `.incoming-*` directories, atomically publishes a ready directory
and deletes the old one only after checking the full file set. Domain modules do not duplicate this
logic: they specify contents, the full name grammar and their own retention policy.

`backups` (`moon-core/src/backups.rs`) is one wall-clock coordinator for both domains. It is pinned
to 12:00 UTC, on a normal start immediately catches up the last noon slot that has arrived, and then each
time recomputes the delay until the next noon. Settings and strategies run in separate
job threads: a long SQLite copy or a retry after an error does not delay the second domain. FireTest
does not start the coordinator.

`config::backup` (`moon-core/src/config/backup.rs`) puts into
`<data_dir>/backups/settings/<UTC timestamp>/` copies of two irrecoverable files — `servers.enc`
(API keys) and `cfg/settings.toml` (groups, core checkboxes, the uid counter). The Save button no longer
creates a snapshot: all ordinary writes use `AppConfig::save()`, and the coordinator creates at most one
canonical copy per UTC-noon period. Both config writes and both copy operations are guarded by
one pair-lock, so a snapshot does not mix file generations. A schema migration before overwrite
uses that same arrived day slot as a safety barrier, not a separate snapshot series. Old
snapshots of previous versions sitting directly in `backups/` stay put: the application neither moves nor deletes them.
Before the first of the two replacements `save()` plants `.config-pair-pending` and clears the marker only after
the second; an incomplete pair does not enter the backup, and the next ordinary start re-saves the in-memory
config as a whole. Besides the process-lock, the snapshot re-reads both sources after assembly:
this detects a replacement from a second terminal instance and leaves the slot for a retry instead of a mixed copy.

`strat_db::backup` stores consistent SQLite copies in
`<data_dir>/backups/strategies/<UTC timestamp>/strategies.sqlite`. If the existing database contains
at least one strategy row, startup catch-up is allowed immediately. On a clean database a scheduled snapshot waits
until the writer has successfully applied one full set from each active core with the strategies feed
enabled; an empty set also counts as delivered. The feed advances its delivery-cursor only on
confirmation of a successful SQLite commit from the writer: an overflowed queue or a write error leaves
the same set for a one-second retry even without a new MoonProto event. A change of core membership through
Settings updates the barrier, and the final rename runs under a short topology claim, so a new
core cannot appear between the generation check and the publish. After a temporary error of a ready
source the backup-job retries the overdue slot in five minutes. The manual button on the
Storage tab uses the same atomic mechanism, but always creates a separate snapshot and does not replace
the mandatory noon one.

Both domains keep all manual and scheduled snapshots of the last seven UTC-noon periods. Old
`data/strategies-backup-*.sqlite` files and former root settings snapshots stay untouched.

What is easy to break unnoticed:

- **The directory name `YYYYMMDD-HHMMSS` (UTC) is a contract, not decoration.** A colon is forbidden in
  Windows file names, and the fixed width gives "lexicographic order = chronological",
  which is what old-copy pruning rests on. Sorting by mtime does NOT work: a file copy and
  cloud sync rewrite it. The stamp is built by `util::time::utc_stamp_compact` — from date parts,
  not by slicing a ready-made string, because `{y:04}` sets a MINIMUM width.
- **A snapshot is published whole.** Files are assembled in a `.incoming-*` directory, which is deliberately not
  recognized, and only then the whole non-empty directory is published with one `fs::rename`.
  A scheduled snapshot has the canonical noon-slot name: if two terminal instances
  assembled it at the same time, the finished winner is a shared success, not an error of the second.
- **Cleanup must not be able to delete someone else's files.** A symlink root is skipped, the child's type is taken
  via `DirEntry::file_type` (does not follow links), only expected file names are deleted,
  and the directory itself is removed with a NON-recursive `remove_dir` — so a foreign file inside a snapshot
  preserves both itself and the snapshot.
- **`backups/` is NOT in any of the `paths.rs` migration lists.** Both migrations work
  through `fs::copy`/`fs::rename` and do not recurse into directories, but they also do not refuse them: a
  directory name on the list would silently haul the whole snapshot tree to where `backups_dir()` does not look.
  Precedent — `logs/`, which is absent there for the same reason. Closed by a test.
- **An unreadable `settings.toml` is NOT overwritten.** `toml_io::ConfigLoad` distinguishes "file is missing"
  from "file failed to read"; in the second case automatic re-save under an outdated
  schema version is cancelled. Otherwise a temporary read error (permissions, a share, an unloaded cloud
  placeholder) would become an irreversible replacement of the live config with defaults.

## Telegram: its own thread, chat pairing, and the one-shot tunnel

`moon-core/src/telegram/` is a UI-agnostic agent: its own bot and Mini App. There is no async runtime,
HTTP is synchronous `ureq`, `getUpdates` holds the socket for up to 25 s, so the transport lives in its own
threads, in the same shape as `crowd/`. **An empty token is a hard off-switch: `TelegramService::start`
returns `None` before a channel, thread, path, listener, or helper process.**

- **Two threads.** `telegram-bot` blocking-polls `getUpdates`. `telegram-miniapp-owner`
  separately holds the loopback server and the tunnel, otherwise one delayed poll would block HTTP.
  The GPUI `Backend` talks only through typed `Work` / `Response` channels and drains them in the
  100 ms loop (`Backend::tick_telegram`).
- **The token lives in a `Secret` inside `servers.enc`, not in `settings.toml`.** `TelegramConfig`
  is serialized only into the encrypted aggregate; `Secret` in `Debug` is `Secret(***)`, `ApiError` carries
  neither the token nor the Bot API URL. Settings masks the field and hashes only the token's emptiness.
- **The trust boundary is chat pairing.** The bot name from `getMe` is public. Commands are accepted only from
  a chat id in `authorized_chat_ids` after `/pair`. The pairing code is six characters, 10 minutes, one
  shot, memory-only. An unpaired correspondent gets a flat `telegram.refusal` ("Access
  denied.") — with no hint that a terminal sits behind the bot. `/pair` and `/miniapp` are only from a
  private chat.
- **Mini App from the loop to the outside through a quick-tunnel.** `MiniAppServer` binds on `127.0.0.1:0`;
  `cloudflared` (verified-download via the self-updater path, SHA-256) raises a tunnel to that
  port. The only authenticated request is `POST /api/session`: HMAC `initData`, then
  `authorize_paired_identity`, then a live re-check of `mini_app_enabled` and chat membership in
  `Backend::telegram_mini_request`. Authenticity of a Telegram launch is not terminal authorization.
- **Native Mini App menu follows the tunnel.** The Mini App owner publishes the latest URL and
  localized label; the bot worker reconciles per-chat `setChatMenuButton` between long polls.
  Only paired private chats receive a web-app menu. Disabled, unavailable, or revoked targets
  revert to commands; failed writes remain pending with a cooldown. Updates can wait for the
  current long poll (normally up to 25 seconds). Shutdown makes no uncancellable cleanup calls:
  Telegram may retain the last menu until the next service start reconciles it. The `/miniapp`
  inline launcher remains available and uses the current `MiniAppStatus::Tunneling` URL.
  Pairing reset retains cleanup-only chat IDs across same-token service restarts for the lifetime
  of the desktop process; token changes discard them. Those IDs never grant app authorization.
- **Bot navigation.** Pairing and `/help` install the persistent reply keyboard; `/start` sends a localized welcome carrying that keyboard, followed by an inline report.
  Reply buttons own global period selection and Help. Inline report buttons own exchange
  drill-down, core/day views, back and paging; periods are not duplicated. A repeated reply-keyboard
  period request fetches fresh data. The complete-scope Total row follows the main table rows;
  calculation explanations are in Help, not an additional report disclosure. Emoji-decorated
  aliases and older plain labels are matched exactly in ru/en/es. Identity checks precede both
  welcome and report delivery. Reports always carry inline markup from the first send: Telegram
  disallows editing messages with a reply keyboard. The temporary removal/deletion flow is gone.
  The native menu and `/miniapp` retain the independent Mini App launcher.
- **Chat cleanup continuity.** Per-bot chat-history metadata records the permanent reply-menu
  message separately from disposable rich answers and their original Telegram send timestamps.
  The bot ID comes from `getMe`; metadata never authorizes a chat or stores credentials.
  Atomic publication precedes deletion. Restart restores tracking without another menu notice;
  answers near or beyond 48 hours are skipped, and absent/undeletable cleanup targets do not
  change transport health. Callback edits keep the original send time.
- **Chat reports without Mini App.** `/report`, `/today`, `/hour`, `/yesterday`, `/month`,
  `/lastmonth`, and `/daily` read closed real trades from permitted cores in local history. Custom
  `/report YYYY-MM-DD YYYY-MM-DD` and `/daily` ranges include both dates and allow at most 366
  days. `backend/telegram/reports.rs` uses a background executor, a pinned SQLite snapshot,
  `ReportAxis::load`, `query_totals`, and historical valuation; read failures never become zero.
  Core groups use `CoreOrder`, six active groups per page, with the complete-scope total on every page.
  Exchange groups use canonical venue identity and support scoped drill-down; empty scoped membership
  uses the no-match sentinel. Groups without trades are removed before paging; zero-profit trades stay.
  Native subtotals remain available when USDT conversion is incomplete. Rich HTML reports use
  `sendRichMessage`; private sender-matched callbacks edit the originating message and recheck
  saved authorization. Paging/view changes retain UTC bounds; a new reply-keyboard request resolves its preset again.
  The display zone follows the terminal clock. One pending report survives service replacement,
  preventing overlapping reads; a report response has a bounded 120-second wait with failure
  feedback. Unchanged edits are normal no-ops. See [chat report usage](TELEGRAM_REPORTS.md).
- **Per-chat access.** Encrypted `TelegramConfig` stores one owner and named viewer profiles with
  explicit stable core uids. Legacy files resolve their first paired chat as owner; every other
  chat defaults to no data access. The owner sees all history; viewers' core rows, exchange and
  daily groups, and totals all intersect the same saved uid set before SQL aggregation. Empty
  intersections use the no-match sentinel, never the query API's empty-list/all-cores shortcut.
  Report completion rechecks the permission snapshot. Saved role or grant changes synchronously
  revoke old service liveness before retiring the worker, cancelling queued deliveries and retries.
  Settings edits remain a draft until Save; archived candidates load asynchronously from history
  independently of checkbox selection. Read-only viewers gain no core-control authority.
- **The Mini App page is currently empty on purpose: its only request is the session check
  (`POST /api/session`). This is a scope decision, not an unfinished screen.**
- **Shutdown joins everything that was started.** A token or chat-list change is
  `TelegramState::restart`. Clearing the Mini App checkbox does not touch the bot (`MiniAppOwner::stop`: first
  the tunnel, then the listener). Exit is `TelegramState::stop`. `Drop` of `TelegramService` and
  `MiniAppOwner` does the same.

### Telegram on the core

Settings → Telegram tab, the second section under the terminal bot, heading "Telegram on the core",
with a core picker. The section above is the terminal's own bot (`moon-core/src/telegram/**`, pairing,
pushes, Mini App): a different entity, not this reader.

Inbound: `SettingsEvent::TelegramUpdated` → `feed/live/convert.rs::telegram_from_proto` →
`FeedMsg::Telegram(Option<Arc<CoreTelegramState>>)` → `CoreData.telegram` and `telegram_rev`;
the panel reads this through `settings_sig`; panels do not subscribe. Outbound:
`CoreCmd::Telegram(TelegramCmd)` → `feed/live/telegram.rs::handle` →
`client.telegram().<method>()`.

`telegram_rev` is an acknowledgement counter, not a content one: MoonProto sends an event on every snapshot,
including an unchanged reply, and the "Sent, waiting for the core…" banner is cleared by this counter.
`telegram_fresh` and `FeedMsg::TelegramStale`: `ConnStatus::Ready` does not mean the snapshot is fresh —
a reconnect in the loop immediately yields `Ready`, so `live = Ready && telegram_fresh`; a stale
snapshot is drawn muted, actions are off, there is no QR.

Phone, code, 2FA password, email, proxy password / MTProto secret and `qr_link` are entered,
sent and forgotten: neither a setting, nor a draft, nor a log line. Mirror types write
`Debug` by hand so `{:?}` does not leak a secret. `refresh()` on exactly three triggers — tab
activation, section expand, core pick — plus once after `Connected { fresh: false }`;
login / QR / code / reset / logout do not repeat themselves.

## UI Components

The application depends on `Moonbot-Tech/MoonUI` and uses components through `moon_ui::*` /
`moon_ui::components::*`. The terminal's application panels must not redraw shared UI patterns
by hand if MoonUI already has a fitting component or a close Longbridge descendant.

Adaptation rule:

- if a Longbridge component already gives the needed mechanics but theme/geometry/states do not match
  Moonbot design, it must be fixed or wrapped inside MoonUI;
- the terminal then uses the MoonUI API, not the direct Longbridge API and not a local ad-hoc
  widget in a specific panel;
- if MoonUI lacks a public hook/API for a terminal scenario, first add that
  hook in MoonUI, then replace the on-screen hand-rolled code;
- temporary exceptions must be explicitly marked in code or docs with a reason and a removal plan;
- the chart renderer is not a UI component: the chart host may use MoonUI chrome/overlays,
  but its own GPU render stays in `chartdx`.

Practical example: popup/menu/dialog mechanics must go through `moon_ui::components`
(`WindowExt`, `Root` dialog/sheet/context-menu/notification layers, Moon menu wrappers). If
the base Longbridge `ContextMenuExt` draws in a foreign theme, it must be brought to the Moon theme in MoonUI
or a Moon wrapper used. The terminal must not render an open context menu as a panel
child: open it through `window.open_moon_context_menu(...)`, so z-order, dismiss and future
portal behaviour remain MoonUI Root's responsibility.

Root overlay layers are not external render hooks for the application. The application opens a dialog,
sheet, context menu and notification through `WindowExt`/Moon wrappers; `Root::render` itself decides where
and in what order those layers sit relative to the main view. This matters for chart
UnderScene/z-order and for the same behaviour on Windows/macOS/Linux.

FireTest does not read sources and does not check architecture statically. The built-in
`--debug-script chart-smoke` checks live behaviour: opening a chart, real bounds,
native input, counters/CPU/GPU/RAM. Static bans of the form "do not render a menu as a panel
child" live in `tests/theme_contract/`, not inside the runtime scenario.

### UI scale

One user scale (`ui_scale`, Settings → General → "UI zoom", 50–200 %) is installed as the window's
content zoom: `startup::moon_theme_config_for_presentation` puts it into `MoonScale::zoom`, and each
window's `MoonRoot` applies it through `Window::set_content_zoom`. The zoom folds into the window's
`scale_factor`, so every pixel scales — text, geometry, images, hit areas, persisted dock sizes —
without the components taking part. MoonUI's tokens stay at the design's own scale:
`scale.ui == scale.font == 1.0`, `tier = Sm`, `font_delta = 3`; `design::ui_px` is an identity
adapter over the tokens, and `design::CONTROL_TIER` / `BODY_TEXT` / `INPUT_SIZE` are the one size
system that controls, text and chrome bands derive from. There is no density setting (Compact /
Standard / Large) any more: stepping MoonUI tiers substituted a different design instead of scaling
the reviewed one, and did not keep its proportions.

The chart does not take part in the zoom. `chartdx` sizes its render target by the full
`scale_factor` (the slot really is `bounds × factor` device pixels) and its own sizes — line
widths, candles, axes, captions — by `scale_factor / content_zoom`, the platform DPI. Three spaces
meet at the chart and each crossing has one home: a content-space pointer reaches device pixels
through `ChartEngine::slot_scale_factor` (the window's factor); the engine's geometry and the input
container use `last_ppp` (the chart-design factor); the chart's text layer lives in its own logical
pixels and crosses into GPUI's content pixels only in `chartdx::text::content_px` /
`chart_metrics`; and GPUI overlays over chart geometry divide device pixels by the window's factor
(`panels/chart/render.rs`) or go through `chart_origin_logical` (`arb_open.rs`,
`market_actions.rs`). Screen coordinates (`window.bounds()`, window placement, the FireTest probe,
first-open window sizes) stay in the platform's pixels; `windowing::responsive_width` reads
`viewport_size()`, which is content pixels.

## Windows

The terminal uses its own header and borderless/CSD behaviour. Check separately:

- Windows: restore bounds on multi-monitor/DPI.
- macOS: Metal toolchain and `.app` launch from a GUI session.
- Linux X11/Wayland: no second system header, Secret Service for encrypted config,
  surface/present stability.

## Local development

Public `Cargo.toml` files keep git dependencies on `Moonbot-Tech/MoonUI` `branch = "master"`.

`Cargo.lock` **is committed**, and that is a freeze of third-party versions: a compromised or simply
unexpected release of someone else's crate cannot enter the build on its own. The policy in four points:

1. Third-party versions move only by a deliberate commit.
2. MoonUI stays rolling: CI on every run does a targeted
   `cargo update -p moon-gpui -p moon-gpui-platform -p moon-ui`; locally that is `make update-moon-ui`.
3. MoonProto moves ONLY by hand (`make update-moonproto`) in a separate commit; CI does not touch it.
4. Every compiling CI job first checks the lock against the manifests (`cargo fetch --locked`),
   then updates MoonUI, then fails if that update moved anything other than MoonUI —
   a targeted `cargo update` is conservative by documentation, not surgical, and a new MoonUI may
   pull someone else's version along. That case must be a red PR and a deliberate commit.

`EmbarkStudios/cargo-deny` as a separate blocking job scans the committed lock: advisories,
duplicates and the git-source allow-list (`deny.toml`).

A reproducible build for third-party dependencies — yes; MoonUI freshness is a separate explicit step.

Every binary writes a build stamp to the log:

```text
build: moonterminal=<git-sha>[+dirty] release_base=<stable-git-tag|unknown> moonui=<git-sha|local:git-sha>[+dirty]
```

Before a release tag: move MoonUI deliberately, commit the lock, wait for the gates on THIS commit
and only then set the next canonical stable tag `vMAJOR.MINOR.PATCH`: start a new minor line
at `.0`, and ship fixes by incrementing PATCH. Historical two-component tags
like `v0.21` are read by the updater as patch zero, but new tags of that form and the equivalent alias
`v0.21.0` are forbidden. `release.yml`
builds the immutable commit of that tag strictly `--locked`, checks the GitHub SHA-256 of the Windows
artifact while still in draft, and only then publishes the release as Latest. The repository must have
release immutability enabled: publication locks the verified tag and assets; MoonUI is not updated there.

### Windows self-update

An ordinary Windows process after `startup::boot` starts one `Backend`-owned discovery loop:
the first scan runs immediately, later ones are pinned to UTC half-hours with a stable process phase
and a minimum five-minute gap after start. In the normal case a new publication is visible no
later than 30 minutes, and on the start boundary — 35 minutes; suspend, the network and the mandatory
GitHub deadline can increase the delay.

Discovery is limited to the first 200 releases: two pages of 100 entries and no more than two
consecutive metadata requests per scan. The first page is conditionally rechecked every cycle,
the second — when there is no cache, the first changed, or by a daily sentinel. The ETag is stored separately
for the exact page URL; a `304` without the matching validator and an incomplete two-page scan
end fail-closed with no partial cache update. A stable full cache spends up to 49
requests per day per process. With little remaining quota the optional second page is skipped;
`Retry-After`, an exhausted `X-RateLimit-Reset` and a bounded local backoff only delay
the next scan. The GitHub limit is shared per IP, so an unknown number of processes behind one NAT cannot
be assumed to fit the unauthenticated quota.

The header button appears only for the greatest canonical stable tag newer than the built-in
`release_base`, if the release is immutable, not draft/prerelease, contains exactly one `MoonTerminal.exe`
and GitHub returned the required `sha256:` digest. An unknown base version, a network error or
incomplete metadata keep the last trusted state; install starts only on an explicit
click.

`moon_core::update` downloads the exe as a stream into a unique `.part` inside
`.moonterminal-update/<nonce>/`, bounds the size, checks SHA-256 twice through the same open
file handle and only then publishes the staged file. One `UpdateController` belongs to `Backend` and
is observed by every `Shell`, so different windows cannot start a parallel replace.

The downloaded exe is launched as a hidden helper process. The helper first validates the versioned manifest,
canonical paths/nonce/hash, opens a handle of the exact parent process and publishes `ready`.
The UI confirms the read `ready` with a separate nonce-bound `commit`; without it the helper exits on
deadline and does not replace the exe. Only after `commit` does the UI call ordinary `App::quit`, keeping
the existing `on_app_quit` path, and the helper waits for the parent to exit, uses `ReplaceFileW` with
a backup, starts the new target and waits for
time-bounded `started` and `healthy`. The new version publishes `healthy` immediately after entering
the safe part of `startup::run`, but strictly before migrations/opening portable storage: before this boundary
any failure terminates only the launched child, restores the backup and reopens the previous
version with a notification; after it, rolling back the old exe is already forbidden, so old
code does not read a new schema. The helper deletes the backup only after its own read of `healthy`. `cfg/`, `data/`,
`logs/`, `backups/` and `servers.enc` do not take part in the transaction. On macOS/Linux the updater is disabled.

The trust boundary is HTTPS, an immutable GitHub Release and its SHA-256 metadata. The release workflow
serializes publication per tag; the administrative `RELEASE_ADMIN_TOKEN` is allowed only in
the last step of the immutable-release check and publication. The publication job runs in the
`release` environment: it waits for approval from one of its required reviewers, and the token must live in that
environment's secrets, not the repository's, and is then unavailable to runs outside it (the environment accepts only
`v*` and `main` tags). This detects an
asset swap relative to the published release, but is not code signing and does not protect against
compromise of the repository owner or the release publisher.

An active local MoonUI override through `.cargo/config.toml` rewrites the tracked `Cargo.lock`
(it will gain `path` entries). Restore it before committing.

For local development these must sit side by side:

```text
workspace/
  MoonTerminal/
  MoonUI/
  MoonProtoBeta/
```

The local override is done only in the ignored `MoonTerminal/.cargo/config.toml`:

```toml
[patch."https://github.com/Moonbot-Tech/MoonUI"]
moon-gpui = { path = "../MoonUI/crates/moon-gpui" }
moon-gpui-platform = { path = "../MoonUI/crates/moon-gpui-platform" }
moon-ui = { path = "../MoonUI/crates/moon-ui" }

[patch."https://github.com/Moonbot-Tech/MoonProtoBeta"]
moonproto = { path = "../MoonProtoBeta" }
```

Do not use top-level `paths`: it changes the shape of the dependency graph and already produces a Cargo warning
that in future Cargo versions may become an error.
