//! Pure action tests guard the operations shared by drag and keyboard handlers.
use super::{
    LayoutAction, apply_action, column_drop_index, dirty, drop_target_index, segment_index,
};
use moon_core::config::{CardField, CardLayout, MessageLayout, ReportColumn};

/// Raw duplicates and future-only columns must show the same drawable fallback as Telegram.
#[test]
fn editor_normalizes_saved_layout_before_first_edit() {
    let mut draft = MessageLayout::default();
    draft.card.lines = vec![vec![CardField::Coin, CardField::Coin]];
    draft.report.columns = vec![ReportColumn::Other("fees".into())];
    let view = super::editor_view(&draft);
    assert_eq!(view.card.lines, vec![vec![CardField::Coin]]);
    assert_eq!(
        view.report.drawable_columns().cloned().collect::<Vec<_>>(),
        vec![ReportColumn::Profit, ReportColumn::Trades]
    );
    assert_eq!(
        view.report.hidden_columns(),
        vec![
            ReportColumn::Average,
            ReportColumn::Volume,
            ReportColumn::Native
        ]
    );
    apply_action(&mut draft, LayoutAction::CoreHashtag(false));
    assert_eq!(draft.card.lines, view.card.lines);
    assert_eq!(draft.report.columns, view.report.columns);
    assert!(
        draft
            .report
            .columns
            .contains(&ReportColumn::Other("fees".into()))
    );
}

/// Arrows must cross invisible future columns and stop at the visible edges.
#[test]
fn column_arrows_use_visible_neighbours() {
    let mut draft = MessageLayout::default();
    draft.report.columns = vec![
        ReportColumn::Profit,
        ReportColumn::Other("x".into()),
        ReportColumn::Trades,
    ];
    assert_eq!(
        super::neighbour_target(&draft.report.columns, &ReportColumn::Profit, -1),
        None
    );
    assert_eq!(
        super::neighbour_target(&draft.report.columns, &ReportColumn::Trades, 1),
        None
    );
    let target = super::neighbour_target(&draft.report.columns, &ReportColumn::Profit, 1).unwrap();
    apply_action(
        &mut draft,
        LayoutAction::Column(ReportColumn::Profit, target),
    );
    assert_eq!(
        draft.report.drawable_columns().cloned().collect::<Vec<_>>(),
        vec![ReportColumn::Trades, ReportColumn::Profit]
    );
}

/// Tray restoration must append to the last line instead of creating an extra one.
#[test]
fn tray_restore_appends_to_last_line_or_creates_first() {
    let mut draft = MessageLayout::default();
    let line = super::restore_target(&draft.card);
    apply_action(
        &mut draft,
        LayoutAction::Field(CardField::Prices, line, usize::MAX),
    );
    assert_eq!(draft.card.lines.len(), 3);
    assert_eq!(
        draft.card.lines[2],
        vec![CardField::Strategy, CardField::Prices]
    );
    let empty = CardLayout {
        lines: vec![],
        ..Default::default()
    };
    let mut empty = empty;
    empty.move_field(
        &CardField::Prices,
        super::restore_target(&empty),
        usize::MAX,
    );
    assert_eq!(empty.lines, vec![vec![CardField::Prices]]);
}

/// Saved status must compare the entire layout with the read-back, including hashtag edits.
#[test]
fn dirty_tracks_edits_reset_and_missing_readback() {
    let base = MessageLayout::default();
    let mut draft = base.clone();
    assert!(!dirty(&draft, Some(&base)));
    apply_action(&mut draft, LayoutAction::CoinHashtag(false));
    assert!(dirty(&draft, Some(&base)));
    apply_action(&mut draft, LayoutAction::Reset);
    assert!(!dirty(&draft, Some(&base)));
    assert!(dirty(&draft, None));
    let mut custom_base = base.clone();
    custom_base.card = CardLayout::moonbot_preset();
    assert!(dirty(&draft, Some(&custom_base)));
}
/// A custom order must not light a preset or erase hashtag preferences when choosing one.
#[test]
fn presets_compare_lines_and_preserve_hashtags() {
    let mut draft = MessageLayout::default();
    assert_eq!(segment_index(&draft.card), Some(0));
    draft.card.coin_hashtag = false;
    apply_action(&mut draft, LayoutAction::Preset(true));
    assert_eq!(segment_index(&draft.card), Some(1));
    assert!(!draft.card.coin_hashtag);
    apply_action(&mut draft, LayoutAction::Field(CardField::Coin, 0, 0));
    assert_eq!(segment_index(&draft.card), None);
}
/// Appending at the new row and dropping before an existing field must honor original indices.
#[test]
fn card_drop_creates_new_line_and_inserts_before_target() {
    let mut draft = MessageLayout::default();
    let line = draft.card.lines.len();
    let index = drop_target_index(&draft.card, line, None);
    apply_action(
        &mut draft,
        LayoutAction::Field(CardField::Prices, line, index),
    );
    assert_eq!(draft.card.lines.last(), Some(&vec![CardField::Prices]));
    let index = drop_target_index(&draft.card, 0, Some(&CardField::Profit));
    apply_action(&mut draft, LayoutAction::Field(CardField::Coin, 0, index));
    assert_eq!(
        &draft.card.lines[0][..3],
        &[CardField::Mark, CardField::Coin, CardField::Profit]
    );
    assert_eq!(segment_index(&CardLayout::moonbot_preset()), Some(1));
}
/// Both hide routes must preserve the final column, and forward drops must land before targets.
#[test]
fn column_actions_refuse_last_hide_and_use_drop_target() {
    let mut draft = MessageLayout::default();
    apply_action(&mut draft, LayoutAction::HideColumn(ReportColumn::Trades));
    assert!(!apply_action(
        &mut draft,
        LayoutAction::HideColumn(ReportColumn::Profit)
    ));
    apply_action(
        &mut draft,
        LayoutAction::RestoreColumn(ReportColumn::Trades),
    );
    apply_action(
        &mut draft,
        LayoutAction::RestoreColumn(ReportColumn::Volume),
    );
    let self_drop = column_drop_index(
        &draft.report.columns,
        &ReportColumn::Profit,
        Some(&ReportColumn::Profit),
    );
    assert!(!apply_action(
        &mut draft,
        LayoutAction::Column(ReportColumn::Profit, self_drop)
    ));
    let index = column_drop_index(
        &draft.report.columns,
        &ReportColumn::Profit,
        Some(&ReportColumn::Volume),
    );
    apply_action(
        &mut draft,
        LayoutAction::Column(ReportColumn::Profit, index),
    );
    assert_eq!(
        draft.report.columns,
        vec![
            ReportColumn::Trades,
            ReportColumn::Profit,
            ReportColumn::Volume
        ]
    );
}
