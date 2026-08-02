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
        "panels/alerts.rs",
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
