//! Pins for notification HTML. Each test names the edit that would break it.

use chrono_tz::Tz;
use moon_core::telegram::reply::TELEGRAM_MESSAGE_UTF16_LIMIT;

use super::super::trades::ClosedTrade;
use super::*;

/// A synthetic trade matching the pre-#873 card pins.
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
        open_utc: 0,
        ..ClosedTrade::default()
    }
}

/// An optional-only layout must fall back instead of submitting an empty Telegram body.
#[test]
fn unavailable_prices_fall_back_to_default_card() {
    let _locale = crate::test_locale::force("en");
    let layout = CardLayout {
        lines: vec![vec![CardField::Prices]],
        ..Default::default()
    };
    for unchecked in [false, true] {
        assert_eq!(
            trade_card(&trade(), unchecked, &layout),
            trade_card(&trade(), unchecked, &CardLayout::default())
        );
    }
}

/// Falling back from unavailable fields must retain both saved hashtag switches.
#[test]
fn empty_card_fallback_preserves_plain_names() {
    let _locale = crate::test_locale::force("en");
    let layout = CardLayout {
        lines: vec![vec![CardField::Prices]],
        coin_hashtag: false,
        core_hashtag: false,
    };
    let html = trade_card(&trade(), false, &layout);
    assert!(html.contains("<b>BTC</b>"));
    assert!(html.contains("\nalpha\n"));
    assert!(!html.contains('#'));
}

/// Tags this renderer is allowed to emit. `<a>` is legal Telegram HTML and is
/// still rejected: a link in a name must stay text.
const ALLOWED_TAGS: &[&str] = &["<b>", "</b>", "<i>", "</i>", "<code>", "</code>"];

/// Losing the preset order or price precision would change the MoonBot-style card.
#[test]
fn moonbot_preset_names_the_core_first_and_prints_prices() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.buy_price = Some(0.10739);
    row.sell_price = Some(0.10844);
    assert_eq!(
        trade_card(&row, false, &CardLayout::moonbot_preset()),
        "Name: #alpha\n\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%)\n0.10739 \u{2192} 0.10844\n<i>grid</i>"
    );
    row.sell_price = None;
    assert_eq!(
        trade_card(&row, false, &CardLayout::moonbot_preset()),
        "Name: #alpha\n\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%)\n<i>grid</i>"
    );
}

/// Coupling the switches or bypassing escaping would rename or corrupt the other name.
#[test]
fn hashtag_switches_are_independent_and_plain_names_are_escaped() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "BTC<&>".into();
    row.core_name = "alpha<&>".into();
    let mut layout = CardLayout {
        coin_hashtag: false,
        ..CardLayout::default()
    };
    assert_eq!(
        trade_card(&row, false, &layout),
        "\u{1f7e2} <b>BTC&lt;&amp;&gt;</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 100$ \u{00b7} 2m\n#alpha___\n<i>grid</i>"
    );
    layout.coin_hashtag = true;
    layout.core_hashtag = false;
    assert_eq!(
        trade_card(&row, false, &layout),
        "\u{1f7e2} <b>#BTC___</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 100$ \u{00b7} 2m\nalpha&lt;&amp;&gt;\n<i>grid</i>"
    );
}

/// Unknown fields, duplicates and blank lines must not obscure the known saved card.
#[test]
fn unfamiliar_fields_and_empty_lines_are_skipped_and_duplicates_removed() {
    let _locale = crate::test_locale::force("en");
    let layout = CardLayout {
        lines: vec![
            vec![],
            vec![
                CardField::Mark,
                CardField::Other("sparkline".into()),
                CardField::Coin,
            ],
            vec![CardField::Coin, CardField::Other("new".into())],
            vec![CardField::Strategy],
        ],
        ..CardLayout::default()
    };
    assert_eq!(
        trade_card(&trade(), false, &layout.sanitized()),
        "\u{1f7e2} <b>#BTC</b>\n<i>grid</i>"
    );
}

