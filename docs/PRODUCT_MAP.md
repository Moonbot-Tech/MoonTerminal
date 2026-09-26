# MoonTerminal as a product — a feature map and a dictionary of users' words

For whoever answers user questions about the terminal — people and AI agents.
The document's job: quickly understand WHAT the user is asking about, and know what the terminal
HAS and what it does NOT. Truth about behaviour is always the code; the doc is navigation to it.
Current as of v0.42.3 (11.09.2026). Grown with releases: a new
feature — a row in the map, a new user word — a row in the dictionary.

## How to find a feature by name

1. `locales/*.yml` — all UI labels (`grep -rni "слово" locales/`). Each key carries `ru`, `en` and `es` values, so an English reader can grep either side.
   Found a string — the key is next to it, and the code is grepped by that key. Files by area:
   shell, interface, settings, hotkeys, orders, report, analytics,
   strategies, screener, news, connections, core_status, core_run,
   core_settings, dialogs, crowd.
2. The dictionary below — users' words that are not in the labels.
3. `docs/ARCHITECTURE.md` — chapters on the mechanics (Rendering, Data Path, manual
   trading, Core order, Classic/Auto, venue directory, editing strategies,
   Report replication, USDT valuation, backups, Telegram, windows, self-update).
4. Found nowhere — the feature most likely does not exist; but that is settled by the code,
   not by a miss in grep.

## Code map

- `crates/moon-core` — data and state: config, themes, report and
  analytics DBs, figures, venue, market/source (order books, ticks).
- `crates/moon-chart` — chart drawing (layers, figures, volumes).
- `crates/moon-ui-gpui` — panels and windows: chartdx (render state, shaders
  DX11/native/Metal, texts), panels/, settings/, analytics/, strategies/,
  chart_tabs/.
- MoonUI (separate repo Moonbot-Tech/MoonUI, pin in `Cargo.lock`) — base
  UI components: pickers, tables, popups.
- MoonProto (Moonbot-Tech/MoonProto) — Rust SDK for talking to the core: commands,
  subscriptions.

## Dictionary: what users' words mean

- **«Карта ордеров»** (“order map”) = an order-book heatmap over time, as in Bookmap (book
  walls in colour on history). There is no such feature. #489 was closed: that reading was
  not the request. The open ask is #496, Moonbot's HMap — large-trade dots under the
  bottom volumes. Do not confuse either with the
  **order book on the chart** — that exists: cumulative bid/ask walls on the sides of
  the chart + thin lines of individual levels on top (Settings → Interface →
  Order book; drawn by the shaders, the same in DX11/native/Metal). The word “heatmap”
  in the code is only about Analytics (profit).
- **«Превью графиков»** (“chart previews”) = Moonbot charts: small detect chart-cards
  that open into the main chart on click.
