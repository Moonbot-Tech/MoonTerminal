//! Regression checks for forward-compatible layouts and drag editing invariants.
use super::*;

/// Independent report capture must recover drawable columns without discarding future ids.
#[test]
fn report_sanitized_and_drawable_columns_preserve_saved_order() {
    let layout = ReportLayout {
        columns: vec![
            ReportColumn::Other("fees".into()),
            ReportColumn::Volume,
            ReportColumn::Volume,
            ReportColumn::Trades,
        ],
        ..Default::default()
    }
    .sanitized();
    assert_eq!(
        layout.drawable_columns().cloned().collect::<Vec<_>>(),
        vec![ReportColumn::Volume, ReportColumn::Trades]
    );
    assert_eq!(layout.columns.len(), 3);
    let future = ReportLayout {
        columns: vec![ReportColumn::Other("fees".into())],
        ..Default::default()
    }
    .sanitized();
    assert_eq!(
        future.columns,
        vec![
            ReportColumn::Profit,
            ReportColumn::Trades,
            ReportColumn::Other("fees".into())
        ]
    );
}

/// Card capture must normalize independently while retaining the saved hashtag choices.
#[test]
fn card_sanitized_removes_repeats_and_recovers_unknown_only_lines() {
    let mut card = CardLayout {
        lines: vec![vec![CardField::Coin, CardField::Coin], vec![]],
        coin_hashtag: false,
        ..Default::default()
    };
    assert_eq!(card.sanitized().lines, vec![vec![CardField::Coin]]);
    card.lines = vec![vec![CardField::Other("future".into())]];
    let normalized = card.sanitized();
    assert_eq!(normalized.lines, CardLayout::default().lines);
    assert!(!normalized.coin_hashtag);
}

/// Dropping unfamiliar ids would destroy a newer terminal's layout on save.
#[test]
fn newer_layout_ids_round_trip() {
    let value = serde_json::json!({"future": 1, "card": {"lines": [["coin", "sparkline"]], "coin_hashtag": false}, "report": {"columns": ["trades", "fees"], "total": "top", "separation": "gap_band", "group_row": "bold_left"}});
    let layout: MessageLayout = serde_json::from_value(value).unwrap();
    assert_eq!(
        layout.card.lines,
        vec![vec![CardField::Coin, CardField::Other("sparkline".into())]]
    );
    assert!(!layout.card.coin_hashtag);
    assert!(layout.card.core_hashtag);
    assert_eq!(layout.report.total, TotalPlace::Top);
    assert_eq!(layout.report.separation, TotalSeparation::GapBand);
    assert_eq!(layout.report.group_row, GroupRowStyle::BoldLeft);
    let saved = serde_json::to_value(&layout).unwrap();
    assert_eq!(
        saved["card"]["lines"],
        serde_json::json!([["coin", "sparkline"]])
    );
    assert_eq!(
        saved["report"]["columns"],
        serde_json::json!(["trades", "fees"])
    );
}

/// Rejecting unknown closed ids or omitting defaults would prevent old settings loading.
#[test]
fn absent_layout_and_unknown_separation_use_defaults() {
    assert_eq!(
        serde_json::from_str::<MessageLayout>("{}").unwrap(),
        MessageLayout::default()
    );
    for id in ["future", "line", "gap_line"] {
        assert_eq!(
            serde_json::from_value::<TotalSeparation>(serde_json::json!(id)).unwrap(),
            TotalSeparation::Band
        );
    }
}

/// Repeated fields must not render twice, and unknown-only reports need drawable columns.
#[test]
fn sanitizing_keeps_first_fields_and_restores_drawable_columns() {
    let mut layout = MessageLayout::default();
    layout.card.lines = vec![
        vec![CardField::Coin, CardField::Coin],
        vec![],
        vec![CardField::Coin, CardField::Profit],
    ];
    layout.report.columns = vec![
        ReportColumn::Other("fees".into()),
        ReportColumn::Other("fees".into()),
    ];
    let clean = layout.sanitized();
    assert_eq!(
        clean.card.lines,
        vec![vec![CardField::Coin], vec![CardField::Profit]]
    );
    assert_eq!(
        clean.report.columns,
        vec![
            ReportColumn::Profit,
            ReportColumn::Trades,
            ReportColumn::Other("fees".into())
        ]
    );
    layout.report.columns = vec![
        ReportColumn::Trades,
        ReportColumn::Profit,
        ReportColumn::Trades,
    ];
    assert_eq!(
        layout.sanitized().report.columns,
        vec![ReportColumn::Trades, ReportColumn::Profit]
    );
    assert_eq!(
        layout.card.lines.len(),
        3,
        "sanitizing must not mutate stored preferences"
    );
}

