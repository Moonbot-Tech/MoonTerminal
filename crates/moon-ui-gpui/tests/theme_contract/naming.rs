//! Market naming: exactly one place decides what coin a market name is.

use super::support::*;

/// Protects the single naming mechanism against a second copy growing in a panel.
///
/// The plausible edit is what actually happened once: a panel needs the coin from a market name,
/// the shared helper does not quite fit its input, and it grows a local quote table with a local
/// split. Two copies then drift — the Log panel kept `PERP` as a quote while the Orders table did
/// not, and neither knew OKX spells a market `BEAT-USDT-SWAP`, so the same order showed a
/// different token depending on where you looked at it.
///
/// The ban is on OWNING the rules, not on naming a currency: a label, a tooltip or a locale key
/// may say USDT. What no panel may do is hold a LIST of quote currencies, which is the shape a
/// second parser always takes.
#[test]
fn quote_tables_live_only_in_moon_core_symbol() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    assert!(!sources.is_empty(), "no sources found under {root:?}");

    for path in sources {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
            .replace("\r\n", "\n");
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            // Two quote currencies side by side in one expression is a quote TABLE, whatever it
            // is called. One on its own is a label or a single lookup and stays allowed.
            let quotes = ["\"USDT\"", "\"USDC\"", "\"BUSD\"", "\"FDUSD\"", "\"TUSD\""];
            let hits = quotes.iter().filter(|q| code.contains(**q)).count();
            assert!(
                hits < 2,
                "{}:{}: a quote-currency table belongs in moon_core::symbol, not in a panel:\n{}",
                path.display(),
                index + 1,
                line.trim()
            );
        }
    }
}

/// The Orders table must render the coin the FEED resolved, not re-derive one from the name.
///
/// The plausible edit is calling `coin_of_market` in the cell again, which is cheaper to write and
/// silently wrong: the panel has no exchange, so it cannot reproduce the core's own token, and
/// this same value is what the coin menu writes into the core's and the strategy's blacklists.
#[test]
fn orders_token_cell_uses_the_resolved_coin() {
    let table = read_src("panels/orders/table.rs");
    let cell = fn_body(&table, "fn token_cell(");
    assert!(
        cell.contains("e.row.coin.clone()"),
        "the token cell must render OrderRow::coin"
    );
    // Matches a CALL, not the mention of one: a comment explaining why the table stopped calling
    // it must not fail the build.
    assert!(
        !table.contains("coin_of_market("),
        "the Orders table must not re-derive a coin from the market name"
    );
}

/// Every surface that shows a coin and HAS a core must ask the market source for it.
///
/// The plausible edit is reaching for `coin_of_market`, which is one call instead of two and reads
/// the market NAME — so it cannot name a Hyperliquid spot index (`@156`), cannot tell two COIN-M
/// expiries apart, and cannot reproduce the core's own foldings. That is exactly how the coin came
/// to read three different ways in three panels. This pins the surfaces that were converted; the
/// Log panel is absent on purpose, because it scans free text and has no core to ask.
#[test]
fn coin_surfaces_resolve_through_the_market_source() {
    for rel in [
        "controls/coin_search.rs",
        "chartdx/data_state/market.rs",
        "chart_tabs/custom.rs",
        "chrome/terminal_chrome.rs",
        "panels/alerts/mod.rs",
        "panels/detects/mod.rs",
        "panels/report/render.rs",
        "panels/report/columns.rs",
        "panels/chart/trade.rs",
    ] {
        let src = read_src(rel);
        assert!(
            src.contains("market_label(") || src.contains("market_labels("),
            "{rel}: a coin surface must resolve through MarketDataSource::market_label"
        );
        assert!(
            !src.contains("coin_of_market("),
            "{rel}: must not re-derive a coin from the market name"
        );
    }
}

