# locales/

MoonTerminal interface localisation dictionaries. Format — rust-i18n `_version: 2`
(in each file, at the top). Each key → a `locale → string` map; every language
(`ru` / `en` / `es`) sits side by side, so a missing translation is visible at once.
Variable interpolation: `%{var}`.

rust-i18n merges **all** files in this folder into one global
key space — splitting into files is purely organisational, by UI area:

| File               | Area                                                          |
|--------------------|--------------------------------------------------------------|
| `shell.yml`        | top bar, status bar, toolbar, chart tab/window                |
| `crowd.yml`        | crowd stats on empty Main and its toggle                      |
| `strategies.yml`   | the Strategies window (tree, filter, parameters, context menu)|
| `orders.yml`       | the right-hand order panel + the orders table in the bottom dock |
| `dock.yml`         | bottom-dock tabs, detaching, log, panel stubs                 |
| `settings.yml`     | Settings window: shared chrome + tab labels                   |
| `interface.yml`    | the Interface tab (theme, `theme.toml`)                       |
| `general.yml`      | the General tab                                               |
| `connections.yml`  | the Connections tab (cores, groups, statuses, columns, tooltips) |
| `hotkeys.yml`      | the Hotkeys tab                                               |
| `security.yml`     | the Security block in General + the password login window     |
| `telegram_core.yml`| the Telegram tab: the selected core's built-in reader         |
| `report.yml`       | the Report panel (columns, filters, totals)                   |
| `assets.yml`       | the Assets window/panel (columns, wallets)                    |
| `dialogs.yml`      | create/rename/delete dialogs + shared buttons                 |
| `errors.yml`       | error messages (validation, GPU)                              |
| `common.yml`       | strings shared by several panels (loading, DB-read errors)    |
| `city.yml`         | city names for the header-clock zone picker                   |
| `workspace.yml`    | auto-trading mode, core navigation and availability statuses  |
| `core_expert.yml`  | the Core settings — expert mode window (Moonbot tabs, page states) |
| `sounds.yml`       | own sounds: the folder block on the Trade sounds tab, the Sound not found toast, the no file mark |

A new UI section → a new file; we name the key `<area>.<...>` (dot as the separator).

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
    the Settings → Lines tab in full (Buy/Sell/Stop/dashed/knots/…)
    technical Report columns: ID · TaskID · ExOrderID · Strat · BaseCur · Sell set
    three-letter city codes in the header clock (WAW · NYC · TYO …) — they are like tickers;
    the city NAMES themselves we do translate, they live in `city.yml`

## Glyph icons on buttons (the rule for wiring `t!` up later)

Glyphs (`⚙ ▶ ⏸ ↩ + ■ 🔍 ▾`) we **do not store** in dictionary values — the dictionary holds only
clean translatable text. On a button the glyph is placed as a **separate segment** next to
the text, as already done with the `🔍` magnifier in `controls.rs`:

```rust
MoonButton::new("settings")
    .segment(MoonButtonSegment::new("⚙"))            // glyph — not translated
    .segment(MoonButtonSegment::new(t!("shell.settings_btn")))  // text — from the dictionary
```

Why: the icon is the same in every language, a translator cannot lose it, and the strings
in `*.yml` stay clean. Real vector icons (`MoonButton::icon(path)` /
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

In the terminal code use `rust_i18n::t`, add new strings to a separate
`*.yml` by UI area.
