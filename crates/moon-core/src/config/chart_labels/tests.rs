use super::*;

/// The shipped default is the developer's own working layout, transcribed from their `charts.json`
/// on 2026-08-20. Pinned because it is a DECISION, not a guess: a silent edit here changes the
/// chart every fresh profile opens on.
#[test]
fn the_default_is_the_shipped_working_layout() {
    let cfg = ChartLabelsCfg::default();
    let drawn: Vec<_> = cfg
        .slots
        .iter()
        .filter(|s| s.is_drawn())
        .map(|s| (s.field, s.zone, s.align, s.inline))
        .collect();
    use ChartLabelField as F;
    use LabelAlign as A;
    use LabelZone as Z;
    assert_eq!(
        drawn,
        vec![
            // Corner block: coin over the strip, core name under it, both pushed right.
            (F::Coin, Z::ZoneTop, A::Right, false),
            // The Y-scale badge rides the plot's own top-right corner.
            (F::ScaleBadge, Z::ChartTop, A::Right, false),
            (F::Core, Z::ZoneTop, A::Right, false),
            // One row of open-order figures along the plot's top edge, on the left.
            (F::OpenOrders, Z::ChartTop, A::Left, false),
            (F::Exposure, Z::ChartTop, A::Left, true),
            (F::OpenPnlMoney, Z::ChartTop, A::Left, true),
            (F::OpenPnlPct, Z::ChartTop, A::Left, true),
        ]
    );
    assert_eq!(
        cfg.slots[3].style.color,
        Some(LabelColor::Fixed(0x8d99ae)),
        "the order count is muted so the money beside it leads"
    );
}

/// The figures inherited from the overlay carry their captions: a bare "2" over the candles names
/// nothing, and the badge they replaced read "Ордера: 2".
#[test]
fn the_order_figures_are_captioned_by_default() {
    for field in [
        ChartLabelField::OpenOrders,
        ChartLabelField::Exposure,
        ChartLabelField::PosSize,
    ] {
        assert!(field.default_style().caption, "{field:?} must name itself");
        assert!(
            field.caption_key().is_some(),
            "{field:?} has no short caption"
        );
    }
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
    let mut slot = ChartLabelSlot::new(ChartLabelField::Core, LabelZone::ChartTop);
    slot.style.size_mult = Some(99.0);
    assert_eq!(slot.resolved_style().size_mult, LABEL_SIZE_MULT_MAX);
    slot.style.size_mult = Some(0.001);
    assert_eq!(slot.resolved_style().size_mult, LABEL_SIZE_MULT_MIN);
}

/// The first drawn caption has no row before it to join, so the flag is cleared rather than
/// reaching the layout pass.
#[test]
fn the_first_drawn_slot_cannot_be_inline() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.slots[0] = ChartLabelSlot::inline(ChartLabelField::Coin, LabelZone::ZoneTop);
    cfg.sanitize();
    assert!(!cfg.slots[0].inline, "nothing precedes it to join");
}

/// THE rule this feature turns on: joining a row moves the caption INTO that row's zone.
///
/// Breakage this pins: letting an inline slot keep its own zone. It then drifts to the corner that
/// zone names, becomes the first caption there, loses the inline flag on arrival — and to the user
/// the label simply vanished from where they put it.
#[test]
fn an_inline_slot_takes_the_zone_of_the_row_it_joins() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); CHART_LABEL_SLOTS],
    };
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ChartTop);
    // Marked inline while still pointing at a different band — exactly what the popup produces
    // when the user ticks the toggle on a caption that was somewhere else.
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Delta1h, LabelZone::ChartBottom);
    cfg.slots[2] = ChartLabelSlot::new(ChartLabelField::Core, LabelZone::ChartTop);
    cfg.sanitize();
    assert_eq!(
        cfg.slots[1].zone,
        LabelZone::ChartTop,
        "the joined caption follows its row into the plot's top band"
    );
    assert!(cfg.slots[1].inline, "and stays on that row");
    assert!(
        !cfg.slots[2].inline,
        "the caption after it still opens its own row"
    );
}

/// A row can be joined across zones repeatedly: the whole run collapses into the first one's zone.
#[test]
fn a_chain_of_inline_slots_all_land_in_the_head_zone() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); CHART_LABEL_SLOTS],
    };
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ChartBottom);
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Delta1h, LabelZone::ChartTop);
    cfg.slots[2] = ChartLabelSlot::inline(ChartLabelField::Delta24h, LabelZone::ZoneBottom);
    cfg.sanitize();
    assert!(
        cfg.slots[..3]
            .iter()
            .all(|s| s.zone == LabelZone::ChartBottom),
        "every caption on the row lives in the row's zone"
    );
}