/// A trailing mark must remain visible rather than acquiring a dangling separator.
#[test]
fn a_mark_last_on_a_line_stands_alone() {
    let _locale = crate::test_locale::force("en");
    let layout = CardLayout {
        lines: vec![
            vec![CardField::Coin, CardField::Mark],
            vec![CardField::Core],
        ],
        ..CardLayout::default()
    };
    assert_eq!(
        trade_card(&trade(), false, &layout),
        "<b>#BTC</b> \u{00b7} \u{1f7e2}\n#alpha"
    );
}

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
#[test]
fn names_are_escaped_before_they_enter_html() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "123<&>\"".to_string();
    row.core_name = "<&>\"".to_string();
    row.strategy = "strat<&>\"".to_string();
    let html = trade_card(&row, false, &CardLayout::default());
    assert!(html.contains("<b>123&lt;&amp;&gt;&quot;</b>"));
    assert_eq!(html.lines().nth(1), Some("&lt;&amp;&gt;&quot;"));
    assert!(html.contains("<i>strat&lt;&amp;&gt;&quot;</i>"));
    assert!(!html.contains("<&>"));
    assert_only_allowed_tags(&html);
}

/// Dropping either hashtag or using a different mapping on outage notices would split the
/// coin/core history; strategy punctuation must retain its existing escaped appearance.
#[test]
fn cards_and_outages_share_coin_and_core_hashtags() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.coin = "SAMPLE714".into();
    row.core_name = "Desk F-2.v1/test".into();
    row.strategy = "grid<&>".into();
    let card = trade_card(&row, false, &CardLayout::default());
    assert_eq!(
        card,
        "\u{1f7e2} <b>#SAMPLE714</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 100$ \u{00b7} 2m\n#Desk_F_2_v1_test\n<i>grid&lt;&amp;&gt;</i>"
    );
    assert!(down_line(&row.core_name, 0, Tz::UTC).starts_with("\u{1f534} #Desk_F_2_v1_test "));
    assert!(back_line(&row.core_name, 60).starts_with("\u{1f7e2} #Desk_F_2_v1_test "));
    assert_only_allowed_tags(&card);

    row.coin = "12345".into();
    row.core_name = "<&>\"".into();
    let fallback = trade_card(&row, false, &CardLayout::default());
    assert!(fallback.starts_with("\u{1f7e2} <b>12345</b> "));
    assert_eq!(fallback.lines().nth(1), Some("&lt;&amp;&gt;&quot;"));
    assert!(back_line(&row.core_name, 60).starts_with("\u{1f7e2} &lt;&amp;&gt;&quot; "));
    assert_only_allowed_tags(&fallback);
}

/// Putting the core or prices first would hide coin and profit in the push preview. Chart
/// metadata must not add lines to the historical three-line card or its unchecked variant.
#[test]
fn the_complete_card_keeps_the_pre_873_layout() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.buy_price = Some(0.10739);
    row.sell_price = Some(0.10844);
    row.short = true;
    row.report_uid = Some(42);
    assert_eq!(
        trade_card(&row, false, &CardLayout::default()),
        "\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 100$ \u{00b7} 2m\n#alpha\n<i>grid</i>"
    );
    assert_eq!(
        trade_card(&row, true, &CardLayout::default()),
        "\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 100$ \u{00b7} 2m\n#alpha\n<i>grid</i>\nThresholds not checked: no dollar value"
    );
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
    let html = trade_card(&row, false, &CardLayout::default());
    let kept: String = "A".repeat(63).chars().chain(['\u{044f}']).collect();
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines[2], format!("<i>{kept}</i>"));
    assert!(html.chars().count() < 1000);
    assert!(html.encode_utf16().count() < TELEGRAM_MESSAGE_UTF16_LIMIT);
    assert_only_allowed_tags(&html);
}

/// Printing `0.00` for a missing profit would tell the chat the trade broke
/// even, and a white mark on a real loss would hide the sign. The volume
/// segment must disappear when the row has no volume, or a blank notional
/// reads as a zero fill.
#[test]
fn unvalued_profit_is_a_word_and_missing_volume_is_omitted() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.profit_usd = None;
    row.profit_pct = None;
    row.volume_usd = None;
    let html = trade_card(&row, false, &CardLayout::default());
    assert_eq!(
        html,
        "\
\u{26aa} <b>#BTC</b> \u{00b7} Unvalued \u{00b7} 2m
#alpha
<i>grid</i>"
    );
    assert!(!html.contains("0.00"));
    assert!(!html.contains('$'));
    assert_only_allowed_tags(&html);
}