/// Dragging onto the new-line target restores tray fields and removes emptied source lines.
#[test]
fn moving_fields_uses_original_line_targets_and_cleans_empty_lines() {
    let mut card = CardLayout::moonbot_preset();
    assert_eq!(card.tray(), vec![CardField::Volume, CardField::Duration]);
    card.move_field(&CardField::Volume, card.lines.len(), usize::MAX);
    assert_eq!(card.lines.last(), Some(&vec![CardField::Volume]));
    card.move_field(&CardField::Core, 1, 1);
    assert_eq!(
        card.lines[0],
        vec![
            CardField::Mark,
            CardField::Core,
            CardField::Coin,
            CardField::Profit
        ]
    );
    assert_eq!(card.lines.len(), 4);
    assert!(card.hide(&CardField::Prices));
    assert_eq!(card.tray(), vec![CardField::Duration, CardField::Prices]);
    card.move_line(2, 0);
    assert_eq!(card.lines[0], vec![CardField::Volume]);
    let mut preset = CardLayout::coin_first();
    preset.coin_hashtag = false;
    assert!(preset.is_preset(&CardLayout::default()));
}

/// Hiding the last known column must not leave an undrawable report, even with future ids.
#[test]
fn report_edits_preserve_one_known_column() {
    let mut report = ReportLayout::default();
    assert!(report.hide_column(&ReportColumn::Profit));
    report.restore_column(&ReportColumn::Other("fees".into()));
    assert!(!report.hide_column(&ReportColumn::Trades));
    assert_eq!(
        report.hidden_columns(),
        vec![
            ReportColumn::Profit,
            ReportColumn::Average,
            ReportColumn::Volume
        ]
    );
    report.restore_column(&ReportColumn::Profit);
    report.restore_column(&ReportColumn::Profit);
    report.move_column(&ReportColumn::Profit, 0);
    assert_eq!(
        report.columns,
        vec![
            ReportColumn::Profit,
            ReportColumn::Trades,
            ReportColumn::Other("fees".into())
        ]
    );
    assert!(report.hide_column(&ReportColumn::Trades));
    assert!(!report.hide_column(&ReportColumn::Profit));
}

/// Empty or future-only cards must not suppress all drawable fields after loading.
#[test]
fn sanitizing_restores_a_card_with_no_known_fields() {
    for lines in [
        vec![],
        vec![vec![]],
        vec![vec![CardField::Other("future".into())]],
    ] {
        let mut layout = MessageLayout::default();
        layout.card.lines = lines;
        layout.card.coin_hashtag = false;
        let clean = layout.sanitized();
        assert_eq!(clean.card.lines, default_lines());
        assert!(!clean.card.coin_hashtag);
    }
}

/// Hiding the final known field must fail even when a future field remains.
#[test]
fn hiding_preserves_the_last_known_card_field() {
    let mut card = CardLayout {
        lines: vec![vec![
            CardField::Coin,
            CardField::Profit,
            CardField::Other("future".into()),
        ]],
        ..Default::default()
    };
    assert!(card.hide(&CardField::Profit));
    assert!(!card.hide(&CardField::Coin));
    assert!(!card.hide(&CardField::Core));
    assert_eq!(
        card.lines,
        vec![vec![CardField::Coin, CardField::Other("future".into())]]
    );
    assert!(card.hide(&CardField::Other("future".into())));
    assert!(!card.hide(&CardField::Coin));
}

/// Same-line right drops use the pointed original boundary instead of overshooting it.
#[test]
fn moving_right_uses_the_original_field_index() {
    for (index, expected) in [
        (2, vec![CardField::Coin, CardField::Mark, CardField::Profit]),
        (3, vec![CardField::Coin, CardField::Profit, CardField::Mark]),
    ] {
        let mut card = CardLayout {
            lines: vec![vec![CardField::Mark, CardField::Coin, CardField::Profit]],
            ..Default::default()
        };
        card.move_field(&CardField::Mark, 0, index);
        assert_eq!(card.lines, vec![expected]);
    }
}
