# Windowing Contract

This document records the current MoonTerminal window contract on top of MoonUI/GPUI. It is not
a visual reference, but an engineering rule: how to open windows without breaking
taskbar/dock semantics, restore, and chart own-pass.

## Where windows are created

Open new terminal windows through `crates/moon-ui-gpui/src/window/windowing.rs`.

Do not assemble `gpui::WindowOptions` by hand in panels, Settings, or
the Strategies editor without an explicit reason. Otherwise it is easy to forget `app_id`,
decorations, owner, taskbar policy or min size, and get a window that
behaves as a separate application again.

Current factories:

- `trading_window_options` - the main group/terminal window.
- `tool_window_options` - owned tool/secondary windows such as Settings,
  Strategies and Assets.
- `detached_panel_window_options` - detached non-chart panels
  (`Orders`, `Assets`, `Log`, `Report`).
- `detached_chart_window_options` - detached chart windows; deliberately
  independent of the main/group window.
- `debug_window_options` - debug/perf/chart diagnostic windows.
- `profit_monitor_window_options` - independent desktop Profit Monitor without its own taskbar
  button; minimizing a Main/group window never minimizes it.
- `trade_window_options` - one closed trade replayed beside Main. Independent and hidden from
  the taskbar, like a detached chart, with a minimum size the detached-chart factory does not take.
- `login_window_options` - the startup login prompt. Standalone, with the application icon and
  a taskbar button, because it can open before any group window exists.

## Auto workspace: rail, dock, and group windows

Auto is a full workspace inside an already existing group window, not a new kind of
OS-window. Each active group keeps its own `Shell`, its own single `DockArea` and its
local panel instances.
The left `MoonVirtualList` rail shows every configured application core as an exchange tree.
Exchange headings follow alphabetical live market metadata order, with unknown exchange first;
their core leaves retain canonical `core_order::CoreOrder`. Known brand logos appear on exchange
headings only, never on core rows. Core names do not infer exchange identity. Disabled or
unavailable rows remain visible with status and reject clicks.

A core is selectable when it and its group are active, a live session exists and the group window is in
the `Opening` or `Live` state. Per-core `show_window` does not take part here: a headless core is available
through its group's shared live window. A click inside the current group only changes that group's Auto scope. A click
on a core of another group atomically saves `AutoTrading` + core for the destination, hands it
singleton focus and activates the already existing group window; panels are not reparented between windows and
no new parallel window is created.

The rail has Overview only for its current group, while the header summary counts
configured/ready/problem across the application. Its `340 px` initial width is stored in
`layout.toml`, clamped to `52..560 px`, and broadcast after drag-resize to every open Auto window.
`MoonResizablePanelGroup` separates the rail from the single `DockArea`; Full, Compact, and Icon
follow actual width. Full and Compact keep truncating exchange/core labels. Icon uses the exchange
logo when known, a short text fallback when unknown, and a reduced connector/gap budget that leaves
core-label space at the minimum width. Tooltips retain the complete semantic label and status.
Ready is an enlarged green dot without duplicate text; errors and unavailable states keep visible
text in Full and diagnostic tooltips in Compact/Icon. A narrow window locally yields space to the
dock without overwriting the global preference, which returns when the window expands.

Exchange-logo loading is never performed by the virtual row closure. Auto entry joins a
process-wide single-flight prewarm on the background executor. Each Shell publishes its own ready
edge on the UI executor and repaints; before that edge headings render without resolving logos,
then each distinct exchange is resolved once while the flattened item list is built. Unknown
brands keep their label and receive no fabricated logo.

Auto uses one shared topology-only `auto_dock.json`: stable panel names encode split tree, sides,
sizes, and tab order, but panel payload, group IDs, active tabs, zoom, and `Rc` identities never
cross windows. Each Shell applies the topology to its own live panels; Backend revisions propagate
user topology edits to other Auto windows without feedback. `ChartTabs` remains first, visually
separate, drag-pinned, and protected against insertion before it. Other operational panels may be
reordered, split, and resized. Detaching an ordinary panel or chart tab is rejected before window
creation or persistence changes.

