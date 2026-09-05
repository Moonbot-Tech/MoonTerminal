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

/// Pins `hotkeys.rs::clear_generation_3_collisions`: the arriving Ctrl+Z default must yield to a
/// user who had already given that keystroke away.
///
/// Plausible breakage: the figure layer resolves ABOVE the trading actions, so a duplicated Ctrl+Z
/// would silently turn an order-sending key into "delete the last figure" — and the file's own
/// generation gate would never look at it again.
#[test]
fn generation_3_yields_fig_undo_to_an_existing_binding() {
    let mut existing_file = HotkeysConfig {
        schema: 2,
        new_long: "ctrl-z".into(),
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.new_long, "ctrl-z");
    assert!(
        existing_file.fig_undo.is_empty(),
        "the new figure-undo default must yield to the user's existing binding"
    );
    assert_eq!(existing_file.schema, SCHEMA);
}

/// A file that has NOT given Ctrl+Z away keeps the shipped default, so the feature is not
/// switched off for everybody by the collision check that exists for the few.
#[test]
fn generation_3_keeps_fig_undo_on_a_file_that_never_used_it() {
    let mut existing_file = HotkeysConfig {
        schema: 2,
        ..HotkeysConfig::default()
    };

    existing_file.fill_unbound_slots();

    assert_eq!(existing_file.fig_undo, "ctrl-z");
}

/// Every tool takes part in the switch-figure cycle until one is switched off, including in a file
/// written before the exclusion list existed.
///
/// Plausible breakage: shipping an INCLUSION list would read an old file — and a fresh install —
/// as "no tool participates", which is a hotkey that silently does nothing.
#[test]
fn no_tool_is_excluded_from_the_cycle_by_default() {
    assert!(HotkeysConfig::default().switch_figure_skip.is_empty());
    let old_file: HotkeysConfig =
        toml::from_str("").expect("a hotkeys.toml with no keys at all must still load");
    assert!(old_file.switch_figure_skip.is_empty());
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

/// Pins which command a recognised move gesture becomes.
///
/// Plausible breakage: reading the slot's twin — the entry kind for an exit gesture, the primary
/// kind for the secondary slot. Both send a live bulk move built from the wrong half of the
/// settings page, and the orders that come back are not the ones the user was looking at.
#[test]
fn a_move_gesture_resolves_to_its_own_side_and_kind() {
    let hk = HotkeysConfig {
        buy_move_click: MouseGestureBinding::LeftShift,
        sell_move_click: MouseGestureBinding::LeftCtrl,
        buy_move_click2: MouseGestureBinding::MiddleShift,
        sell_move_click2: MouseGestureBinding::MiddleCtrl,
        buy_move_kind: MoveKind::AllToOnePrice,
        sell_move_kind: MoveKind::ParallelShift,
        buy_move_kind2: MoveKind::TopVolume,
        sell_move_kind2: MoveKind::LastMoved,
        same_hotkeys_for_move: false,
        short_buy_move_click: MouseGestureBinding::None,
        short_sell_move_click: MouseGestureBinding::None,
        short_buy_move_click2: MouseGestureBinding::None,
        short_sell_move_click2: MouseGestureBinding::None,
        ..HotkeysConfig::default()
    };
    let resolve = |pressed: MouseGestureBinding| hk.resolve_move_gesture(|b| b == pressed);

    assert_eq!(
        resolve(MouseGestureBinding::LeftShift),
        Some(MoveGestureCommand {
            sell: false,
            kind: MoveKind::AllToOnePrice,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::LeftCtrl),
        Some(MoveGestureCommand {
            sell: true,
            kind: MoveKind::ParallelShift,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::MiddleShift),
        Some(MoveGestureCommand {
            sell: false,
            kind: MoveKind::TopVolume,
            side: MoveSide::Long,
        })
    );
    assert_eq!(
        resolve(MouseGestureBinding::MiddleCtrl),
        Some(MoveGestureCommand {
            sell: true,
            kind: MoveKind::LastMoved,
            side: MoveSide::Long,
        })
    );
    assert_eq!(resolve(MouseGestureBinding::LeftAlt), None);
}

/// Pins the side a press addresses, which is what the mirror flag decides.
///
/// Plausible breakage: sending `Long` for a gesture the user shares between both sides. On a hedged
/// market that moves one side's orders and silently leaves the other where it was, which reads as
/// "half the command worked".
#[test]
fn a_shared_gesture_addresses_both_sides() {
    let mirrored = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        mirrored
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.side),
        Some(MoveSide::Both),
        "the mirror makes one press mean both sides"
    );

    let split = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        short_sell_move_click: MouseGestureBinding::LeftAlt,
        same_hotkeys_for_move: false,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        split
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.side),
        Some(MoveSide::Long)
    );
    assert_eq!(
        split
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftAlt)
            .map(|c| c.side),
        Some(MoveSide::Short)
    );
}

