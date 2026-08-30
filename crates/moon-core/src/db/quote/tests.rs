//! Persisted quote-currency decoding and safe-breakdown regression tests.

use super::*;

/// Every persisted MoonBot quote ordinal keeps its exact ticker identity.
///
/// Reordering or mistyping any arm in `QuoteCurrency::from_report_ordinal` must fail this
/// independently transcribed contract table, otherwise historical PnL is labeled as another
/// asset.
#[test]
fn persisted_ordinals_keep_distinct_quote_identities() {
    let expected = [
        "BTC", "USDT", "ETH", "BNB", "AUD", "TUSD", "BRL", "USDH", "USDC", "FDUSD", "AEUR", "USD",
        "TRX", "RUB", "EUR", "HTX", "USDD", "IDR", "DOGE", "TRY", "USDE",
    ];
    let actual = (0..=20)
        .map(|ordinal| {
            QuoteCurrency::from_report_ordinal(ordinal)
                .expect("known persisted ordinal")
                .ticker()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert!(QuoteCurrency::from_report_ordinal(21).is_none());
    assert!(QuoteCurrency::from_report_ordinal(25).is_none());
    assert!(QuoteCurrency::from_report_ordinal(26).is_none());
    assert!(QuoteCurrency::from_report_ordinal(255).is_none());
    assert_eq!(
        QuoteCurrency::from_report_value(&Value::Real(8.0)),
        None,
        "non-integer SQLite storage must not inherit a currency"
    );
}

/// The conversion resolver must not carry a private, incomplete copy of the quote universe.
///
/// Breakage: omitting a newly persisted ordinal from `QuoteCurrency::all`. The user-visible
/// consequence is that a valid two-leg historical route can never be discovered for that quote.
#[test]
fn ordered_quote_iterator_covers_every_persisted_identity() {
    let currencies = QuoteCurrency::all().collect::<Vec<_>>();
    assert_eq!(currencies.len(), 21);
    assert_eq!(currencies.first().copied(), Some(QuoteCurrency::btc()));
    assert_eq!(currencies.get(1).copied(), Some(QuoteCurrency::usdt()));
    assert_eq!(
        currencies.last().map(|currency| currency.ordinal()),
        Some(20)
    );
    assert!(
        currencies
            .windows(2)
            .all(|pair| pair[0].ordinal() + 1 == pair[1].ordinal())
    );
}

/// Plausible regression: removing the currency key from `QuoteBreakdown::from_groups` must fail
/// the bucket assertions, otherwise USDT and USDC are silently added into one false total.
#[test]
fn breakdown_merges_only_identical_known_quotes() {
    let totals = QuoteBreakdown::from_groups([
        (Some(1), 10.0, 2),
        (Some(8), 3.0, 1),
        (Some(1), -2.0, 1),
        (None, 9_999.0, 4),
        (Some(26), 8_888.0, 5),
    ]);

    assert_eq!(totals.orders, 13);
    assert_eq!(totals.unknown_orders, 9);
    assert_eq!(totals.totals.len(), 2);
    assert_eq!(totals.totals[0].currency.ticker(), "USDT");
    assert_eq!(totals.totals[0].profit, 8.0);
    assert_eq!(totals.totals[0].orders, 3);
    assert_eq!(totals.totals[1].currency.ticker(), "USDC");
    assert_eq!(totals.totals[1].profit, 3.0);
    assert_eq!(totals.scope(), QuoteScope::Unknown);
    assert_eq!(totals.traded_volume, TradedVolume::default());
}

/// Merging physical sources must keep each quote's reconstruction count beside its own subtotal.
/// Collapsing that pair back into an optional amount blanks the USDT bucket over one unprovable
/// row; reusing profit coverage instead would suppress the complete USDC bucket in this fixture.
#[test]
fn traded_volume_merges_quotes_and_keeps_each_bucket_reconstruction_count() {
    let volume = TradedVolume::from_groups([
        (Some(1), 2, 2, 420.0, 2, 840.0),
        (Some(1), 1, 0, 0.0, 0, 0.0),
        (Some(8), 1, 1, 75.0, 1, 75.0),
        (None, 2, 0, 0.0, 0, 0.0),
        // A source group with no eligible closed trade must not create a zero-valued bucket.
        (Some(0), 0, 0, 0.0, 0, 0.0),
    ]);

    assert_eq!(volume.eligible_orders, 6);
    assert_eq!(volume.reconstructed_orders, 3);
    assert_eq!(volume.valued_orders, 3);
    assert_eq!(volume.unknown_orders, 2);
    assert_eq!(volume.scope(), QuoteScope::Unknown);
    assert_eq!(volume.totals.len(), 2);
    assert_eq!(volume.totals[0].currency.ticker(), "USDT");
    assert_eq!(volume.totals[0].orders, 3);
    assert_eq!(
        volume.totals[0].reconstructed, 2,
        "the unprovable row shortens the USDT bucket instead of erasing it"
    );
    assert_eq!(volume.totals[0].amount, 420.0);
    assert_eq!(volume.totals[1].currency.ticker(), "USDC");
    assert_eq!(volume.totals[1].orders, 1);
    assert_eq!(volume.totals[1].reconstructed, 1);
    assert_eq!(volume.totals[1].amount, 75.0);
    assert_eq!(
        volume.usdt, None,
        "the unified figure stays complete-only whatever the native buckets publish"
    );
}

/// Columns a report source carries, as the SQL builders discover them.
fn columns(names: &[&str]) -> std::collections::HashSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// Evaluate the effective-quote expression over a seeded fixture, newest row first.
///
/// Args:
///     rows: `(basecurrency, fname)` pairs, inserted in order.
///
/// Returns:
///     Effective ordinal per row, `None` for SQL NULL.
fn effective_ordinals(rows: &[(Value, Option<&str>, &str)]) -> Vec<Option<i64>> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (id INTEGER PRIMARY KEY, basecurrency, fname TEXT, coin TEXT)",
    )
    .unwrap();
    for (index, (quote, fname, coin)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO orders_rep (id, basecurrency, fname, coin) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![index as i64, quote, fname, coin],
        )
        .unwrap();
    }
    let expression = effective_ordinal_expr("r", &columns(&["basecurrency", "fname", "coin"]));
    let sql = format!("SELECT ({expression}) FROM orders_rep r ORDER BY r.id");
    let mut statement = conn.prepare(&sql).unwrap();
    let values = statement
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    values
}

