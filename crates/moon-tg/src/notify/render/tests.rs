//! Pins for notification HTML. Each test names the edit that would break it.

use chrono_tz::Tz;
use moon_core::telegram::reply::TELEGRAM_MESSAGE_UTF16_LIMIT;

use super::super::trades::ClosedTrade;
use super::*;

fn trade() -> ClosedTrade {
    ClosedTrade {
        core: 1,
        rec_id: 1,
        close_utc: 125,
        coin: "BTC".to_string(),
        core_name: "alpha".to_string(),
        strategy: "grid".to_string(),
        volume_usd: Some(100.0),
        profit_usd: Some(12.5),
        profit_pct: Some(1.25),
        ..ClosedTrade::default()
    }
}

/// Tags this renderer is allowed to emit. `<a>` is legal Telegram HTML and is
/// still rejected: a link in a name must stay text.
const ALLOWED_TAGS: &[&str] = &["<b>", "</b>", "<i>", "</i>", "<code>", "</code>"];

/// `<[^>]+>` over `html`. A cut that lands inside an entity shows up here as
/// a tag the allow-list does not name.
fn html_tags(html: &str) -> Vec<&str> {
    let mut tags = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let Some(end) = rest.find('>') else {
            tags.push(rest);
            break;
        };
        if end == 0 {
            rest = &rest[1..];
            continue;
        }
        tags.push(&rest[..=end]);
        rest = &rest[end + 1..];
    }
    tags
}

fn assert_only_allowed_tags(html: &str) {
    for tag in html_tags(html) {
        assert!(
            ALLOWED_TAGS.contains(&tag),
            "notification HTML emitted {tag}, which Telegram would treat as markup"
        );
    }
}

/// Letting `<`, `&`, `>`, or `"` through would make Telegram parse a coin or
/// a strategy as markup, and the card would fail to send or show the wrong text.
/// A coin is a hashtag, which holds no markup character at all.
#[test]
fn names_are_escaped_before_they_enter_html() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "BTC<&>\"USDT".to_string();
    row.core_name = "core<&>\"".to_string();
    row.strategy = "strat<&>\"".to_string();
    let html = trade_card(&row, false);
    assert!(html.contains("#BTC____USDT "), "{html}");
    assert!(html.starts_with("core&lt;&amp;&gt;&quot;:\n"), "{html}");
    assert!(html.contains("<i>strat&lt;&amp;&gt;&quot;</i>"));
    assert!(!html.contains("<&>"));
    assert_only_allowed_tags(&html);
}

/// The card as the cores' own bot writes it: the core first, the coin as a tag, the profit, the
/// entry and exit price, the strategy. No volume and no duration (LinKvo, 04.10).
#[test]
fn the_card_reads_like_the_cores_own_bot() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "MARSCOIN".to_string();
    row.core_name = "BinF4RMain".to_string();
    row.strategy = "MoonShot: (strategy <MainShotL>)".to_string();
    row.quote = moon_core::db::QuoteCurrency::from_report_ordinal(1);
    row.profit_native = Some(3.36);
    row.profit_pct = Some(0.96);
    row.buy_price = Some(0.10739);
    row.sell_price = Some(0.10844);
    assert_eq!(
        trade_card(&row, false),
        "BinF4RMain:\n\u{1f7e2} #MARSCOIN <b>+3.36 USDT</b> (+0.96%)\n0.10739 \u{2192} 0.10844\n<i>MoonShot \u{00b7} MainShotL</i>"
    );
}

/// Prices keep enough digits to tell an exit from its entry; a row without both prices, or with
/// one the row could not read, prints no price line rather than a half or a zero.
#[test]
fn prices_tell_the_exit_from_the_entry() {
    assert_eq!(
        prices_text(Some(0.014860), Some(0.015238)).as_deref(),
        Some("0.01486 \u{2192} 0.015238")
    );
    assert_eq!(
        prices_text(Some(1.234561), Some(1.234564)).as_deref(),
        Some("1.234561 \u{2192} 1.234564")
    );
    assert_eq!(
        prices_text(Some(65432.37), Some(65440.0)).as_deref(),
        Some("65432.4 \u{2192} 65440")
    );
    assert_eq!(
        prices_text(Some(2.5), Some(2.5)).as_deref(),
        Some("2.5 \u{2192} 2.5")
    );
    assert_eq!(
        prices_text(Some(0.000_000_012_345_6), Some(0.000_000_012_345_9)).as_deref(),
        Some("0.0000000123456 \u{2192} 0.0000000123459")
    );
    assert_eq!(prices_text(None, Some(1.0)), None);
    assert_eq!(prices_text(Some(1.0), Some(f64::NAN)), None);
    assert_eq!(prices_text(Some(0.0), Some(1.0)), None);
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.buy_price = Some(1.0);
    assert_eq!(trade_card(&row, false).lines().count(), 3);
}

