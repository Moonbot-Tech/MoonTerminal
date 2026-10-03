# locales/

MoonTerminal interface localisation dictionaries. Format — rust-i18n `_version: 1`
(the first real line of each file). Each file is one language: flat dotted keys,
one `key: "value"` (or a single-quoted scalar) per line. The shipped languages
are `ru`, `en`, `es`, and `uk`, each in its own folder. Variable interpolation:
`%{var}`.

A file lives at `locales/<lang>/<area>.<lang>.yml`. rust-i18n reads the locale
from the **last dot-segment of the filename**, so the stem must end in `.<lang>`
and that suffix must be the folder name. `locales/en/dock.yml` would register a
locale named `dock`, not English. `fallback = "en"` (wired below) is the string
used when the active language has no entry for a key.

rust-i18n merges **all** area files of a language into one global
key space — splitting into files is purely organisational, by UI area:

| File                         | Area                                                          |
|------------------------------|--------------------------------------------------------------|
| `shell.<lang>.yml`           | top bar, status bar, toolbar, chart tab/window                |
| `tick_volume.<lang>.yml`     | cursor volume readout and the hovered candle's bucket        |
| `update.<lang>.yml`          | the header update button: Windows self-update, macOS `.dmg`  |
| `crowd.<lang>.yml`           | crowd stats on empty Main and its toggle                      |
| `strategies.<lang>.yml`      | the Strategies window (tree, filter, parameters, context menu)|
| `screener.<lang>.yml`        | the Screener window's coin table                              |
| `orders.<lang>.yml`          | the right-hand order panel + the orders table in the bottom dock |
| `dock.<lang>.yml`            | bottom-dock tabs, detaching, log, panel stubs                 |
| `news.<lang>.yml`            | the News panel and the chart's news marks                     |
| `settings.<lang>.yml`        | Settings window chrome, tab labels, and the Badges tab        |
| `interface.<lang>.yml`       | the Interface tab (theme, `theme.toml`)                       |
| `lines.<lang>.yml`           | Lines tab attributes; line names stay English literals        |
| `general.<lang>.yml`         | the General tab                                               |
| `import.<lang>.yml`          | MoonBot settings import on the General tab                    |
| `storage.<lang>.yml`         | the Storage tab: reports, strategies, klines, replay prints   |
| `connections.<lang>.yml`     | the Connections tab (cores, groups, statuses, columns, tooltips) |
| `hotkeys.<lang>.yml`         | the Hotkeys tab                                               |
| `horizontal_ray.<lang>.yml`  | the Horizontal Ray figure name and its draw hotkey            |
| `security.<lang>.yml`        | the Security block in General + the password login window     |
| `telegram_core.<lang>.yml`   | the Telegram tab: the selected core's built-in reader         |
| `telegram.<lang>.yml`        | the Telegram Settings tab, the terminal bot, and Mini App     |
| `telegram_access.<lang>.yml` | Telegram owner and read-only viewer assignments               |
| `telegram_report.<lang>.yml` | Telegram report replies, period buttons, and help text        |
| `telegram_menu.<lang>.yml`   | the bot's configurable menu items, its inline section screens, and their Settings editor |
| `report.<lang>.yml`          | the Report panel (columns, filters, totals)                   |
| `trade_window.<lang>.yml`    | the trade window opened from a closed Report row              |
| `assets.<lang>.yml`          | the Assets window/panel (columns, wallets)                    |
| `analytics.<lang>.yml`       | the Analytics window and the desktop Profit Monitor           |
| `dialogs.<lang>.yml`         | create/rename/delete dialogs + shared buttons                 |
| `errors.<lang>.yml`          | error messages (validation, GPU)                              |
| `common.<lang>.yml`          | strings shared by several panels (loading, DB-read errors)    |
| `tables.<lang>.yml`          | shared table actions, including the column-width reset        |
| `city.<lang>.yml`            | city names for the header-clock zone picker                   |
| `workspace.<lang>.yml`       | auto-trading mode, core navigation and availability statuses  |
| `core_run.<lang>.yml`        | the shared core run control: up, detecting, and trading       |
| `core_status.<lang>.yml`     | the Core Status panel: health, warnings, problems, startup    |
| `core_update.<lang>.yml`     | the Core Status update queue, separate from `update.<lang>.yml` |
| `core_expert.<lang>.yml`     | the Core settings — expert mode window (Moonbot tabs, page states) |
| `sounds.<lang>.yml`          | own sounds: the folder block on the Trade sounds tab, the Sound not found toast, the no file mark |
| `trade_sounds.<lang>.yml`    | the Trade sounds tab: entry and exit sounds per exchange      |
| `station.<lang>.yml`         | the Station tab (remote server login and host-key errors)     |