Classic keeps a separate local `DockNamedLayout` containing stable-name topology plus active-tab,
zoom, and split-size metadata. It does not hold opaque panel `Rc` identities; ordinary exact
identities remain in the live `DockArea` while the name-only layout changes. Auto's entry tab is
instead the per-group `auto_workspace_tab_by_group` preference in `layout.toml`. Eligible names are
`ChartTabs`, `Report`, `Assets`, `CoreStatus`, `Log`, and `Detects`; the lower `Orders` surface and
Classic-only `News` and `Alerts` (the Figures tab) are ineligible. Missing or stale values,
including either Classic-only name, fall back to `Report` without rewriting the stored value merely
because fallback was used. A real user activation, drag activation, or programmatic `ChartTabs`
reveal updates the preference. Programmatic mode/topology application keeps its guard active
through deferred
Dock-event delivery, preventing its `PanelActivated` and `LayoutChanged` events from becoming
either a tab preference or a shared-topology edit.

A missing `auto_dock.json` means first launch and allows the starting preset to be saved.
An unreadable or invalid file is a separate recovery state: the safe preset is shown
only in memory and does not overwrite the file until the user explicitly changes topology. Writing
Live-saving of `layout.toml`, the shared Auto topology and the Classic pair
`docks.json`/`detached.json` goes through one serial persistence worker. GPUI hands it only
immutable snapshots and polls acknowledgements; file open/write/flush/sync do not run in the
UI tick. The Classic pair is saved as one logical transaction through
`window-state.pending.json`: the journal is written before both public files and is deleted only after
two successful atomic replaces. After a crash, startup first replays the journal in full.
After an accepted enqueue the matching dirty flag is cleared; a new mutation or a failed
acknowledgement sets it again, so a temporary filesystem error is retried on the
next flush instead of silently losing the layout. On quit the last full snapshot
is placed behind the write already in flight, the worker is joined, and an unavailable or crashed worker gets
the only synchronous fallback exactly at the shutdown boundary. Repeated detached-specs with
the same `(group, panel)` are collapsed before native windows are created.

If `auto_dock.json` does not exist, the first Auto workspace receives a vertical operations preset:
the flexible upper tab stack contains pinned-leading `ChartTabs`, `Report`, `Log`, and the other
eligible operational tabs, while the single lower `Orders` surface receives four extra table rows
of height. The saved eligible group tab is activated after the preset is installed, or `Report`
when no valid preference exists. The first user reorder/split/resize replaces the preset with the
shared topology for every Auto window; Classic topology is neither read nor changed.

On Auto entry, the full Classic named layout remains local Shell runtime state. Each docked
Classic-only `News` or `Alerts` panel is removed before the shared topology is applied, so neither
name appears even when a stale valid `auto_dock.json` names it; Shell keeps the exact docked
identities in `Shell::classic_only_panels`. Returning to Classic supplies those identities while
applying the saved name-only `DockNamedLayout`, restoring active, zoom, sizes, placement, and local
view state. Classic panels already detached are temporarily closed without deleting their
specs/geometry and normally receive Auto-only dock instances, but detached `News` and `Alerts` are
excluded so Auto never creates duplicates. Closing a previously detached owned panel first removes
its exact live handle, then notifies Shell so Auto can restore the tab without a mode change. On
Classic return, temporary instances are removed and detached windows respawn from their original
specs after a real timer yield and renewed mode/ownership/shutdown checks.

Auto dock panels have no close buttons; every operational surface except the Classic-only `News`
and `Alerts` panels remains available for reorder/split/resize. Existing detached chart windows
remain open, but Auto cannot create new ones. `docks.json` and `detached.json` remain exclusively
Classic authorities and Auto events never rewrite them.

An existing detached chart may stay on another monitor as context, but every one of its
trading actions and every go-to-Main re-checks the group's current rail-scope. The old core's
chart stays visible, but cannot send a command or bypass the server choice through the left panel.

All previous `Backend::open_on_main` routes remain in force. On creation the Shell remembers the
already existing request revision, so the `ChartTabs` tab is programmatically activated only by new
revisions this Shell noticed after it was created and addressed to its Auto group; a request
that appeared before the window does not steal the starting tab. In Classic a reveal does not steal the active dock tab.
The request `activate` flag still separately decides whether to raise the OS-window. Before the observer
signature and consume, the target core is resolved again through the live session. A group-owned request carries
the immutable authority of the source window: if the core changed group, vanished, or left the current Auto
scope, the request is cancelled instead of moving the action. Only an explicitly unscoped internal/global request
may follow a live core into a new group. Detached chart windows remain independent in OS
ownership and do not gain an owner just because of Auto.

