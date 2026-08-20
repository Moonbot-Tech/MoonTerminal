use super::*;

/// The default configuration IS the caption the chart drew before it was configurable. If this
/// changes, every existing profile silently gets a different chart corner.
#[test]
fn the_default_reproduces_the_hard_coded_caption() {
    let cfg = ChartLabelsCfg::default();
    let drawn: Vec<_> = cfg
        .slots
        .iter()
        .filter(|s| s.is_drawn())
        .map(|s| (s.field, s.zone, s.inline))
        .collect();
    assert_eq!(
        drawn,
        vec![
            (ChartLabelField::Coin, LabelZone::ZoneTop, false),
            (ChartLabelField::ScaleBadge, LabelZone::ZoneTop, true),
            (ChartLabelField::Core, LabelZone::ZoneTop, false),
            (ChartLabelField::CompareDelta, LabelZone::ZoneTop, false),
        ],
        "the coin leads with the badge on its row, then the core name, then the comparison delta"
    );
}

/// The coin is set one size up and the comparison delta larger still: those two sizes are the
/// caption's whole visual hierarchy.
#[test]
fn default_styles_keep_the_captions_size_hierarchy() {
    let coin = ChartLabelField::Coin.default_style();
    let badge = ChartLabelField::ScaleBadge.default_style();
    let delta = ChartLabelField::CompareDelta.default_style();
    let core = ChartLabelField::Core.default_style();
    assert!(
        coin.size_mult > core.size_mult,
        "the coin leads the core name"
    );
    assert!(
        delta.size_mult > badge.size_mult,
        "the comparison delta stays dominant over the scale badge"
    );
    assert_eq!(
        delta.color,
        LabelColor::BySign,
        "a signed figure colors by sign"
    );
    assert_eq!(core.color, LabelColor::Theme);
}

#[test]
fn a_partial_style_override_leaves_the_rest_on_the_field_default() {
    let mut slot = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ZoneTop);
    slot.style.color = Some(LabelColor::Fixed(0x00ff00));
    let resolved = slot.resolved_style();
    assert_eq!(resolved.color, LabelColor::Fixed(0x00ff00));
    assert_eq!(
        resolved.size_mult,
        ChartLabelField::Coin.default_style().size_mult,
        "an untouched size still follows the field"
    );
}

#[test]
fn an_out_of_range_size_is_clamped_rather_than_drawn() {
    let mut slot = ChartLabelSlot::new(ChartLabelField::Core, LabelZone::TopLeft);
    slot.style.size_mult = Some(99.0);
    assert_eq!(slot.resolved_style().size_mult, LABEL_SIZE_MULT_MAX);
    slot.style.size_mult = Some(0.001);
    assert_eq!(slot.resolved_style().size_mult, LABEL_SIZE_MULT_MIN);
}

/// A hand-edited file can mark the first label of a corner as inline. There is no row for it to
/// join, so the flag is cleared instead of reaching the layout pass.
#[test]
fn the_first_slot_of_a_zone_cannot_be_inline() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.slots[0] = ChartLabelSlot::inline(ChartLabelField::Coin, LabelZone::ZoneTop);
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Core, LabelZone::TopLeft);
    cfg.sanitize();
    assert!(!cfg.slots[0].inline, "first in ZoneTop opens its own row");
    assert!(
        !cfg.slots[1].inline,
        "first in TopLeft opens its own row too"
    );
}

/// Hidden slots do not count as opening a row: an inline slot following only hidden ones would
/// have nothing to attach to.
#[test]
fn a_hidden_slot_does_not_open_a_row_for_an_inline_one() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::TopLeft);
    cfg.slots[0].visible = false;
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Core, LabelZone::TopLeft);
    cfg.sanitize();
    assert!(
        !cfg.slots[1].inline,
        "the only visible slot of the zone must open its own row"
    );
}

#[test]
fn removing_a_slot_closes_the_gap() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.remove(0);
    let fields: Vec<_> = cfg.slots.iter().take(3).map(|s| s.field).collect();
    assert_eq!(
        fields,
        vec![
            ChartLabelField::ScaleBadge,
            ChartLabelField::Core,
            ChartLabelField::CompareDelta
        ],
        "the survivors keep their relative order with no hole between them"
    );
    assert!(
        !cfg.slots[0].inline,
        "the badge inherited the first position and cannot stay inline"
    );
}