A new UI section → a new area file in every language folder; we name the key `<area>.<...>` (dot as the separator).

## Adding a key

Write the same key, in every language folder, inside that area's file. The
contract below is what used to be "a missing translation is visible side by
side". It fails when a language is missing the key, has an extra key, or
names a different `%{var}`:

```
cargo test -p moon-ui-gpui --test theme_contract locales
```

## Adding a language

Add `locales/<code>/`, copy each English area file to
`<area>.<code>.yml` (the stem must end in `.<code>`), add a `Language`
variant whose code is that folder name (`crates/moon-core/src/config/lang.rs`,
including `Language::ALL`), then re-run the contract test above.

## Deliberately NOT translated

Industry standard / tech metrics we leave as they are in every language:

    BUY · SELL · Cancel Buy · PANIC SELL · LONG · SHORT · ON · OFF · Live
    Spot · Futures · Quarterly (market kinds in exchange names)
    Size · Sell · SL · TP · Lev · TS · VStop · Buy · Fill · Strat · Host · Port
    USDT eq. in the Size, USDT eq. caption — a technical equivalent label
    PRO · FREE (plan names)
    Hotkeys (the tab label in expert core settings — labelled the same in Moonbot itself)
    Copy to ClipBoard · Paste (the Settings transfer buttons — in Moonbot they are Latin in every language)
    HMAC · RSA · @MBOnlineBot · RU/EN/ES (the Login tab of expert settings — the same in Moonbot)
    Remote · System (section headings of the Special tab — in Moonbot Latin in every language)
    Orders Controls · Fixed Order Sizes · Fixed Sell Prices · Manual strategies (inner Hotkeys tabs — the same in Moonbot)
    Max Orders · Listen UDP port · UDP Commands Port/Pass · Control VDS IP (Special labels — the same in Moonbot)
    Add @TMoonBot to your channel · Generate PIN code · Reset channel · Cancel buys · Apply (Special buttons — the same in Moonbot)
    RTT · MTU (network abbreviations in core-start telemetry)
    the status-bar metrics line (ticks / book / fps / present / CPU / RAM)
    line names on the Settings → Lines tab (Buy/Sell/Stop/…); dashed, crosses and knots are translated in `lines.<lang>.yml`
    technical Report columns: ID · TaskID · ExOrderID · Strat · BaseCur · Sell set
    three-letter city codes in the header clock (WAW · NYC · TYO …) — they are like tickers;
    the city NAMES themselves we do translate, they live in `city.<lang>.yml`

## Glyph icons on buttons (the rule for wiring `t!` up later)

Glyphs (`⚙ ▶ ⏸ ↩ + ■ 🔍 ▾`) we **do not store** in dictionary values — the dictionary holds only
clean translatable text. On a button the glyph is placed as a **separate segment** next to
the text, as the scale dropdown does with the `🔍` magnifier in
`crates/moon-ui-gpui/src/controls/scale.rs`:

```rust
MoonDropdown::new(dropdown_id)
    .segment(MoonButtonSegment::new("🔍").color(p.text_muted)) // glyph — not translated
    .segment(MoonButtonSegment::new(trigger_val).color(p.text))
```

Why: the icon is the same in every language, a translator cannot lose it, and the strings
in `locales/<lang>/<area>.<lang>.yml` stay clean. Real vector icons (`MoonButton::icon(path)` /
`leading_icon`/`trailing_icon`) have nothing to do with locale — they need not be touched.

Text and tooltips in every MoonUI component take `Into<SharedString>`, so
`t!("key")` is substituted directly: `.label()`, `.segment()`, `.text_segment()`,
`.tooltip()`, `MoonTooltipView::new()`, `MoonMenuItem::with_key(id, …)`,
`MoonDataTableColumn::new(id, …)`.

## Connection status (important)

The dictionaries are wired in `crates/moon-ui-gpui/src/main.rs` through:

```rust
rust_i18n::i18n!("../../locales", fallback = "en");
```

In the terminal code use `rust_i18n::t`. Add new strings to the area file in
every language folder (`locales/<lang>/<area>.<lang>.yml`).