/// Plausible regression: widening the COIN-M rule to one fact — the contract shape alone (a USD-M
/// core trades the same `ETH_0926`), the filename alone (its first segment is a user-named
/// strategy, so `USD-hedge` would relabel a whole USD-M core), or an unanchored `%USD-%` (which
/// `mtf_USD-scalp` satisfies). Each of those mistakes is worth one row here, and each would value a
/// +12.50 USDT trade at the BTC rate.
#[test]
fn a_row_moves_only_when_the_market_spelling_and_the_contract_shape_agree() {
    let ordinals = effective_ordinals(&[
        // COIN-M: the core spells its market `USD-<COIN>` and mislabels the money as USDT.
        (
            Value::Integer(1),
            Some("Pump_USD-UNI_RP_02-24-16-01-51_2.bin"),
            "UNI_RP",
        ),
        (
            Value::Integer(1),
            Some("BinanceQ_USD-ETH_0926_09-09-2025 07-57-19_2.bin"),
            "ETH_0926",
        ),
        // A bare filename with no source segment still names the market it starts with.
        (
            Value::Integer(1),
            Some("USD-ADA_RP_11-08-2026.bin"),
            "ADA_RP",
        ),
        // The rotated spelling MoonBot writes for 1 476 rows of this replica: market `USD-DOT_0329`
        // stored as `TUSD-DO_0329`. Anchoring the marker to a `_` boundary loses every one.
        (
            Value::Integer(1),
            Some("Pump_TUSD-DO_0329_03-27-01-30-12_2.bin"),
            "DOT_0329",
        ),
        (
            Value::Integer(1),
            Some("Pump_BUSD-BN_0630_03-27-15-01-11_2.bin"),
            "BNB_0630",
        ),
        // USD-M dated contract on a USDT core: same coin shape, different market spelling.
        (
            Value::Integer(1),
            Some("Pump_USDT-ETH_0927_08-05-01-10-55_2.bin"),
            "ETH_0927",
        ),
        // A strategy someone named after the marker, on an ordinary USDT market.
        (
            Value::Integer(1),
            Some("USD-hedge_APRUSDT_11-08-2026 07-13-40_2.bin"),
            "APR",
        ),
        (
            Value::Integer(1),
            Some("mtf_USD-scalp_APRUSDT_11-08-2026.bin"),
            "APR",
        ),
        // The collision the contract shape alone cannot resolve: that same strategy name on a
        // USD-M DATED contract, whose coin has exactly the COIN-M shape. The market segment still
        // spells the quote in full, which is what vetoes it.
        (
            Value::Integer(1),
            Some("USD-hedge_USDT-ETH_0927_08-05-01-10-55_2.bin"),
            "ETH_0927",
        ),
        // The marker on a market with no contract shape at all.
        (
            Value::Integer(1),
            Some("Pump_USD-SOMETHING_11-08-2026.bin"),
            "SOMETHING",
        ),
        // Plain USDT row, and one whose market cannot be proven at all.
        (
            Value::Integer(1),
            Some("BinanceF_APRUSDT_11-08-2026 07-13-40_2.bin"),
            "APR",
        ),
        (Value::Integer(1), None, "UNI_RP"),
        // A currency the core named deliberately keeps it, whatever its market is called.
        (
            Value::Integer(8),
            Some("HyperF_USD-AAVE_RP_01-01-2026 00-00-00_2.bin"),
            "AAVE_RP",
        ),
        (
            Value::Integer(0),
            Some("Pump_USD-BTC_0927_02-24-16-01-51_2.bin"),
            "BTC_0927",
        ),
        // Untrusted storage classes stay unknown rather than inheriting a neighbour's identity.
        (
            Value::Real(1.0),
            Some("Pump_USD-UNI_RP_02-24-16-01-51_2.bin"),
            "UNI_RP",
        ),
        (
            Value::Text("1".into()),
            Some("Pump_USD-UNI_RP_1.bin"),
            "UNI_RP",
        ),
        (Value::Null, Some("Pump_USD-UNI_RP_1.bin"), "UNI_RP"),
    ]);

    assert_eq!(
        ordinals,
        vec![
            // Three COIN-M spellings, two of them rotated.
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            // Everything the three facts refuse to move.
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            Some(1),
            // Currencies the core named deliberately.
            Some(8),
            Some(0),
            // Untrusted storage classes.
            None,
            None,
            None,
        ]
    );
}