A concrete click on the rail becomes the only way to change the Auto core: every scoped panel
re-reads the effective scope, the header shows the same core as a passive indicator, and the current Main
chart is replaced or focused without accumulating new tabs. The market is kept by exact-match,
then through match-key/quote fallback; if there is no matching market, the chart does not change. Overview does not
pick an arbitrary core and does not retarget the Main chart. Classic selection and the Classic dock are not
changed.

A group-owned Report applies one contextual column lens to that effective scope. Only `AutoCore`
makes `core_name` unavailable; Auto Overview, Classic, and standalone Report continue to honor the
saved preference. The lens never mutates saved visibility, sort, or width state, and the table,
Columns menu, selection copy, and visible-columns export all use it consistently. The explicit
all-columns export remains the full runtime schema.

Logical ownership of singleton scope follows real interaction: native activation of a
group window or detached panel updates `WorkspaceFocus`. For a detached chart every native
activation does this without depending on the inactivity-close setting, and the existing
active chart interaction/polling path re-confirms the group while the user works in
the window. For charts this is not an OS owner/transient relationship — only routing of
`Analytics`/`Strategies`. Native focus/taskbar behaviour and the visual hit of the three responsive
tiers require a manual check in the built application; static and unit tests check only
programmatic decisions and invariants.

## MoonUI native contract

In MoonUI `WindowOptions` is extended with two fields:

- `WindowRelationship` - an independent window or an owned window with an owner handle.
- `WindowTaskbarVisibility` - whether to show a separate taskbar button where
  the platform supports per-window taskbar entries.

Default policy: owned windows are hidden from the taskbar, independent windows
are shown.

Backend mapping:

- Windows: `WindowRelationship::Owned` becomes a Win32 owned window through
  the owner `HWND`, without modal blocking of the parent. Dialog remains a separate modal
  logic. A group window's `AppUserModelID` is `MoonTerminal.<group>` when the group name is
  non-empty (`group_app_id`); every other factory sets `MoonTerminal`.
- macOS: an owned floating window is added as an AppKit child window over the owner and
  is excluded from the native Windows menu.
- Wayland: the owner becomes an xdg parent.
- X11: the owner becomes a transient parent, and the hidden taskbar policy sets
  `_NET_WM_STATE_SKIP_TASKBAR`.

## Window chrome: the closed API

Final contract: the terminal does not use `MoonWindowChrome` directly and does not
draw homemade `x`, `-`, `[]` in individual windows. For visual chrome and
native hit zones, `MoonWindowFrame` from MoonUI is used.

The old `MoonWindowChrome` is removed from the public `moon_ui` API. It was too
low-level: it gave hit zones and window-control areas, but did not own the visual
semantics of a window - brand, title cluster, colours, hover state and which
logo is allowed for a given window kind. That is why a debug/tool window
could again get the large wordmark of the main window. Now a screen does not
assemble chrome from parts; it picks a window kind through `MoonWindowFrameKind`.

`MoonWindowFrame` sets all of:

- window kind: `Main`, `Tool`, `Popup`, `DetachedPanel`, `DetachedChart`, `Debug`;
- window-control set: `None`, `Close`, `MinimizeClose`,
  `MinimizeMaximizeClose`;
- visual controls: symbols, colours, hover state, button size from MoonTheme;
- native control areas: `Min`, `Max`, `Close`;
- drag handle: `WindowControlArea::Drag`, double click -> native titlebar
  double click, mouse down -> native window move;
- hit overlay for those windows where the drag zone must be a separate transparent
  area over the header.

## Visual window kinds

MoonTerminal uses three visual window classes:

- Main window: the one primary terminal window. Only it has the full `MoonTerminal`
  wordmark in the header. In the API this is `MoonWindowFrameKind::Main`.
- Tool/secondary windows: Settings, Strategies, debug, detached chart and other
  auxiliary windows. They have a small mark without the Moonbot caption. In the API this is
  `Tool`, `DetachedPanel`, `DetachedChart`, `Debug`.
- Popup/overlay windows: compact windows with no branding. In the API this is `Popup`.

A screen does not pick the logo itself. Direct calls to terminal helpers such as
`logo_sized`, `logo_mark`, or drawing an SVG/logo by hand in the titlebar, are forbidden. Branding
is chosen by `MoonWindowFrame` from `MoonWindowFrameKind`:

- `Main` -> full logo;
- `Tool` / `DetachedPanel` / `DetachedChart` / `Debug` -> small mark;
- `Popup` -> no logo.

