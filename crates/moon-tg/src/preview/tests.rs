//! Preview regressions guard the link to the real renderers.
use super::*;
use crate::notify::render::trade_card;

/// Independent HTML oracle decodes only the card renderer's documented b/i tag subset.
fn plain_card_html(html: &str) -> String {
    html.replace("<b>", "")
        .replace("</b>", "")
        .replace("<i>", "")
        .replace("</i>", "")
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

/// Read raw semantic text without encoding, keeping the renderer's explicit line breaks.
fn plain_lines(lines: &[PreviewLine]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replacing the preview with a separate formatter would stop reflecting bot output.
#[test]
fn preview_card_equals_real_renderer() {
    let _locale = crate::test_locale::force("en");
    for layout in [
        CardLayout::default(),
        CardLayout::moonbot_preset(),
        CardLayout {
            coin_hashtag: false,
            core_hashtag: false,
            ..Default::default()
        },
    ] {
        assert_eq!(
            plain_lines(&preview_card(&layout)),
            plain_card_html(&trade_card(&sample_trade(), false, &layout))
        );
    }
    let mut trade = sample_trade();
    trade.coin = "SOL<&\"".into();
    trade.core_name = "Core<&>".into();
    trade.strategy = "Strategy<&\"".into();
    let layout = CardLayout {
        coin_hashtag: false,
        core_hashtag: false,
        ..Default::default()
    };
    let lines = card_lines(&trade, true, &layout);
    assert_eq!(
        plain_lines(&lines),
        plain_card_html(&trade_card(&trade, true, &layout))
    );
    assert!(plain_lines(&lines).contains("SOL<&\""));
}

/// A preview that ignores column order would misrepresent the report the user saves.
#[test]
fn preview_report_respects_column_order() {
    let _locale = crate::test_locale::force("en");
    let layout = ReportLayout {
        columns: vec![
            moon_core::config::ReportColumn::Trades,
            moon_core::config::ReportColumn::Profit,
        ],
        ..Default::default()
    };
    let table = preview_report(&layout);
    assert_eq!(&table.header[1..], &["Trades", "Profit / loss"]);
    assert_eq!(
        table.rows[1]
            .cells
            .iter()
            .map(|span| span.text.as_str())
            .collect::<Vec<_>>(),
        vec!["Binance-2", "1", "+8.40$"]
    );
    assert_eq!(table.rows[2].cells[0].text, "Spot-7");
    assert_eq!(table.rows[0].kind, PreviewRowKind::Group);
    assert!(table.rows[0].band);
    assert_eq!(table.rows.last().unwrap().kind, PreviewRowKind::Total);
}

/// Losing typed row metadata would make total and group choices visually indistinguishable.
#[test]
fn preview_report_preserves_top_total_spacer_and_bold_left_group() {
    let layout = ReportLayout {
        total: moon_core::config::TotalPlace::Top,
        separation: moon_core::config::TotalSeparation::GapBand,
        group_row: moon_core::config::GroupRowStyle::BoldLeft,
        ..Default::default()
    };
    let table = preview_report(&layout);
    assert_eq!(
        table.rows.iter().map(|row| row.kind).collect::<Vec<_>>(),
        vec![
            PreviewRowKind::Total,
            PreviewRowKind::Spacer,
            PreviewRowKind::Group,
            PreviewRowKind::Data,
            PreviewRowKind::Data
        ]
    );
    assert!(table.rows[0].band);
    assert!(table.rows[0].cells.iter().all(|span| span.bold));
    assert!(!table.rows[2].band);
    assert!(!table.rows[2].first_right);
    assert!(table.rows[2].cells.iter().all(|span| span.bold));
}

/// Preview tones must preserve the renderer's rounded sign and independent hashtag choices.
#[test]
fn preview_card_spans_carry_link_gain_loss_and_strategy_styles() {
    let layout = CardLayout::default();
    let lines = preview_card(&layout);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.text == "#SOLUSDT" && span.bold && span.tone == Tone::Link)
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.text == "+12.40$" && span.bold && span.tone == Tone::Gain)
    );
    assert!(lines[0].spans.iter().any(|span| span.text == "250$"));
    assert!(
        lines
            .last()
            .unwrap()
            .spans
            .iter()
            .any(|span| span.text == "Dropdown 3%" && span.italic)
    );
    let mut trade = sample_trade();
    trade.profit_native = Some(-0.42);
    let lines = card_lines(&trade, false, &layout);
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.text == "-0.42$" && span.tone == Tone::Loss)
    );
}