/// Plausible regression: a future rule that states no veto, or no contract shape, building `()` and
/// failing to prepare. Every money query names this expression, so that mistake would not degrade
/// one column — it would take the Report, Analytics and the valuation worker down together, and the
/// live rule's own non-empty lists would hide it from every other test here.
#[test]
fn a_rule_with_an_empty_fact_list_still_builds_preparable_sql() {
    let rule = DenominationRule {
        market_markers: &["%USD-%"],
        excluded_markers: &[],
        contract_shapes: &[],
        labeled: QuoteCurrency::usdt(),
        denominated: QuoteCurrency::btc(),
    };
    let guard = rule
        .guard_sql("r", &columns(&["basecurrency", "fname", "coin"]))
        .expect("a source carrying both facts evidences the rule");
    assert!(!guard.contains("()"), "empty parentheses in SQL: {guard}");

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE orders_rep (basecurrency, fname TEXT, coin TEXT)")
        .unwrap();
    conn.prepare(&format!("SELECT ({guard}) FROM orders_rep r"))
        .expect("the guard must prepare");
}

/// Plausible regression: naming `fname` unconditionally in the expression. A source that predates
/// the column would fail to prepare EVERY statement quoting money, taking the Report and Analytics
/// down together rather than leaving those rows with their persisted identity.
#[test]
fn a_source_without_the_market_column_keeps_persisted_identities() {
    for available in [&["basecurrency"][..], &["basecurrency", "fname"][..]] {
        let expression = effective_ordinal_expr("r", &columns(available));
        assert!(
            !expression.contains("fname") && !expression.contains("coin"),
            "a rule fired without both of its facts: {expression}"
        );
    }
    let expression = effective_ordinal_expr("r", &columns(&["basecurrency"]));

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE legacy (basecurrency);
         INSERT INTO legacy (basecurrency) VALUES (1), (8), ('x')",
    )
    .unwrap();
    let sql = format!("SELECT ({expression}) FROM legacy r");
    let mut statement = conn.prepare(&sql).unwrap();
    let ordinals: Vec<Option<i64>> = statement
        .query_map([], |row| row.get::<_, Option<i64>>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(ordinals, vec![Some(1), Some(8), None]);

    assert_eq!(effective_ordinal_expr("r", &columns(&["coin"])), "NULL");
}