/// Dropping the hour or the day, or keeping seconds, would make a two-day
/// hold and a two-minute hold look like the same kind of number. Dropping the
/// leading plus, painting a gain red, or marking a profit that rounds to
/// `0.00` as a gain would make the figure and the mark disagree.
#[test]
fn duration_and_sign_follow_the_rounded_figure() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.volume_usd = None;
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some("\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 2m")
    );
    row.profit_usd = Some(-4.0);
    row.profit_pct = Some(-0.5);
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some("\u{1f534} <b>#BTC</b> \u{00b7} <b>-4.00$</b> (-0.50%) \u{00b7} 2m")
    );
    row.profit_usd = Some(0.0);
    row.profit_pct = Some(0.0);
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some("\u{26aa} <b>#BTC</b> \u{00b7} <b>0.00$</b> (0.00%) \u{00b7} 2m")
    );
    assert!(!trade_card(&row, false, &CardLayout::default()).contains("+0.00$"));
    row.profit_usd = Some(0.001);
    row.profit_pct = Some(0.0);
    assert!(trade_card(&row, false, &CardLayout::default()).starts_with('\u{26aa}'));
    assert!(trade_card(&row, false, &CardLayout::default()).contains("<b>0.00$</b>"));
    row.profit_usd = Some(12.5);
    row.profit_pct = Some(1.25);
    row.open_utc = 0;
    row.close_utc = 3 * 86_400 + 4 * 3_600;
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 3d 4h"));
    row.close_utc = 2 * 3_600 + 5 * 60;
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 2h 5m"));
    row.close_utc = 90;
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 1m"));
    row.close_utc = 45;
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 0m"));
    assert!(!trade_card(&row, false, &CardLayout::default()).contains("Duration"));
    assert_eq!(
        back_line("alpha", 3 * 86_400 + 4 * 3_600),
        "\u{1f7e2} #alpha \u{2014} connection restored (3d 4h)"
    );
    assert_eq!(
        back_line("alpha", -5),
        "\u{1f7e2} #alpha \u{2014} connection restored (0m)"
    );
}

/// A UTC clock on a zoned outage would name the wrong hour. Markup in the
/// core name must stay text on this line too.
#[test]
fn down_line_uses_the_report_zone_clock() {
    let _locale = crate::test_locale::force("en");
    // 2026-10-02 15:04:00 UTC. Moscow is UTC+3 with no DST.
    let since = 1_790_953_440_i64;
    let marked = "<&>\"";
    assert_eq!(
        down_line(marked, since, Tz::UTC),
        "\u{1f534} &lt;&amp;&gt;&quot; \u{2014} lost connection since 15:04"
    );
    assert_eq!(
        down_line("alpha", since, Tz::Europe__Moscow),
        "\u{1f534} #alpha \u{2014} lost connection since 18:04"
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
    let html = trade_card(&row, false, &CardLayout::default());
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
    let html = trade_card(&row, false, &CardLayout::default());
    let kept: String = "N".repeat(63).chars().chain(['\u{044f}']).collect();
    let lines: Vec<&str> = html.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2], format!("<i>Desk \u{00b7} {kept}</i>"));
    assert!(!html.contains("strategy"));
    let source = "S".repeat(80);
    row.strategy = format!("{source}: (strategy <GRID>)");
    assert!(
        trade_card(&row, false, &CardLayout::default())
            .ends_with(&format!("<i>{} \u{00b7} GRID</i>", "S".repeat(64)))
    );
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
    assert!(
        trade_card(&row, false, &CardLayout::default())
            .ends_with("<i>Desk \u{00b7} GRID_A / SAMPLE</i>")
    );
    row.strategy = "(strategy <GRID_A>)".to_string();
    assert!(trade_card(&row, false, &CardLayout::default()).ends_with("<i>GRID_A</i>"));
    row.strategy = "  Desk:  (strategy <GRID (long)>)".to_string();
    assert!(
        trade_card(&row, false, &CardLayout::default())
            .ends_with("<i>Desk \u{00b7} GRID (long)</i>")
    );
    row.strategy = "plain grid".to_string();
    assert!(trade_card(&row, false, &CardLayout::default()).ends_with("<i>plain grid</i>"));
    row.strategy = "Desk: (strategy <GRID>) extra".to_string();
    assert!(
        trade_card(&row, false, &CardLayout::default())
            .ends_with("<i>Desk: (strategy &lt;GRID&gt;) extra</i>")
    );
    row.strategy = "Desk: (strategy <A&B>)".to_string();
    assert!(
        trade_card(&row, false, &CardLayout::default()).ends_with("<i>Desk \u{00b7} A&amp;B</i>")
    );
    assert_only_allowed_tags(&trade_card(&row, false, &CardLayout::default()));
}

