use super::super::state::PendingDelta;
use super::{ChipKind, FieldChip, ListFieldModel, MAX_CHIPS, pending_text};
use gpui::SharedString;

fn chip(token: &str, kind: ChipKind) -> FieldChip {
    FieldChip {
        token: SharedString::from(token.to_string()),
        text: SharedString::from(token.to_string()),
        tip: SharedString::default(),
        id: SharedString::from(format!("an-pick-b-{token}")),
        kind,
        alpha: 1.0,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// An empty whitelist says what empty MEANS for the strategy, and offers nothing to clear
/// or copy.
#[test]
fn an_empty_whitelist_explains_itself_and_cannot_be_cleared() {
    let _locale = crate::test_locale::force("en");
    let m = ListFieldModel::new(false, Some(Vec::new()));
    assert_eq!(m.count, Some(0));
    assert!(!m.can_clear && !m.can_copy);
    assert_eq!(m.key, "CoinsWhiteList");
    assert_eq!(m.empty.as_deref(), Some("The list is empty."));
    assert!(
        m.legend
            .iter()
            .any(|l| l.contains("every coin except the blacklist")),
        "legend: {:?}",
        m.legend
    );
}

/// Without a strategy both fields say so, and the count is not claimed.
#[test]
fn no_strategy_fields_say_so() {
    let _locale = crate::test_locale::force("en");
    for black in [true, false] {
        let m = ListFieldModel::new(black, None);
        assert_eq!(m.count, None);
        assert_eq!(m.empty.as_deref(), Some("No strategy selected"));
        assert!(!m.can_clear && !m.can_copy);
    }
}

/// The count is the WORKING list: a removal shown for undo is not counted, and a cut past
/// the chip cap is stated.
#[test]
fn removed_chips_are_not_counted_and_the_cap_is_stated() {
    let _locale = crate::test_locale::force("en");
    let mut chips = vec![
        chip("XAAA", ChipKind::Saved),
        chip("XBBB", ChipKind::Removed),
    ];
    chips.push(chip("XCCC", ChipKind::Added));
    let m = ListFieldModel::new(true, Some(chips));
    assert_eq!(m.count, Some(2));
    assert!(m.can_clear);

    let many: Vec<_> = (0..MAX_CHIPS + 3)
        .map(|i| chip(&format!("X{i:04}"), ChipKind::Saved))
        .collect();
    let m = ListFieldModel::new(true, Some(many));
    assert_eq!((m.chips.len(), m.more), (MAX_CHIPS, 3));
}

/// The band is built from the pending delta: per side, sorted, additions then removals.
#[test]
fn the_pending_band_is_signed_per_side() {
    let _locale = crate::test_locale::force("en");
    assert_eq!(pending_text(&PendingDelta::default()), None);
    let d = PendingDelta {
        added_bl: strings(&["XAAA", "XBBB"]),
        removed_bl: strings(&["XCCC"]),
        added_wl: Vec::new(),
        removed_wl: strings(&["XDDD"]),
    };
    assert_eq!(
        pending_text(&d).as_deref(),
        Some("BL +XAAA +XBBB \u{2212}XCCC · WL \u{2212}XDDD")
    );
}

/// The count is in COINS: two written entries of one coin count once.
#[test]
fn two_entries_of_one_coin_count_once() {
    let _locale = crate::test_locale::force("en");
    let mut second = chip("BTC", ChipKind::Saved);
    second.text = SharedString::from("BTC_0626");
    let m = ListFieldModel::new(true, Some(vec![chip("BTC", ChipKind::Saved), second]));
    assert_eq!(m.count, Some(1));
    assert_eq!(m.chips.len(), 2, "both entries are still drawn");
}