/// Plausible regression: letting `GROUP BY` fall back to the raw column while the projection
/// corrects it. SQLite would then bucket a COIN-M row under BTC in the SELECT and under USDT in the
/// grouping, and the Report footer would report two totals for one set of trades.
#[test]
fn the_grouped_quote_is_the_same_expression_it_selects() {
    let cols = columns(&["basecurrency", "fname"]);
    let (quote, group_by) = trusted_quote_group("r", &cols);
    assert_eq!(quote, effective_ordinal_expr("r", &cols));
    assert_eq!(group_by, format!(" GROUP BY {quote}"));

    let (quote, group_by) = trusted_quote_group("r", &columns(&["coin"]));
    assert_eq!(quote, "NULL");
    assert!(group_by.is_empty());
}

/// Plausible regression: a new reader — a panel, an export, another aggregate — reaching for
/// `r.basecurrency` directly instead of [`effective_ordinal_expr`]. Nothing would fail to compile
/// and no test of that reader would fail; its money would simply be valued in the currency the core
/// mislabeled it with, disagreeing with every other surface. Unified sources are exempt on purpose:
/// they read `o.basecurrency` from a projection this module already corrected.
#[test]
fn no_reader_selects_the_raw_quote_column_behind_this_module() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db");
    let mut offenders = Vec::new();
    let mut pending = vec![root];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap().path();
            if entry.is_dir() {
                pending.push(entry);
                continue;
            }
            let name = entry.file_name().unwrap().to_string_lossy().to_string();
            let is_this_module = entry.ends_with("quote.rs") || entry.ends_with("quote/tests.rs");
            if entry.extension().is_none_or(|ext| ext != "rs")
                || is_this_module
                || name == "tests.rs"
                || name == "test_support.rs"
            {
                continue;
            }
            let text = std::fs::read_to_string(&entry).unwrap();
            for (number, line) in text.lines().enumerate() {
                // Both spellings the SQL builders use: bare and double-quoted, under either the
                // literal `r` alias or the `{alias}` a builder interpolates.
                let raw = ["r.basecurrency", "{alias}.basecurrency"]
                    .iter()
                    .any(|form| line.contains(form) || line.contains(&form.replace('.', ".\\\"")));
                if raw {
                    offenders.push(format!("{}:{}", entry.display(), number + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "read the quote through quote::effective_ordinal_expr instead: {offenders:?}"
    );
}

// ── COIN-M liquidations: two eras ───────────────────────────────────────────

/// Evaluate the settled amount and the effective quote over a liquidation fixture.
///
/// Args:
///     rows: `(closedate, sellreason, coin, fname, buyprice, spentbtc)` fixtures.
///
/// Returns:
///     `(settled spentbtc, effective ordinal)` per row.
fn settled_and_quote(
    rows: &[(i64, i64, &str, &str, Option<&str>, f64, f64)],
) -> Vec<(f64, Option<i64>)> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE orders_rep (id INTEGER PRIMARY KEY, core_uid INTEGER, closedate INTEGER,
             sellreason TEXT, coin TEXT, fname TEXT, buyprice REAL, spentbtc REAL,
             basecurrency INTEGER)",
    )
    .unwrap();
    for (index, (core, close, reason, coin, fname, price, spent)) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO orders_rep (id, core_uid, closedate, sellreason, coin, fname, buyprice,
                 spentbtc, basecurrency) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
            rusqlite::params![index as i64, core, close, reason, coin, fname, price, spent],
        )
        .unwrap();
    }
    let cols = columns(&[
        "basecurrency",
        "core_uid",
        "fname",
        "coin",
        "sellreason",
        "closedate",
        "buyprice",
        "spentbtc",
    ]);
    // A nameless liquidation is only recognizable once the core is known to own COIN-M rows, so
    // the fixture proves that the same way production does — from a NAMED row of that core. The
    // knowledge is remembered per core, so this fixture must start from nothing or a sibling
    // test's core ids would already count as examined.
    super::coin_m::reset();
    learn_coin_m_cores(
        &conn,
        &[crate::db::ReadSource {
            table: "orders_rep",
            cols: cols.clone(),
            legacy: false,
        }],
    );
    let sql = format!(
        "SELECT ({}), ({}) FROM orders_rep r ORDER BY r.id",
        settled_amount_expr("r", &cols, "spentbtc"),
        effective_ordinal_expr("r", &cols),
    );
    let mut statement = conn.prepare(&sql).unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get::<_, f64>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The core books a COIN-M liquidation two different ways, and the boundary between them is a