/// Keeping cents on the entry volume, or gluing the thousands together, would
/// make `5992.4` read as a different notional than the whole-dollar figure.
#[test]
fn entry_volume_is_grouped_whole_dollars() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.volume_usd = Some(5992.4);
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 5 992$ \u{00b7} "));
    row.volume_usd = Some(5992.5);
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 5 993$ \u{00b7} "));
    row.volume_usd = Some(10_000.0);
    assert!(
        trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 10 000$ \u{00b7} ")
    );
    row.volume_usd = Some(0.0);
    assert!(trade_card(&row, false, &CardLayout::default()).contains(" \u{00b7} 0$ \u{00b7} "));
    row.volume_usd = Some(f64::NAN);
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some("\u{1f7e2} <b>#BTC</b> \u{00b7} <b>+12.50$</b> (+1.25%) \u{00b7} 2m")
    );
}

/// A missing percent must keep the dollars and say the percent is unvalued.
/// Dropping the parentheses would make the word look like a second profit.
#[test]
fn missing_percent_keeps_the_unvalued_word_beside_the_dollars() {
    let _locale = crate::test_locale::force("en");
    let mut row = trade();
    row.profit_pct = None;
    row.volume_usd = None;
    assert!(trade_card(&row, false, &CardLayout::default()).contains("<b>+12.50$</b> (Unvalued)"));
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
    row.volume_native = Some(0.0105);
    row.profit_usd = None;
    row.volume_usd = None;
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some(
            "\u{1f7e2} <b>#ETHBTC</b> \u{00b7} <b>+0.00012 BTC</b> (+1.25%) \u{00b7} 0.0105 BTC \u{00b7} 2m"
        )
    );
    row.profit_usd = Some(7.5);
    assert_eq!(
        trade_card(&row, false, &CardLayout::default())
            .lines()
            .next(),
        Some(
            "\u{1f7e2} <b>#ETHBTC</b> \u{00b7} <b>+0.00012 BTC</b> \u{2248} +7.50$ (+1.25%) \u{00b7} 0.0105 BTC \u{00b7} 2m"
        )
    );
    row.quote = moon_core::db::QuoteCurrency::from_report_ordinal(8);
    row.profit_native = Some(-3.3);
    row.profit_usd = Some(-3.29);
    row.volume_native = Some(1234.4);
    row.profit_pct = Some(-0.5);
    let card = trade_card(&row, true, &CardLayout::default());
    assert_eq!(
        card.lines().next(),
        Some(
            "\u{1f534} <b>#ETHBTC</b> \u{00b7} <b>-3.3 USDC</b> (-0.50%) \u{00b7} 1 234 USDC \u{00b7} 2m"
        )
    );
    assert_eq!(
        card.lines().nth(3),
        Some("Thresholds not checked: no dollar value")
    );
    assert_only_allowed_tags(&card);
    row.profit_native = Some(0.0);
    row.volume_native = Some(0.4);
    let flat = trade_card(&row, false, &CardLayout::default());
    assert!(flat.starts_with('\u{26aa}'), "{flat}");
    assert!(
        flat.contains("<b>0 USDC</b>"),
        "a flat trade claims no gain: {flat}"
    );
    assert!(
        flat.contains(" 0.4 USDC "),
        "a notional under one keeps its cents: {flat}"
    );
}