/// Hidden slots do not open a row: an inline slot following only hidden ones has nothing to join.
#[test]
fn a_hidden_slot_does_not_open_a_row_for_an_inline_one() {
    let mut cfg = ChartLabelsCfg::default();
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ChartTop);
    cfg.slots[0].visible = false;
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Core, LabelZone::ChartTop);
    cfg.sanitize();
    assert!(
        !cfg.slots[1].inline,
        "the only visible caption must open its own row"
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
            ChartLabelField::OpenOrders
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
    assert!(cfg.push(ChartLabelField::Delta1h, LabelZone::ChartTop));
    assert_eq!(cfg.slots[used].field, ChartLabelField::Delta1h);
    assert_eq!(cfg.slots[used].zone, LabelZone::ChartTop);
    while cfg.push(ChartLabelField::LastPrice, LabelZone::ChartTop) {}
    assert!(
        !cfg.push(ChartLabelField::Core, LabelZone::ChartTop),
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
    cfg.push(ChartLabelField::OpenPnlPct, LabelZone::ChartTop);
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

/// The control strip is its own BAND, not an alignment of the plot's: one lies over the book, the
/// other over the candles. Collapsing them is what made "right" mean different edges on two panes.
#[test]
fn the_control_strip_is_a_band_of_its_own() {
    assert_ne!(LabelZone::ZoneTop, LabelZone::ChartTop);
    assert!(LabelZone::ZoneTop.is_control_zone() && LabelZone::ZoneBottom.is_control_zone());
    assert!(!LabelZone::ChartTop.is_control_zone());
    assert!(LabelZone::ZoneTop.is_top() && !LabelZone::ZoneBottom.is_top());
    assert_eq!(
        ChartLabelsCfg::default().slots[0].zone,
        LabelZone::ZoneTop,
        "the default caption lives in the control strip, where it has always been drawn"
    );
}

/// Alignment travels with the band when a caption joins a row: a row has ONE alignment, or the
/// captions on it would be anchored to different edges and overlap.
#[test]
fn an_inline_slot_takes_the_alignment_of_the_row_it_joins() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); CHART_LABEL_SLOTS],
    };
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ChartTop);
    cfg.slots[0].align = LabelAlign::Right;
    cfg.slots[1] = ChartLabelSlot::inline(ChartLabelField::Delta1h, LabelZone::ChartTop);
    cfg.slots[1].align = LabelAlign::Left;
    cfg.sanitize();
    assert_eq!(
        cfg.slots[1].align,
        LabelAlign::Right,
        "the row owns the alignment"
    );
}

/// A caption that opens its OWN row keeps whatever alignment it was given.
#[test]
fn a_row_head_keeps_its_own_alignment() {
    let mut cfg = ChartLabelsCfg {
        slots: [ChartLabelSlot::default(); CHART_LABEL_SLOTS],
    };
    cfg.slots[0] = ChartLabelSlot::new(ChartLabelField::Coin, LabelZone::ZoneTop);
    cfg.slots[0].align = LabelAlign::Right;
    cfg.slots[1] = ChartLabelSlot::new(ChartLabelField::Core, LabelZone::ZoneTop);
    cfg.slots[1].align = LabelAlign::Left;
    cfg.sanitize();
    assert_eq!(cfg.slots[0].align, LabelAlign::Right);
    assert_eq!(
        cfg.slots[1].align,
        LabelAlign::Left,
        "a second row in the same band may be anchored to the other edge"
    );
}

/// Legacy files named the alignment inside the zone. They must still load, landing in the matching
/// band rather than being rejected — the alignment then falls back to the default.
///
/// The slot list is a fixed-length array, so the fixture is a REAL document with one value swapped:
/// a hand-built fragment would fail on the array length and prove nothing about the alias.
#[test]
fn the_legacy_zone_spellings_still_load() {
    let text = toml::to_string_pretty(&ChartLabelsCfg::default()).expect("serializes");
    let legacy = text.replace("zone = \"zone_top\"", "zone = \"top_right\"");
    assert!(
        legacy.contains("top_right"),
        "the fixture really carries the old spelling"
    );
    let back: ChartLabelsCfg = toml::from_str(&legacy).expect("an old spelling parses");
    assert_eq!(
        back.slots[0].zone,
        LabelZone::ChartTop,
        "top_right lands in the plot's top band"
    );
}