The only exception is the main window's full logo: `MoonWindowFrame` draws its own
Moonbot lockup, baked into the component, and the product is called MoonTerminal. Therefore
the main window's header assembles the brand cluster itself (`chrome/terminal_chrome.rs`):
the drag zone is still given by `MoonWindowFrame::drag_handle()`, and the logo and divider by
`design::header_logo` over the `assets/brand/` assets. Cluster geometry is the same as
`brand_cluster` (`CHROME_GAP`, a 16px rule), but the rule is drawn by `design::chrome_divider`:
once the terminal assembles the cluster, the seam follows the same contrast as the other seams
of the row, not the paler `border` from MoonUI. `brand_cluster` should come back exactly
when `MoonWindowFrame` learns to accept a foreign lockup. For mark windows and popups
the rule below holds unchanged.

For the titlebar zone use:

- `MoonWindowFrame::brand_cluster(cx)` - brand + separator without title;
- `MoonWindowFrame::title_cluster(title, cx)` - brand + separator + title;
- `MoonWindowFrame::visual_controls(cx)` - OS buttons;
- `MoonWindowFrame::drag_handle()` / `hit_overlay()` - native drag/hit zones.

Correct composition:

- `windowing.rs` opens the OS-window and sets owner/taskbar/app_id/decorations;
- the header visually draws the window's application content: brand, title, metrics,
  terminal buttons;
- `MoonWindowFrame::brand_cluster(...)` / `title_cluster(...)` draw the correct
  brand for the window kind;
- `MoonWindowFrame::visual_controls(...)` draws the window's OS buttons;
- `MoonWindowFrame::drag_handle()` is placed on the spacer zone;
- `MoonWindowFrame::hit_overlay()` is placed as the last child only where
  a separate transparent drag overlay is needed.

If a screen wants to "just put a logo" or "just draw an x",
that means MoonUI is missing the needed `MoonWindowFrameKind` or helper on
`MoonWindowFrame`. Fix the MoonUI contract, not the particular screen.
The only recorded exception is the main-window brand lockup above: until
`MoonWindowFrame` has a variant for a foreign logo, `design::header_logo` draws it
from `chrome/terminal_chrome.rs`. It is recorded here and pinned by a test, not
introduced in place — there must not be a second such exception.

Direct uses in the terminal UI are forbidden:

- `MoonWindowChrome::new`;
- `MoonWindowChromeButton`;
- `WindowControlArea::Drag`;
- `start_window_move`;
- `titlebar_double_click`;
- `logo_sized` / `logo_mark` outside the brand/helper layer itself;
- `design::header_logo` outside `chrome/terminal_chrome.rs` and opening a file by the
  `assets/brand/` path outside `design.rs` (naming the folder in a comment is allowed) — the logo
  is taken from `MoonWindowFrame`, and the only exception is described above;
- `WindowOptions { ... }` outside `windowing.rs`.

The ban is pinned by the test `terminal_windows_use_closed_window_frame_api` in
`crates/moon-ui-gpui/tests/theme_contract/`.

If a new window kind is needed, for example a non-standard round titlebar or
controls in the centre, add a new `MoonWindowFrameKind`/layout in MoonUI and
one factory in `windowing.rs`, rather than editing individual screens.

Generic detached panels (`Orders`, `Assets`, `Log`, `Report`) also count as
`DetachedPanel`, not "just a separate window with content". They must have a
custom titlebar through `MoonWindowFrame::detached_panel(...)` and open
through `detached_panel_window_options(...)`; otherwise we get a fourth
visual/behavioural window kind that is not in the design.

## Owner and taskbar policy

Do not call `cx.window_handle()` from a `Context` view/entity. `gpui::Context<'_, T>`
has no such API, and on restore of saved windows the current
`Window` physically does not exist.

Owner is used only for owner-aware window kinds:

- `tool_window_options`;
- `debug_window_options`;
- `detached_panel_window_options`.

For them the correct scheme is:

- live UI click: take `window.window_handle()` in the callback that has a `Window`,
  and pass `Some(owner)`;
- restore/startup: first try to find the live group window through
  `Backend.group_windows`; if no owner is found, pass `None`.

If a detached panel is restored without an owner, `detached::spawn` tries to
find the group window through `Backend.group_windows`. If no owner is found, the window
stays independent. This is normal restore behaviour.