- **«Тики»** (“ticks”) — trade dots on the chart (buy/sell/liquidation); colours
  are baked as constants in the shaders, only size and opacity are configurable
  (#490).
- **«Вспышка» / «моргание»** (“flash” / “blink”) — the pulse frame of a chart that arrived on a tab
  from a detect (3 pulses, colour — the theme accent; turned off in the tab setting).
- **«Каптионы» / подписи** (“Captions”) — caption modules around the chart (background deltas,
  funding, rate, Session counter, countdown to candle close…): their own editor,
  layout on two axes, per-tab setting.
- **«Реплей» / «окно сделки»** (“replay” / “Trade window”) — viewing a closed trade: candle context
  6h before entry / 2h after exit, tick detail around the position,
  history from the core's archives.
- **«Супер-растяжка»** (“super stretch”) — Moonbot's Ctrl+Shift+wheel zooms time down to
  3 seconds. Plain wheel and Ctrl+wheel still stop at 30 seconds. The super-zoom hotkeys
  use that same 3-second floor and are unbound by default (#493).
- **«Фигуры»** (“Figures”) — drawing: Segment, Ray, Rectangle, Triangle,
  “Position”, two kinds of Fibonacci (including Moonbot's), fills, line styles,
  Ctrl+Z, magnet on Ctrl, figure alerts with sound and flags.
- **«Флешки»** (“screenshot dumps”) — batches of screenshots with bugs and UX complaints from Kostya
  (@kostmain), a genre of the project chat; after each — a wave of fixes.

## Feature map by area

**Workspaces**: Classic and Auto (AUTO mode — an overview of Auto cores:
core rail, their own panels, problem cores highlighted); a core can be assigned
to a workspace preset.

**Header**: MANUAL/AUTO mode, the active core, the Real/Emulator badge, balance,
the MS toggle, SPR, Hook, Buy/Sell percentages, SL/TS, BTC rate and deltas, the clock with
city picker (the whole terminal lives in the chosen zone), Sleep mode on a
schedule. Second row — order size, leverage, MAX, SL/TP, sell
percentages. Clusters with dividers; on a narrow window it compresses in a cascade.

**Chart**: tick trail + candles with TF, history is pulled from the core's
archives, a “trades only” zone, the order book on the sides (walls + level lines),
volumes at the bottom (in quote currency), horizontal volumes by price beside the
plot (Moonbot's HVol), bought/sold volume by the chart,
volume measure around the cursor, liquidation count, news marks (gems by
tags, Ctrl-hover card), own-trade markers, chart stacks/columns,
detach into separate windows, chart screenshot to the clipboard (with the header baked in), favourite
coins and a temporary coin ban (as in MB), coin search with tabs.

**Detects**: cards with configurable badges (own colours — a shared HEX history,
see Other), the fired strategy's name, per-tab feed limit,
AddToChart detects,
sounds; the core colour (from Connections) paints the list rows.

**Manual trading**: the window group shares the visible Size/TP/SL — the order goes out
exactly with the visible parameters; SYNC mode across several cores; Moonbot
gestures and hotkeys (split, sells-to-zone, shift orders by %, bulk move to
click price, HotKey figure switcher); Panic Sell; hedge checkbox; per-coin
exchange limits (money minimum, order and leverage ceiling); the manual
strategy is owned by the core (MS/Hook popups, Moonbot's stop rule).

**Report**: a full local replica of every core's report DB (checkpoint +
live-row map, survives drops), USDT valuation at trade time
(two modes, COIN-M in BTC), filters on everything, multi-select of rows, marking
deleted, comments, the core log for a trade.

**Analytics**: KPI summary, profit calendar (year in GitHub style / month),
live Profit Monitor (by cores/groups, start/stop cores right
from the table), Tuner: “what-if” on report fields, Beam search over combinations,
By coin and By time axes (heatmap sliders for week/day/hour), an Entry/Exit
axis that replays every closed trade on its recorded tape of prints and searches
the strategy's entry and exit fields (MoonShot corridor, MoonHook take, sell line,
stops, delta modifiers) on the trades the model reproduces, a check
on held-out data, writing thresholds back into the core's strategy; history of
strategy versions with each version's profit; a strategy-name mask everywhere.

**Strategies**: a tree with file-manager operations, order and folders —
as on the core, human labels under Moonbot field names (on by
default), editing a field = the core confirms it actually applied,
filter by exchange, copy between cores.

**Core status**: grouping by servers/IP, live CPU/RAM/ping charts
(client↔core and core→exchange), alerts with thresholds/history/sound and marks
on the trading chart, the Problems tab (core diagnostics + actions),
“why the core will not connect”, API-key expiry, API-request quota,
core updates from the terminal (multi-select, a named build).

**Connections**: a core = key + address, core colour, MoonProto transport mode
per-core (a hint for “the one that works” from the user's bots' experience),
servers.enc bound to the machine, moved with a password.

**Other**: Screener (deltas in Moonbot ranges), the news panel
(per-core feed, tag filters, jump to a coin), Log (core lines, addresses
redacted), Assets (wallets by exchange, PnL, spot Market Sell), Crowd
(crowd stats on empty Main, a “loud coin” as a detect), its own
Telegram bot (reports to chat, navigation, per-core rights, Mini App), hotkeys
(keyboard + mouse on every action, conflicts highlighted), themes
(Light/Dark/Graphite + editor), a single colour picker (one palette
everywhere — cores, strategies, lines, figures, badges, news tags; a custom HEX
is remembered app-wide, last 20, the grid scrolls),
locales ru/en/es, self-update
(a check every 15 min, no restart), Settings and strategy backups
(`backups/`, daily), one instance per install folder, FireTest
(`chart-smoke`) — the built-in chart bench.

**Your own sounds**: the `sounds/` folder next to the exe (on macOS — in the data directory),
drop a `.wav` — it appears in every sound list (strategies, trade
sounds, core alerts, alerts); the “Your own sounds” block on the Trade sounds tab
shows the folder, what loaded, own numbers and what was rejected (PCM
WAV only), the “Open folder”/“Rescan” buttons; the exchange table on that tab
scrolls on its own, the block is pinned to the bottom. A file named like a built-in sound
(`ding1.wav`) replaces it. In the core's settings sounds are stored by NUMBER: 1–18 —
built-ins in Moonbot order, an own number is claimed by the file name
`19_MySound.wav` (the number is pinned to the file, adding other files does not
move it); a file without a number cannot be picked in the core's settings; a taken/reserved
number and a taken name are rejected with a reason. There is no sync with the core and there
will not be: the core does not play the sound, files do not go to it; a Moonbot strategy with
a sound name that is not in the folder plays `ding1` and shows a toast with the name —
the same for a number nobody holds, and for a deleted file. Silence is
only by an explicit Silent/`NONE`.

## Known gaps (frequently requested)

A % ruler without creating a figure (#485), and Moonbot's HMap — large trades under
the volume bars (#496). A Bookmap-style order-book heatmap is also absent; #489 was
closed because that description was the wrong ask. Before
answering the user “no” or “yes” — check the open issues and the code:
the list grows, and something on it may already have been done.