/// Cutting a name after escaping, or on a byte index, would either split
/// `&amp;` into a tag or panic on a multibyte scalar, and the chat would get
/// a broken card or none.
#[test]
fn long_strategy_is_cut_to_64_chars_before_escape() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    let mut strategy = "A".repeat(63);
    strategy.push('\u{044f}');
    strategy.push_str(&"B".repeat(236));
    assert_eq!(strategy.chars().count(), 300);
    row.strategy = strategy;
    let html = trade_card(&row, false);
    let kept: String = "A".repeat(63).chars().chain(['\u{044f}']).collect();
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines[2], format!("<i>{kept}</i>"));
    assert!(html.chars().count() < 1000);
    assert!(html.encode_utf16().count() < TELEGRAM_MESSAGE_UTF16_LIMIT);
    assert_only_allowed_tags(&html);
}

/// Printing `0.00` for a missing profit would tell the chat the trade broke
/// even, and a white mark on a real loss would hide the sign.
#[test]
fn unvalued_profit_is_a_word() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.profit_usd = None;
    row.profit_pct = None;
    let html = trade_card(&row, false);
    assert_eq!(html, "alpha:\n\u{26aa} #BTC Unvalued\n<i>grid</i>");
    assert!(!html.contains("0.00"));
    assert!(!html.contains('$'));
    assert_only_allowed_tags(&html);
}

/// Dropping the leading plus, painting a gain red, or marking a profit that rounds to `0.00` as a
/// gain would make the figure and the mark disagree. The outage line keeps its duration.
#[test]
fn sign_follows_the_rounded_figure() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    let second = |row: &ClosedTrade| trade_card(row, false).lines().nth(1).map(str::to_string);
    assert_eq!(
        second(&row).as_deref(),
        Some("\u{1f7e2} #BTC <b>+12.50$</b> (+1.25%)")
    );
    row.profit_usd = Some(-4.0);
    row.profit_pct = Some(-0.5);
    assert_eq!(
        second(&row).as_deref(),
        Some("\u{1f534} #BTC <b>-4.00$</b> (-0.50%)")
    );
    row.profit_usd = Some(0.0);
    row.profit_pct = Some(0.0);
    assert_eq!(
        second(&row).as_deref(),
        Some("\u{26aa} #BTC <b>0.00$</b> (0.00%)")
    );
    assert!(!trade_card(&row, false).contains("+0.00$"));
    row.profit_usd = Some(0.001);
    assert!(second(&row).unwrap().starts_with('\u{26aa}'));
    assert!(trade_card(&row, false).contains("<b>0.00$</b>"));
    assert_eq!(
        back_line("alpha", 3 * 86_400 + 4 * 3_600),
        "\u{1f7e2} alpha \u{2014} connection restored (3d 4h)"
    );
    assert_eq!(
        back_line("alpha", 2 * 3_600 + 5 * 60),
        "\u{1f7e2} alpha \u{2014} connection restored (2h 5m)"
    );
    assert_eq!(
        back_line("alpha", -5),
        "\u{1f7e2} alpha \u{2014} connection restored (0m)"
    );
}

/// A UTC clock on a zoned outage would name the wrong hour. Markup in the
/// core name must stay text on this line too.
#[test]
fn down_line_uses_the_report_zone_clock() {
    let _locale = crate::test_locale::force("en");
    // 2026-10-02 15:04:00 UTC. Moscow is UTC+3 with no DST.
    let since = 1_790_953_440_i64;
    let marked = "core<&>\"";
    assert_eq!(
        down_line(marked, since, Tz::UTC),
        "\u{1f534} core&lt;&amp;&gt;&quot; \u{2014} lost connection since 15:04"
    );
    assert_eq!(
        down_line("alpha", since, Tz::Europe__Moscow),
        "\u{1f534} alpha \u{2014} lost connection since 18:04"
    );
    assert_only_allowed_tags(&down_line(marked, since, Tz::UTC));
}

/// An empty strategy must not render as a blank line. The chat would not be
/// able to tell a manual close from a missing row.
#[test]
fn empty_strategy_uses_the_manual_word() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.strategy.clear();
    let html = trade_card(&row, false);
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines[2], "<i>Manual</i>");
}