Detached chart windows are a separate rule. They are NEVER owned, even on
runtime detach, because the OS owned/transient relationship raises the Main/group window
on a click on the chart. On a multi-monitor setup this looks like the main window jumping
on another screen. Therefore chart windows open only through
`detached_chart_window_options(...)`: their API has no owner, the window is
independent, and the separate taskbar button is suppressed by the shared mechanism (see below).
Trade windows follow that same ownership rule through `trade_window_options(...)`.

Final taskbar policy:

- `trading_window_options` - the visible main application window;
- `tool_window_options`, `debug_window_options`,
  `detached_panel_window_options` - hidden from the taskbar when there is an owner; on
  restore without an owner they become independent and may get a taskbar entry;
- `detached_chart_window_options` - always independent, but hidden from the taskbar;
- `trade_window_options` - the same combination, plus a minimum size;
- `profit_monitor_window_options` - the same combination: always independent, but hidden from the taskbar;
- `login_window_options` - always independent, and shown on the taskbar.

The current `Settings`, `Strategies` and `Assets` windows count as
`Tool/secondary`, so they open through `tool_window_options(...)`. If
a screen visually uses `MoonWindowFrame::tool(...)` but opens through
a standalone `WindowOptions` branch, that is an architectural error: the window looks
like part of the terminal, but the OS treats it as a separate application.

Profit Monitor is the deliberate exception to the usual visual-tool ownership rule. It keeps
`MoonWindowFrame::tool(...)` chrome because it is visually part of MoonTerminal, but its product
role is a separately placeable desktop widget, so its centralized factory carries no owner: a
minimized Main window leaves the monitor on screen and FancyZones can snap it. The terminal still
presents ONE taskbar icon, so the monitor's own button is suppressed exactly as a chart window's
is. Routes back to a minimized monitor: the terminal toolbar button (its `activate_window` restores
an iconic window) and Alt+Tab, which the window keeps because hidden taskbar visibility only drops
`WS_EX_APPWINDOW` and never applies the tool-window style. Do not copy that OS policy to ordinary
tool windows.

The login prompt is the other exception to tool-window ownership. It draws
`MoonWindowFrame::tool(...)` chrome, but it opens through `login_window_options`, not
`tool_window_options`. It can be the first and only window of the session, so it stays
independent, keeps a taskbar button, and carries the application icon.

Detached charts, trade windows, and Profit Monitor suppress the taskbar button with ONE
mechanism: `WindowTaskbarVisibility::Hidden` plus `hide_window_from_taskbar_soon`.
`WindowTaskbarVisibility::Hidden` by itself is not a guarantee: it only clears `WS_EX_APPWINDOW`,
which an unowned top-level window does not even need in order to get a button. `ITaskbarList::DeleteTab` removes
an already existing item and is NOT a permanent window state: the shell publishes the item shortly
after the window is shown and again on restoring a minimized window. Therefore a deletion burst
is armed when the window opens and again on every activation (`observe_window_activation`); the previous
burst is cancelled before the new one starts. COM, sleep and retries run outside GPUI, and the bounded burst does not
turn into standing work.
The real home for this logic is the Windows backend of the MoonUI fork (wndproc and the
`TaskbarCreated` broadcast are available there); until `Hidden` guarantees anything there, the compensation lives here.

Profit Monitor uses one monotonic pending create request: startup restore does not activate
the window, and a user open may promote an already pending request to foreground, but does not create
a second window. Native create runs only after a real timer yield and a repeated
shutdown check; a manual close clears only the matching `WindowId`, keeping the reopen flag during quit.

## Chart windows and UnderScene

The chart is not a GPUI UI component. It is drawn as an own-pass in UnderScene through
the chartdx/raw GPU path. Therefore the GPUI shell must reserve space for the chart, but
must not place an opaque quad over the plot/body.

Rule:

- chart/debug/detached-chart/trade-window root: `MoonBackgroundPolicy::NoFill`;
- header/chrome may be painted with `.bg(...)`;
- the body around `ChartPanel` must not be painted with `.bg(...)`;
- ordinary non-chart windows/panels may be opaque.

If the body around `ChartPanel` is painted, on macOS/Linux the native chart may
work by logs and counters but be visually empty: the GPUI background
covers the own-pass.

The contract is pinned by the test `crates/moon-ui-gpui/tests/theme_contract/`.

## Debug artifacts

Do not put screenshots, temporary logs, or live-test artifacts in `docs`.
Use `tmp/` for that; the folder must stay ignored.