/// Pins that a bound gesture with no kind sends nothing.
///
/// Plausible breakage: treating `MoveKind::None` as a kind and putting it on the wire. Moonbot uses
/// it to switch a gesture off without clearing the binding, and the core has no arrangement to
/// apply — so what a "none" move actually did to live orders would be anyone's guess.
#[test]
fn a_gesture_without_a_kind_is_inert() {
    let hk = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        sell_move_kind: MoveKind::None,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        hk.resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl),
        None
    );

    // And an inert slot must not shadow a later one holding the same binding: the row the user
    // switched off would silence the row they are still using.
    let shared = HotkeysConfig {
        sell_move_click: MouseGestureBinding::LeftCtrl,
        sell_move_kind: MoveKind::None,
        sell_move_click2: MouseGestureBinding::LeftCtrl,
        sell_move_kind2: MoveKind::AllToOnePrice,
        same_hotkeys_for_move: true,
        ..HotkeysConfig::default()
    };
    assert_eq!(
        shared
            .resolve_move_gesture(|b| b == MouseGestureBinding::LeftCtrl)
            .map(|c| c.kind),
        Some(MoveKind::AllToOnePrice)
    );
}

/// Pins the shipped kind, which is what a fresh install and every old `hotkeys.toml` get.
///
/// Plausible breakage: leaving the derived `None` as the default. Every move gesture would be
/// recognised and do nothing, which is indistinguishable from the gesture being broken.
#[test]
fn the_shipped_move_kind_is_moonbots_parallel_shift() {
    let shipped = HotkeysConfig::default();
    for kind in [
        shipped.buy_move_kind,
        shipped.sell_move_kind,
        shipped.buy_move_kind2,
        shipped.sell_move_kind2,
    ] {
        assert_eq!(kind, MoveKind::ParallelShift);
    }
    // A file written before this field existed loads with the same value rather than an inert one.
    let old: HotkeysConfig = toml::from_str("schema = 2\n").expect("an old file still loads");
    assert_eq!(old.sell_move_kind, MoveKind::ParallelShift);
    assert_eq!(old.buy_move_kind2, MoveKind::ParallelShift);
}

/// The POSITION of a variant in these two lists is a wire value.
///
/// `feed::GestureSettings` carries Moonbot's mouse gestures and move kinds as the raw bytes the
/// safe-share config holds, and the expert window's Hotkeys page turns a byte into a menu entry by
/// indexing `ALL`. Nothing else pins the two together, so reordering either list — a harmless-
/// looking edit, since both are "just a display order" — would silently rewrite every core's stored
/// gestures on the next OK.
///
/// The anchors are moonproto's own annotated defaults (`shared_config/sections.rs`:
/// `buy_set_click: 1, // Dbl_Click`, `sell_move_click: 2, // CTRL_Click`) and its
/// `ReplaceMultiKind` constants (`commands/trade/enums.rs`, `TReplaceMultiKind` at Vars.pas:37),
/// which run None=0, Shift=1, TopVol=2, LowVol=3, TopProfit=4, All=5, LastSet=6, LastMoved=7.
#[test]
fn wire_ordinals_are_the_positions_in_these_lists() {
    // Every position, not a handful: a reorder in the middle of the list is exactly as damaging
    // as one at its ends, and pinning only the ends would let it through.
    assert_eq!(
        MouseGestureBinding::ALL,
        [
            MouseGestureBinding::None,
            MouseGestureBinding::LeftDouble,
            MouseGestureBinding::LeftCtrl,
            MouseGestureBinding::LeftShift,
            MouseGestureBinding::LeftAlt,
            MouseGestureBinding::Middle,
            MouseGestureBinding::MiddleCtrl,
            MouseGestureBinding::MiddleShift,
            MouseGestureBinding::MiddleAlt,
            MouseGestureBinding::RightDouble,
            MouseGestureBinding::RightCtrl,
            MouseGestureBinding::RightShift,
            MouseGestureBinding::RightAlt,
            MouseGestureBinding::LeftCtrlDouble,
            MouseGestureBinding::LeftShiftDouble,
            MouseGestureBinding::LeftAltDouble,
        ]
    );
    assert_eq!(
        MoveKind::ALL,
        [
            MoveKind::None,
            MoveKind::ParallelShift,
            MoveKind::TopVolume,
            MoveKind::LowVolume,
            MoveKind::TopProfit,
            MoveKind::AllToOnePrice,
            MoveKind::LastSet,
            MoveKind::LastMoved,
        ]
    );
}