#[test]
fn moving_refuses_at_the_ends_and_swaps_in_the_middle() {
    let mut cfg = ChartLabelsCfg::default();
    assert!(!cfg.move_slot(0, true), "the first slot cannot move up");
    let used = cfg.used_len();
    assert!(
        !cfg.move_slot(used - 1, false),
        "the last used slot cannot move down"
    );
    assert!(cfg.move_slot(2, true));
    assert_eq!(cfg.slots[1].field, ChartLabelField::Core);
    assert_eq!(cfg.slots[2].field, ChartLabelField::ScaleBadge);
}

#[test]
fn moving_ignores_indices_past_the_used_run() {
    let mut cfg = ChartLabelsCfg::default();
    let used = cfg.used_len();
    assert!(
        !cfg.move_slot(used, true),
        "an empty slot is not a movable label"
    );
    assert!(!cfg.move_slot(CHART_LABEL_SLOTS + 5, false));
}

#[test]
fn push_fills_the_first_free_slot_and_reports_a_full_list() {
    let mut cfg = ChartLabelsCfg::default();
    let used = cfg.used_len();
    assert!(cfg.push(ChartLabelField::Delta1h, LabelZone::TopLeft));
    assert_eq!(cfg.slots[used].field, ChartLabelField::Delta1h);
    assert_eq!(cfg.slots[used].zone, LabelZone::TopLeft);
    while cfg.push(ChartLabelField::LastPrice, LabelZone::TopLeft) {}
    assert!(
        !cfg.push(ChartLabelField::Core, LabelZone::TopLeft),
        "a full list refuses instead of dropping an existing label"
    );
}

#[test]
fn the_basis_selects_which_orders_count() {
    assert!(PnlBasis::All.accepts(true) && PnlBasis::All.accepts(false));
    assert!(PnlBasis::Real.accepts(false) && !PnlBasis::Real.accepts(true));
    assert!(PnlBasis::Emulator.accepts(true) && !PnlBasis::Emulator.accepts(false));
}

/// Only fields that actually read a basis offer the control, so switching a slot's field cannot
/// leave a stale basis visible in the popup.
#[test]
fn only_position_fields_use_the_basis() {
    assert!(ChartLabelField::OpenPnlPct.uses_pnl_basis());
    assert!(ChartLabelField::PosSize.uses_pnl_basis());
    assert!(!ChartLabelField::Coin.uses_pnl_basis());
    assert!(!ChartLabelField::Delta1h.uses_pnl_basis());
}

/// Every assignable field must be reachable through a menu section, or it can be configured in a
/// file and never removed through the UI.
#[test]
fn every_field_belongs_to_a_menu_group_and_has_a_locale_key() {
    for field in ChartLabelField::ALL {
        assert!(
            ChartLabelGroup::ALL.contains(&field.group()),
            "{field:?} has no menu section"
        );
        assert!(
            field.locale_key().starts_with("chart_labels.field."),
            "{field:?} has no locale key"
        );
    }
}

/// The configuration travels inside `StackSetting`, which the ⧉ walk copies by value.
#[test]
fn the_configuration_round_trips_through_toml() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.slots[0].style.color = Some(LabelColor::Fixed(0x112233));
    cfg.slots[0].style.size_mult = Some(1.5);
    cfg.push(ChartLabelField::OpenPnlPct, LabelZone::TopLeft);
    let text = toml::to_string_pretty(&cfg).expect("serializes");
    let back: ChartLabelsCfg = toml::from_str(&text).expect("parses");
    assert_eq!(back, cfg);
}

/// A file written before this feature existed carries no labels at all, and must land on the
/// caption the chart already drew rather than on an empty corner.
#[test]
fn an_absent_table_loads_as_the_default_caption() {
    let back: ChartLabelsCfg = toml::from_str("").expect("parses an empty document");
    assert_eq!(back, ChartLabelsCfg::default());
}

/// The control strip and the plot's right edge are DIFFERENT zones: one sits over the book, the
/// other over the candles. Collapsing them is what made "right" ambiguous on a pane with a book.
#[test]
fn the_control_zone_is_not_the_plots_right_corner() {
    assert_ne!(LabelZone::ZoneTop, LabelZone::TopRight);
    assert!(LabelZone::ZoneTop.is_control_zone() && LabelZone::ZoneBottom.is_control_zone());
    assert!(!LabelZone::TopRight.is_control_zone());
    assert!(LabelZone::ZoneTop.is_top() && !LabelZone::ZoneBottom.is_top());
    assert_eq!(
        ChartLabelsCfg::default().slots[0].zone,
        LabelZone::ZoneTop,
        "the default caption lives in the control strip, where it has always been drawn"
    );
}
