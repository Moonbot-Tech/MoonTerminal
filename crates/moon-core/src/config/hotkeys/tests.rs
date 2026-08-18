use super::*;

/// Pins the one-time backfill of slots that shipped unbound.
///
/// Plausible breakage: running the fill on every load hands back a hotkey the user deliberately
/// cleared on the next launch; running it never leaves everyone who already has a `hotkeys.toml`
/// without the Moonbot keys this build ships.
#[test]
fn unbound_slots_are_filled_once_and_user_choices_survive() {
    let shipped = HotkeysConfig::default();
    // A file from the build before these defaults: no generation, empty slots, and one key the
    // user chose for themselves.
    let mut old = HotkeysConfig {
        schema: 0,
        cancel_buy: String::new(),
        join_sells: String::new(),
        sells_to_rect: String::new(),
        new_long: "f9".into(),
        ..shipped.clone()
    };
    old.fill_unbound_slots();
    assert_eq!(old.cancel_buy, shipped.cancel_buy);
    assert_eq!(old.join_sells, shipped.join_sells);
    assert_eq!(old.sells_to_rect, shipped.sells_to_rect);
    assert_eq!(
        old.new_long, "f9",
        "a key the user set is never overwritten"
    );
    assert_eq!(old.schema, SCHEMA);

    // Clearing a slot after the migration ran must STAY cleared across loads.
    old.cancel_buy = String::new();
    old.fill_unbound_slots();
    assert!(old.cancel_buy.is_empty());
}

/// Pins `hotkeys.rs::clear_generation_2_collisions` against an off-by-one duplicate threshold.
///
/// Plausible breakage: treating the two holders of Ctrl+F10 as non-colliding leaves the new chart
/// shot above the user's existing panic-sell binding, so that keystroke captures a chart instead
/// of sending the order it used to send.
#[test]
fn generation_2_yields_chart_shot_to_an_existing_binding() {
    let mut existing_file = HotkeysConfig {
        schema: 1,
        panic_sell: "ctrl-f10".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.panic_sell, "ctrl-f10");
    assert!(
        existing_file.chart_shot.is_empty(),
        "the new chart-shot default must yield to the user's existing binding"
    );
}

/// Pins `hotkeys.rs::fill_unbound_slots` so generation 1 cannot re-run on a generation-1 file.
///
/// Plausible breakage: collapsing the generation gates restores a deliberately cleared Cancel Buy
/// key while upgrading the chart-shot slot, so a user can accidentally send an order they disabled.
#[test]
fn generation_2_preserves_slots_deliberately_cleared_after_generation_1() {
    let mut existing_file = HotkeysConfig {
        schema: 1,
        cancel_buy: String::new(),
        panic_sell: "ctrl-f10".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert!(
        existing_file.cancel_buy.is_empty(),
        "generation 1 must not restore a key cleared after its migration"
    );
    assert!(
        existing_file.chart_shot.is_empty(),
        "generation 2 must still clear the chart-shot collision"
    );
}

/// Pins that the shipped keyboard defaults are Moonbot's own, so a user switching over finds
/// their keys where they left them.
///
/// Plausible breakage: an invented default silently diverges from Moonbot and nothing says so.
#[test]
fn keyboard_defaults_match_moonbot() {
    let h = HotkeysConfig::default();
    for (actual, expected, name) in [
        (&h.cancel_buy, "alt-z", "Cancel Buy"),
        (&h.panic_sell, "alt-6", "Panic Sell"),
        (&h.panic_sell_one, "alt-5", "Panic Sell 1 order"),
        (&h.cancel_all_buys, "alt-a", "Cancel ALL buys"),
        (&h.join_sells, "alt-e", "Объединить Sell"),
        (&h.switch_charts, "alt-f", "Переключ. графиков"),
        (&h.switch_figure, "alt-d", "Switch Chart Figure"),
        (&h.new_long, "alt-1", "New Long"),
        (&h.new_short, "alt-3", "New Short"),
        (&h.split_order, "alt-c", "Split Order"),
        (&h.split_order_x, "ctrl-x", "Split to N"),
        (&h.sells_to_rect, "ctrl-s", "Sells to rectangle"),
        (&h.shift_buy_up, "shift-up", "Shift buys +1%"),
        (&h.shift_buy_down, "shift-down", "Shift buys -1%"),
        (&h.shift_sell_up, "alt-up", "Shift sells +1%"),
        (&h.shift_sell_down, "alt-down", "Shift sells -1%"),
        (&h.scale_plus, "ctrl-q", "Scale +"),
        (&h.scale_minus, "ctrl-w", "Scale -"),
    ] {
        assert_eq!(actual, expected, "{name} must ship Moonbot's key");
    }
}

/// Pins the order-shift step as WHOLE percent, which is the unit the protocol takes everywhere.
///
/// Plausible breakage: reading Moonbot's "±1%" as a fraction and shipping 0.01 would move live
/// orders by a hundredth of a percent, which looks like the hotkey doing nothing; shipping 100.0
/// would move them by a factor of two. Neither is visible in a type.
#[test]
fn order_shift_is_one_whole_percent() {
    assert_eq!(SHIFT_PERCENT, 1.0);
}

/// Pins the `Split N` count against a hand-edited or imported file.
///
/// Plausible breakage: reading the raw field sends a live split into one part, or into 200.
#[test]
fn split_n_parts_stays_inside_its_range() {
    let clamp = |split_parts| {
        HotkeysConfig {
            split_parts,
            ..HotkeysConfig::default()
        }
        .split_n_parts()
    };
    assert_eq!(clamp(0), i32::from(SPLIT_PARTS_MIN));
    assert_eq!(clamp(1), i32::from(SPLIT_PARTS_MIN));
    assert_eq!(clamp(4), 4);
    assert_eq!(clamp(200), i32::from(SPLIT_PARTS_MAX));
}