/// Cutting the raw channel name at 64 would slice inside `(strategy <` and
/// drop the end of a long name. The cut has to land on the parsed name, and
/// the italic tags have to stay around the whole line.
#[test]
fn long_parsed_name_is_cut_without_breaking_the_strategy_line() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    let mut strategy_name = "N".repeat(63);
    strategy_name.push('\u{044f}');
    strategy_name.push_str(&"B".repeat(20));
    row.strategy = format!("Desk: (strategy <{strategy_name}>)");
    let html = trade_card(&row, false);
    let kept: String = "N".repeat(63).chars().chain(['\u{044f}']).collect();
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2], format!("<i>Desk \u{00b7} {kept}</i>"));
    assert!(!html.contains("strategy"));
    let source = "S".repeat(80);
    row.strategy = format!("{source}: (strategy <GRID>)");
    assert!(trade_card(&row, false).ends_with(&format!("<i>{} \u{00b7} GRID</i>", "S".repeat(64))));
    assert_only_allowed_tags(&html);
}

/// A channel name that is not `Source: (strategy <NAME>)` must stay the stored
/// text. Parsing a lookalike would rename a strategy the core did not write
/// in that shape.
#[test]
fn strategy_channel_parses_source_and_name_or_stays_whole() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.strategy = "Desk: (strategy <GRID_A / SAMPLE>)".to_string();
    assert!(trade_card(&row, false).ends_with("<i>Desk \u{00b7} GRID_A / SAMPLE</i>"));
    row.strategy = "(strategy <GRID_A>)".to_string();
    assert!(trade_card(&row, false).ends_with("<i>GRID_A</i>"));
    row.strategy = "  Desk:  (strategy <GRID (long)>)".to_string();
    assert!(trade_card(&row, false).ends_with("<i>Desk \u{00b7} GRID (long)</i>"));
    row.strategy = "plain grid".to_string();
    assert!(trade_card(&row, false).ends_with("<i>plain grid</i>"));
    row.strategy = "Desk: (strategy <GRID>) extra".to_string();
    assert!(trade_card(&row, false).ends_with("<i>Desk: (strategy &lt;GRID&gt;) extra</i>"));
    row.strategy = "Desk: (strategy <A&B>)".to_string();
    assert!(trade_card(&row, false).ends_with("<i>Desk \u{00b7} A&amp;B</i>"));
    assert_only_allowed_tags(&trade_card(&row, false));
}

/// A missing percent must keep the dollars and say the percent is unvalued.
/// Dropping the parentheses would make the word look like a second profit.
#[test]
fn missing_percent_keeps_the_unvalued_word_beside_the_dollars() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.profit_pct = None;
    assert!(trade_card(&row, false).contains("<b>+12.50$</b> (Unvalued)"));
}

/// A card in the trade's own currency: waiting for a valuation would delay it, converting would
/// print a figure the trade never had. Outside a USD stablecoin the dollars follow once known; a
/// stablecoin's amount already is them. An unchecked card says so on its own line.
#[test]
fn a_card_prints_the_trade_in_its_own_currency() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "ETHBTC".to_string();
    row.quote = moon_core::db::QuoteCurrency::from_report_ordinal(0);
    row.profit_native = Some(0.00012);
    row.profit_usd = None;
    let second = |row: &ClosedTrade| trade_card(row, false).lines().nth(1).map(str::to_string);
    assert_eq!(
        second(&row).as_deref(),
        Some("\u{1f7e2} #ETHBTC <b>+0.00012 BTC</b> (+1.25%)")
    );
    row.profit_usd = Some(7.5);
    assert_eq!(
        second(&row).as_deref(),
        Some("\u{1f7e2} #ETHBTC <b>+0.00012 BTC</b> \u{2248} +7.50$ (+1.25%)")
    );
    row.quote = moon_core::db::QuoteCurrency::from_report_ordinal(8);
    row.profit_native = Some(-3.3);
    row.profit_usd = Some(-3.29);
    row.profit_pct = Some(-0.5);
    let card = trade_card(&row, true);
    assert_eq!(
        card.lines().nth(1),
        Some("\u{1f534} #ETHBTC <b>-3.3 USDC</b> (-0.50%)")
    );
    assert_eq!(
        card.lines().nth(3),
        Some("Thresholds not checked: no dollar value")
    );
    assert_only_allowed_tags(&card);
    row.profit_native = Some(0.0);
    let flat = second(&row).unwrap();
    assert!(flat.starts_with('\u{26aa}'), "{flat}");
    assert!(
        flat.contains("<b>0 USDC</b>"),
        "a flat trade claims no gain: {flat}"
    );
}
