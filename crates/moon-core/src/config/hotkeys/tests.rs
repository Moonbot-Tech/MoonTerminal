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
    assert_eq!(old.new_long, "f9", "a key the user set is never overwritten");
    assert_eq!(old.schema, SCHEMA);

    // Clearing a slot after the migration ran must STAY cleared across loads.
    old.cancel_buy = String::new();
    old.fill_unbound_slots();
    assert!(old.cancel_buy.is_empty());
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