/// measured date. Getting either era wrong is catastrophic in a different direction: era two left
/// alone shows 33 750 USD of real liquidations as 19 cents, while applying era two's correction to
/// era one would multiply a 129-dollar loss by the coin's price.
#[test]
fn a_coin_m_liquidation_is_corrected_per_era() {
    const BEFORE: i64 = 1_693_492_380; // 2023-08-31 14:33 UTC, the last row of era one.
    const AFTER: i64 = 1_711_700_027; // 2024-03-29 08:13 UTC, the first row of era two.
    let out = settled_and_quote(&[
        // Era one: the stored value already IS the margin in USD, which the USDT label fits.
        (
            7,
            BEFORE,
            "LIQUIDATION",
            "THETA_RP",
            None,
            14.59854,
            129.197_080,
        ),
        // Era two: the value is margin-in-BTC divided by the price, so the price restores it,
        // and the row is BTC from then on.
        (
            7,
            AFTER,
            "LIQUIDATION",
            "BNB_0927",
            None,
            613.253,
            0.000_018_264_459,
        ),
        // The named row that proves core 7 is COIN-M at all.
        (
            7,
            AFTER,
            "Sell Price",
            "ETH_0927",
            Some("Pump_USD-ETH_0927_x"),
            3_600.0,
            0.5,
        ),
    ]);

    let (era_one, quote_one) = out[0];
    assert!((era_one - 129.197_080).abs() < 1e-9, "era one is untouched");
    assert_eq!(quote_one, Some(1), "era one keeps its USDT label");

    let (era_two, quote_two) = out[1];
    let expected = 0.000_018_264_459 * 613.253;
    assert!(
        (era_two - expected).abs() < 1e-12,
        "era two restores margin in BTC: {era_two} vs {expected}"
    );
    assert_eq!(quote_two, Some(0), "era two settles in BTC");
}

/// The correction is keyed on the liquidation, not on the date alone: an ordinary COIN-M trade of
/// the same era must keep its stored amount, or every normal row would be multiplied by its price.
#[test]
fn an_ordinary_trade_is_never_rewritten() {
    let out = settled_and_quote(&[
        // A named COIN-M trade after the switch: relabeled by the market rule, amount untouched.
        (
            7,
            1_711_700_027,
            "Sell Price",
            "BTC_1226",
            Some("Pump_USD-BTC_1226_x"),
            128_146.0,
            2.05,
        ),
        // A liquidation whose coin is not a contract shape at all: not COIN-M, nothing to correct.
        (7, 1_711_700_027, "LIQUIDATION", "BTC", None, 70_000.0, 1.5),
        // A USD-M core liquidating a DATED contract: same shape, same missing name, same era —
        // and it must be left alone, or a -1.5 loss becomes -105 000.
        (
            42,
            1_711_700_027,
            "LIQUIDATION",
            "BTC_0926",
            None,
            70_000.0,
            1.5,
        ),
    ]);
    assert!(
        (out[0].0 - 2.05).abs() < 1e-12,
        "a named trade is not rewritten"
    );
    assert_eq!(out[0].1, Some(0), "the market rule still relabels it");
    assert!(
        (out[1].0 - 1.5).abs() < 1e-12,
        "a spot liquidation is not rewritten"
    );
    assert_eq!(out[1].1, Some(1), "and it keeps its label");
    assert!(
        (out[2].0 - 1.5).abs() < 1e-12,
        "a USD-M core's dated liquidation is not COIN-M and must not be rewritten"
    );
    assert_eq!(out[2].1, Some(1), "nor relabeled");
}

/// A liquidation without a usable price cannot be restored, and must keep its stored amount rather
/// than collapsing to zero — deleting a loss is worse than reporting it in the wrong unit.
#[test]
fn a_liquidation_without_a_price_keeps_its_amount() {
    let out = settled_and_quote(&[
        (
            7,
            1_711_700_027,
            "LIQUIDATION",
            "ENS_RP",
            None,
            0.0,
            0.000_33,
        ),
        (
            7,
            1_711_700_027,
            "Sell Price",
            "ETH_0927",
            Some("Pump_USD-ETH_0927_x"),
            3_600.0,
            0.5,
        ),
    ]);
    assert!((out[0].0 - 0.000_33).abs() < 1e-12);
    assert_eq!(
        out[0].1,
        Some(1),
        "an uncorrected amount keeps its old label"
    );
}