/// The Main tab row must name each chart the way that chart's own caption names it.
///
/// The plausible edit is labelling the tab with the raw `entry.market` — it is right there on the
/// stack entry and looks like the market's name. It is the exchange's key, not a name: a
/// Hyperliquid spot index reads as `@156`, and two COIN-M expiries of one coin read alike. The
/// resolved label already exists on the panel, resolved once through `MarketDataSource` and
/// re-resolved when the catalog generation moves, so the row reads it rather than resolving again
/// — `market_label` takes the source lock and a snapshot and must not sit on a render path.
#[test]
fn the_main_tab_row_labels_charts_from_the_panels_own_ticker() {
    let main_stack = read_src("chart_tabs/main_stack.rs");
    let row = braced_body(&main_stack, "fn render_tab_row(");

    assert!(
        row.contains("pane_ticker()"),
        "the tab row must take its label from the panel's resolved ticker"
    );
    assert!(
        !row.contains("market_label(") && !row.contains("market_labels("),
        "and must not resolve it again on the render path: that takes the market-source lock"
    );
    assert!(
        !row.contains("coin_of_market("),
        "and must not re-derive a coin from the market name"
    );
}

/// Every visible exchange caption must come from the one venue-directory formatter.
///
/// Removing it from `controls/core_combo.rs:core_combo`, `screener/view.rs:source_combo`,
/// `chrome/terminal_chrome.rs:header`, `settings/connections/tab.rs:connections_tab`,
/// `profit_monitor/rows.rs:grouped_rows`, `panels/news/mod.rs:open_coin`,
/// `panels/detects/cards.rs:chip` or `shell/workspace.rs:render_rail_item` puts a core build's own
/// spelling back on that surface — which is how `Binance Quarterly` (COIN-M, code 6) rendered
/// without its brand while every other surface showed `Binance`. The Log selector captions from the
/// venue identity instead, because its selection outlives the cores that carry it.
#[test]
fn current_exchange_surfaces_share_display_policy_without_changing_identity() {
    for (rel, formatter) in [
        ("controls/core_combo.rs", "venue_section_label(venue)"),
        (
            "screener/view.rs",
            "crate::controls::venue_section_label(*venue)",
        ),
        (
            "chrome/terminal_chrome.rs",
            "crate::controls::venue_section_label(venue)",
        ),
        (
            "settings/connections/tab.rs",
            "crate::controls::venue_section_label(venue)",
        ),
        (
            "analytics/profit_monitor/rows.rs",
            "venue_section_label(venue)",
        ),
        ("panels/news/mod.rs", "crate::controls::venue_label(venue)"),
        (
            "shell/workspace.rs",
            "crate::controls::venue_section_label(venue.as_ref())",
        ),
        ("panels/detects/cards.rs", "crate::controls::venue_label("),
        (
            "panels/log/controls.rs",
            "crate::controls::venue_section_label(venue)",
        ),
    ] {
        let source = read_src(rel);
        assert!(
            source.contains(formatter),
            "{rel}: every visible exchange caption must pass through {formatter}"
        );
    }

    // A CLOSED rule, not a list: the caption is presentation and the platform code is identity, so
    // `CoreVenue::reported` — the one field carrying a core build's own spelling — may be read only
    // inside the formatter that displays it. Sweeping the whole crate means a panel added tomorrow
    // is covered by default, which an enumerated list cannot promise.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    rust_sources(&root, &mut sources);
    let mut readers = Vec::new();
    for path in sources {
        let rel = path.strip_prefix(&root).unwrap_or(&path).to_path_buf();
        // The formatter itself and its sibling tests — that exact module, not any file that
        // happens to be named `venue_label.rs`. Compared component by component because a literal
        // `"controls/venue_label"` never matches on Windows, where the separator is a backslash,
        // and an exemption that silently covers nothing reports PASS forever.
        let parts: Vec<_> = rel.components().map(|part| part.as_os_str()).collect();
        let exempt = matches!(parts.as_slice(), [first, second, ..]
            if *first == "controls"
                && (*second == "venue_label" || *second == "venue_label.rs"));
        if exempt {
            continue;
        }
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        for line in text.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains(".reported") {
                readers.push(rel.display().to_string());
                break;
            }
        }
    }
    assert!(
        readers.is_empty(),
        "a venue's reported caption belongs to controls/venue_label.rs alone, but it is read in \
         {readers:?}"
    );

    let log = read_src("panels/log/controls.rs");
    let source_combo = braced_body(&log, "pub(super) fn source_combo(");
    assert!(
        source_combo.contains("let exchange = venue.id;")
            && source_combo.contains("this.set_source(LogSource::Exchange(exchange), cx);"),
        "LogSource::Exchange must retain the venue identity, never its caption"
    );
    assert!(
        !source_combo.contains("LogSource::Exchange(exchange_label"),
        "the formatted Log label must never become its membership key"
    );
}
