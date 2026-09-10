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
/// safe-share config holds, and TWO surfaces turn a byte into a value by indexing `ALL`: the expert
/// window's Hotkeys page, and `settings::hotkeys::pull_gestures`, which decodes the same bytes into
/// this terminal's own layout. Nothing else pins the lists to the wire, so reordering either — a
/// harmless-looking edit, since both are "just a display order" — would silently rewrite every
/// core's stored gestures on the next OK and mis-import every pulled gesture.
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

/// The figure-delete gesture is a SETTING with a shipped default, not a hardcoded button.
///
/// Plausible breakage: a bare serde default lands `None` on every file written before the field
/// existed, so the gesture the user has been using silently stops working after an update — and
/// nothing in the UI says why, because the row shows exactly what the file said.
#[test]
fn the_shipped_figure_delete_gesture_is_the_middle_click() {
    assert_eq!(
        HotkeysConfig::default().fig_delete_click,
        MouseGestureBinding::Middle
    );
    let old: HotkeysConfig = toml::from_str("schema = 2\n").expect("an old file still loads");
    assert_eq!(old.fig_delete_click, MouseGestureBinding::Middle);

    // And a file that names it keeps what it names, `None` (gesture off) included.
    let off: HotkeysConfig =
        toml::from_str("fig_delete_click = \"none\"\n").expect("an explicit gesture loads");
    assert_eq!(off.fig_delete_click, MouseGestureBinding::None);
    let right: HotkeysConfig =
        toml::from_str("fig_delete_click = \"right-ctrl\"\n").expect("an explicit gesture loads");
    assert_eq!(right.fig_delete_click, MouseGestureBinding::RightCtrl);
}

/// Fields of `HotkeysConfig` that hold a string but not a KEYSTROKE, and so name no slot.
///
/// Written out because the check below cannot tell a gesture's `"middle"` from a keystroke by
/// looking — and because being explicit is the point: a new keystroke field has nowhere to hide.
/// Adding a gesture without adding it here fails the test too, which is the right direction to
/// fail in.
const NOT_KEYSTROKES: [&str; 18] = [
    // Not a binding at all: the tools excluded from the switch cycle.
    "switch_figure_skip",
    "fig_delete_click",
    "buy_set_click",
    "short_set_click",
    "pending_long_click",
    "pending_short_click",
    "buy_move_click",
    "sell_move_click",
    "buy_move_click2",
    "sell_move_click2",
    "short_buy_move_click",
    "short_sell_move_click",
    "short_buy_move_click2",
    "short_sell_move_click2",
    "buy_move_kind",
    "sell_move_kind",
    "buy_move_kind2",
    "sell_move_kind2",
];

/// Every keystroke the file stores is reachable through a [`KeySlot`] — checked against the STRUCT,
/// not against the slot list.
///
/// The distinction is the whole test. Walking `KeySlot::all()` and asking `bound_keys()` what it
/// found proves nothing: `bound_keys()` is now derived from that same list, so a field no slot names
/// is invisible to both, and the first version of this test passed exactly that way. It is checked
/// through serialization instead — every slot is written a marker, and any string left in the file
/// that is not a marker has to be a declared non-keystroke.
///
/// What a missed field costs: `bound_keys()` never sees it, so the migration's collision check and
/// the core pull's conflict gate both read that keystroke as free, and a shipped default takes a key
/// the user is already using — silently, because nothing else looks.
#[test]
fn no_stored_keystroke_is_missing_from_the_registry() {
    const MARKER: &str = "zz-registry-marker";

    let mut cfg = HotkeysConfig::default();
    for slot in KeySlot::all() {
        cfg.set_key(slot, MARKER.to_string());
    }
    let text = toml::to_string(&cfg).expect("serialize hotkeys");
    let table: toml::Table = text.parse().expect("re-read hotkeys");

    let mut unreached = Vec::new();
    let mut nested = Vec::new();
    for (name, value) in &table {
        if NOT_KEYSTROKES.contains(&name.as_str()) {
            continue;
        }
        let strings: Vec<&str> = match value {
            toml::Value::String(s) => vec![s.as_str()],
            toml::Value::Array(items) => items.iter().filter_map(toml::Value::as_str).collect(),
            // The config is FLAT, and this walk only looks one level down. A nested table would
            // carry its keystrokes past the check unseen, so it fails here rather than passing
            // quietly — extend the walk when one arrives.
            toml::Value::Table(_) => {
                nested.push(name.clone());
                continue;
            }
            _ => continue,
        };
        // An EMPTY array counts as unreached too: every slot was just written a marker, so a
        // keystroke field holding none of them is one no slot addresses.
        if strings.is_empty() || strings.iter().any(|held| *held != MARKER) {
            unreached.push(name.clone());
        }
    }

    assert!(
        nested.is_empty(),
        "this walk reads only top-level values; extend it for {nested:?}"
    );
    assert!(
        unreached.is_empty(),
        "these stored keystrokes have no KeySlot, so nothing sees them bound: {unreached:?}"
    );
}

/// A slot's stem IS its key in the file — the fact every derived spelling rests on.
///
/// Checked against the serialized table, not against the accessor: `key` and `stem` are two
/// matches over the same enum, and a typo in one of them is exactly what this has to catch. A
/// family's stem names its array, with the index outside it.
///
/// Plausible breakage: a field renamed for the file with the stem left behind, which would let the
/// import id, the page id and the locale key all agree with each other and disagree with the file.
#[test]
fn a_slots_stem_is_its_key_in_the_file() {
    let mut cfg = HotkeysConfig::default();
    let marker = |slot: KeySlot| format!("zz-{slot:?}");
    for slot in KeySlot::all() {
        assert!(cfg.set_key(slot, marker(slot)));
    }
    let text = toml::to_string(&cfg).expect("serialize hotkeys");
    let table: toml::Table = text.parse().expect("re-read hotkeys");

    for slot in KeySlot::all() {
        let stored = match slot.index() {
            Some(i) => table[slot.stem()]
                .as_array()
                .and_then(|family| family.get(i))
                .and_then(toml::Value::as_str),
            None => table[slot.stem()].as_str(),
        };
        assert_eq!(
            stored,
            Some(marker(slot).as_str()),
            "{slot:?}: the file does not store this slot under its stem {:?}",
            slot.stem()
        );
    }
}

/// Every gesture the file stores is reachable through a [`GestureSlot`], and no two slots share one.
///
/// The same shape as the keystroke check above, for the same reason: `GestureSlot::all()` cannot
/// be verified by walking `GestureSlot::all()`. Every slot is written a gesture — the thirteen with
/// a field a DISTINCT one, the key halves a rotating one, since there are more of them than there
/// are gestures — the file is read back, and the gesture-shaped values in it, top level and the
/// `[action_clicks]` table together, have to be exactly what was written, as a multiset. A field no
/// slot reaches still holds its default, which is either `none` or a duplicate of a marker; a slot
/// that reaches no field writes nothing and is missing from the count. The four kind fields are
/// moved off `none` first so they cannot be mistaken for an unreached gesture.
///
/// What a missed field costs: the settings page never draws it, the core pull never writes it, and
/// the clash index never sees the gesture it holds — so a press two rows answer is captioned as
/// answered by one.
#[test]
fn no_stored_gesture_is_missing_from_the_registry() {
    let mut cfg = HotkeysConfig::default();
    for row in MoveKindSlot::ALL {
        cfg.set_move_kind(row, MoveKind::TopVolume);
    }
    let slots = GestureSlot::all();
    // None of them `None`: `ALL[0]` is the unset gesture. Every slot is cleared first, so a marker
    // that happens to be a slot's shipped default is still a change.
    let live = &MouseGestureBinding::ALL[1..];
    let markers: Vec<MouseGestureBinding> =
        (0..slots.len()).map(|i| live[i % live.len()]).collect();
    for slot in &slots {
        cfg.set_gesture(*slot, MouseGestureBinding::None);
    }
    for (slot, marker) in slots.iter().zip(&markers) {
        assert!(cfg.set_gesture(*slot, *marker), "{slot:?} stores nothing");
    }
    let text = toml::to_string(&cfg).expect("serialize hotkeys");
    let table: toml::Table = text.parse().expect("re-read hotkeys");

    let mut stored: Vec<(String, MouseGestureBinding)> = Vec::new();
    for (name, value) in &table {
        match value {
            toml::Value::String(raw) => {
                if let Some(gesture) = MouseGestureBinding::from_config_value(raw) {
                    stored.push((name.clone(), gesture));
                }
            }
            // The one nested table the file has: the key halves, keyed by slot name.
            toml::Value::Table(inner) if name == "action_clicks" => {
                for (key, value) in inner {
                    let gesture = value
                        .as_str()
                        .and_then(MouseGestureBinding::from_config_value)
                        .unwrap_or_else(|| panic!("action_clicks.{key} is not a gesture"));
                    stored.push((format!("action_clicks.{key}"), gesture));
                }
            }
            toml::Value::Table(_) => {
                panic!("this walk reads one nested table; extend it for {name}")
            }
            _ => {}
        }
    }
    let mut found: Vec<&str> = stored.iter().map(|(_, g)| g.config_value()).collect();
    found.sort_unstable();
    let mut expected: Vec<&str> = markers.iter().map(|g| g.config_value()).collect();
    expected.sort_unstable();
    assert_eq!(
        found, expected,
        "the file's gesture values are not exactly the slots' markers: {stored:?}"
    );

    // Read back through the accessor too, all writes before any read, so an alias cannot pass.
    for (slot, marker) in slots.iter().zip(&markers) {
        assert_eq!(
            cfg.gesture(*slot),
            *marker,
            "{slot:?} shares a field with another slot"
        );
    }
}

/// A key half is stored under the slot's full name and is ABSENT when unset — so a file that never
/// used one carries no table, and an older build reading a file that did keeps everything else.
///
/// Plausible breakage: storing `none` instead of removing, which fills the file with forty-six
/// lines nobody set; or a name spelling that `for_name` does not read back, which loses the gesture
/// on the next launch.
#[test]
fn a_key_half_is_stored_by_name_and_absent_when_unset() {
    let mut cfg = HotkeysConfig::default();
    assert!(
        !toml::to_string(&cfg).unwrap().contains("action_clicks"),
        "an unused table is not written"
    );
    let slot = GestureSlot::ForKey(KeySlot::OrderSize(2));
    assert!(cfg.set_gesture(slot, MouseGestureBinding::MiddleAlt));
    assert!(
        !cfg.set_gesture(slot, MouseGestureBinding::MiddleAlt),
        "no change reports none"
    );
    let text = toml::to_string(&cfg).unwrap();
    assert!(text.contains("[action_clicks]"), "{text}");
    assert!(text.contains("\"order_size.2\" = \"middle-alt\""), "{text}");

    let back: HotkeysConfig = toml::from_str(&text).unwrap();
    assert_eq!(back.gesture(slot), MouseGestureBinding::MiddleAlt);
    assert_eq!(
        KeySlot::for_name("order_size.2"),
        Some(KeySlot::OrderSize(2))
    );
    assert_eq!(KeySlot::for_name("order_size.9"), None);
    assert_eq!(KeySlot::for_name("cancel_buy"), Some(KeySlot::CancelBuy));

    assert!(cfg.set_gesture(slot, MouseGestureBinding::None));
    assert!(!cfg.set_gesture(slot, MouseGestureBinding::None));
    assert!(cfg.action_clicks.is_empty(), "unset is removed, not stored");

    // A name this build does not know survives a load-and-save untouched and names no slot.
    let foreign: HotkeysConfig =
        toml::from_str("[action_clicks]\nfrom_the_future = \"middle\"\n").unwrap();
    assert_eq!(foreign.action_clicks.len(), 1);
    assert!(
        toml::to_string(&foreign)
            .unwrap()
            .contains("from_the_future")
    );
    assert!(
        GestureSlot::all()
            .iter()
            .all(|slot| !matches!(slot, GestureSlot::ForKey(k) if k.name() == "from_the_future"))
    );
}

/// The two slots with no mouse half are exactly the two the design names, and every other key
/// slot appears in `all()` once, after the thirteen with a field of their own.
#[test]
fn every_key_slot_but_two_has_a_mouse_half_once() {
    let all = GestureSlot::all();
    assert_eq!(&all[..GestureSlot::OWN.len()], &GestureSlot::OWN[..]);
    let halves: Vec<KeySlot> = all
        .iter()
        .filter_map(|slot| match slot {
            GestureSlot::ForKey(key) => Some(*key),
            _ => None,
        })
        .collect();
    let without: Vec<KeySlot> = KeySlot::all()
        .into_iter()
        .filter(|key| !halves.contains(key))
        .collect();
    assert_eq!(without, [KeySlot::SellsToRect, KeySlot::FigDelete]);
    let mut seen = halves.clone();
    seen.sort_by_key(|k| k.name());
    seen.dedup();
    assert_eq!(seen.len(), halves.len(), "a key half is listed twice");
}

/// Each move row owns one kind field, and the row's two halves are two different gesture slots.
#[test]
fn each_move_row_addresses_its_own_kind_and_halves() {
    let mut cfg = HotkeysConfig::default();
    let kinds = [
        MoveKind::TopVolume,
        MoveKind::LowVolume,
        MoveKind::TopProfit,
        MoveKind::LastSet,
    ];
    for (row, kind) in MoveKindSlot::ALL.into_iter().zip(kinds) {
        assert!(cfg.set_move_kind(row, kind), "{row:?} stores nothing");
        assert!(!cfg.set_move_kind(row, kind), "rewriting the same kind");
    }
    for (row, kind) in MoveKindSlot::ALL.into_iter().zip(kinds) {
        assert_eq!(cfg.move_kind(row), kind, "{row:?} shares a kind field");
    }

    let mut halves: Vec<GestureSlot> = MoveKindSlot::ALL
        .into_iter()
        .flat_map(|row| [row.half(false), row.half(true)])
        .collect();
    halves.sort_by_key(|slot| slot.stem());
    halves.dedup();
    assert_eq!(halves.len(), 8, "two move halves name one gesture slot");
    for row in MoveKindSlot::ALL {
        for short in [false, true] {
            let slot = row.half(short);
            assert_eq!(slot.move_half(), Some(MoveHalf { row, short }), "{slot:?}");
        }
        assert_eq!(row.half(false).kind(), Some(row));
        assert_eq!(
            row.half(true).kind(),
            None,
            "a short row carries no kind of its own"
        );
        assert_eq!(row.half(false).short_twin(), Some(row.half(true)));
        assert_eq!(row.half(true).short_twin(), None);
    }
    for slot in [
        GestureSlot::BuySet,
        GestureSlot::PendingShort,
        GestureSlot::FigDelete,
    ] {
        assert_eq!(slot.move_half(), None, "{slot:?} is not a move row");
        assert_eq!(slot.kind(), None);
    }
}

/// The gesture in effect for a short row follows the long row while the mirror is set, and is
/// the row's own field once it is not — the same reading `move_gestures` gives the dispatcher.
///
/// Plausible breakage: a reader that goes to the field, which puts a binding nothing fires into
/// a preview's "current" column or into every other row's clash caption.
#[test]
fn the_gesture_in_effect_reads_through_the_mirror() {
    let mut cfg = HotkeysConfig::default();
    cfg.set_gesture(GestureSlot::BuyMove, MouseGestureBinding::LeftAlt);
    cfg.set_gesture(GestureSlot::ShortBuyMove, MouseGestureBinding::MiddleAlt);

    cfg.same_hotkeys_for_move = true;
    assert_eq!(
        cfg.gesture_in_effect(GestureSlot::ShortBuyMove),
        MouseGestureBinding::LeftAlt,
        "with the mirror set the short row fires the long row's gesture"
    );
    assert_eq!(
        cfg.gesture(GestureSlot::ShortBuyMove),
        MouseGestureBinding::MiddleAlt,
        "the stale short value is still stored"
    );

    cfg.same_hotkeys_for_move = false;
    assert_eq!(
        cfg.gesture_in_effect(GestureSlot::ShortBuyMove),
        MouseGestureBinding::MiddleAlt
    );
    // A placement row has no mirror: field and effect are one.
    cfg.set_gesture(GestureSlot::BuySet, MouseGestureBinding::RightAlt);
    assert_eq!(
        cfg.gesture_in_effect(GestureSlot::BuySet),
        MouseGestureBinding::RightAlt
    );
}

/// No two slots address the same field.
///
/// Every value is written BEFORE any is read back, which is what makes the check work: asserting
/// right after each write lets an alias pass, because the later slot's own read still returns what
/// it just wrote while the earlier slot's value is already gone.
#[test]
fn each_slot_addresses_storage_of_its_own() {
    let mut cfg = HotkeysConfig::default();
    let slots = KeySlot::all();
    // Deliberately NOT `f1`, `f2`...: those are the order-size defaults, so writing one would be a
    // no-op and `set_key` would rightly answer false.
    let value = |n: usize| format!("zz-{n}");
    for (n, slot) in slots.iter().enumerate() {
        assert!(cfg.set_key(*slot, value(n)), "{slot:?} stores nothing");
    }
    for (n, slot) in slots.iter().enumerate() {
        assert_eq!(
            cfg.key(*slot),
            value(n),
            "{slot:?} shares a field with another slot"
        );
    }
}

/// Writing the value a slot already holds is not a change, and an index outside its family is
/// neither a change nor a panic.
///
/// Plausible breakage: bare indexing in the accessors, which is what the settings page's macro did
/// before this. The families are arrays, the page builds their indices in loops, and `hotkeys.toml`
/// is hand-editable.
#[test]
fn a_no_op_write_and_an_impossible_index_are_both_quiet() {
    let mut cfg = HotkeysConfig::default();
    let slot = KeySlot::CancelBuy;
    let held = cfg.key(slot).to_string();

    assert!(!cfg.set_key(slot, held), "rewriting the same value");
    assert!(
        cfg.set_key(slot, "ctrl-q".into()),
        "a real edit reports true"
    );

    for out_of_range in [
        KeySlot::OrderSize(ORDER_SIZE_KEYS),
        KeySlot::SellPreset(999),
        KeySlot::ManualStrategy(MANUAL_STRATEGY_KEYS),
    ] {
        assert_eq!(cfg.key(out_of_range), "");
        assert!(!cfg.set_key(out_of_range, "alt-1".into()));
    }
}

/// The arriving figure-delete gesture yields to a trading gesture the file already fires on
/// Middle — and keeps its default where Middle is free.
///
/// Plausible breakage: the yield reading the stored short field rather than the gesture in effect,
/// which would clear the default against a mirrored-away value nobody can press, or not yielding
/// at all, which puts figure deletion in front of an order move on every figure.
#[test]
fn the_arriving_figure_gesture_yields_to_a_trading_gesture_on_the_same_button() {
    // A file from before the gesture existed, and one an intermediate build stamped 4 with the
    // gesture already in it: both are judged once.
    for from in [3, 4] {
        let mut taken = HotkeysConfig::default();
        taken.schema = from;
        taken.buy_move_click = MouseGestureBinding::Middle;
        assert!(taken.fill_unbound_slots());
        assert_eq!(
            taken.fig_delete_click,
            MouseGestureBinding::None,
            "from {from}"
        );
        assert_eq!(
            taken.buy_move_click,
            MouseGestureBinding::Middle,
            "the user's own gesture is the one kept"
        );
    }

    let mut free = HotkeysConfig::default();
    free.schema = 3;
    assert!(free.fill_unbound_slots());
    assert_eq!(free.fig_delete_click, MouseGestureBinding::Middle);

    // A move row whose kind is None dispatches nothing, so Middle on it is free — the same reading
    // the settings page gives that row.
    let mut inert = HotkeysConfig::default();
    inert.schema = 3;
    inert.buy_move_click = MouseGestureBinding::Middle;
    inert.buy_move_kind = MoveKind::None;
    assert!(inert.fill_unbound_slots());
    assert_eq!(inert.fig_delete_click, MouseGestureBinding::Middle);

    // A short field left on Middle behind the mirror switch is not a gesture anyone can press.
    let mut mirrored = HotkeysConfig::default();
    mirrored.schema = 3;
    mirrored.same_hotkeys_for_move = true;
    mirrored.short_sell_move_click = MouseGestureBinding::Middle;
    assert!(mirrored.fill_unbound_slots());
    assert_eq!(mirrored.fig_delete_click, MouseGestureBinding::Middle);

    // A file already at this generation is not re-judged: a later deliberate choice stands.
    let mut chosen = HotkeysConfig::default();
    chosen.schema = SCHEMA;
    chosen.buy_move_click = MouseGestureBinding::Middle;
    assert!(!chosen.fill_unbound_slots());
    assert_eq!(chosen.fig_delete_click, MouseGestureBinding::Middle);
}

/// The two pending gestures are cleared ONCE, when they stop being inert.
///
/// They shipped saved but unsent, with the settings row saying so; a value set under that promise
/// is not a decision to place live pending orders, which is what the same value does now. Everything
/// else in the file is left exactly as it was.
///
/// Plausible breakage: clearing on every load takes the gesture back from someone who set it
/// deliberately AFTER the change, which is the same mistake the keyboard generations are gated
/// against.
#[test]
fn the_pending_gestures_are_cleared_once_when_they_go_live() {
    let mut old = HotkeysConfig {
        schema: 3,
        pending_long_click: MouseGestureBinding::Middle,
        pending_short_click: MouseGestureBinding::LeftAlt,
        // A neighbour that must survive untouched: this one always did something.
        buy_set_click: MouseGestureBinding::LeftDouble,
        ..HotkeysConfig::default()
    };

    assert!(old.fill_unbound_slots());
    assert_eq!(old.pending_long_click, MouseGestureBinding::None);
    assert_eq!(old.pending_short_click, MouseGestureBinding::None);
    assert_eq!(
        old.buy_set_click,
        MouseGestureBinding::LeftDouble,
        "only the two that changed meaning are touched"
    );

    // Set again on a file that has already been through it, and it stays.
    old.pending_long_click = MouseGestureBinding::MiddleShift;
    assert!(!old.fill_unbound_slots(), "the generation has already run");
    assert_eq!(old.pending_long_click, MouseGestureBinding::MiddleShift);
}

/// A press claims the click half bound to it, and two halves bound to one press resolve in the
/// page's order — never in the table's alphabetical one.
///
/// Plausible breakage: iterating the `BTreeMap` and taking the first hit, which would let
/// `cancel_all_buys` (alphabetically first) beat `cancel_buy` although the page lists and
/// captions it the other way round.
#[test]
fn a_press_claims_the_click_half_in_page_order() {
    let mut cfg = HotkeysConfig::default();
    let is = |wanted: MouseGestureBinding| move |g: MouseGestureBinding| g == wanted;
    assert_eq!(
        cfg.action_for_gesture(is(MouseGestureBinding::MiddleAlt)),
        None
    );

    cfg.set_gesture(
        GestureSlot::ForKey(KeySlot::CancelAllBuys),
        MouseGestureBinding::MiddleAlt,
    );
    cfg.set_gesture(
        GestureSlot::ForKey(KeySlot::CancelBuy),
        MouseGestureBinding::MiddleAlt,
    );
    cfg.set_gesture(
        GestureSlot::ForKey(KeySlot::PanicSell),
        MouseGestureBinding::RightAlt,
    );
    let all = KeySlot::all();
    let position = |slot| all.iter().position(|s| *s == slot).unwrap();
    assert!(position(KeySlot::CancelBuy) < position(KeySlot::CancelAllBuys));
    assert_eq!(
        cfg.action_for_gesture(is(MouseGestureBinding::MiddleAlt)),
        Some(KeySlot::CancelBuy),
        "the page's order, not the table's"
    );
    assert_eq!(
        cfg.action_for_gesture(is(MouseGestureBinding::RightAlt)),
        Some(KeySlot::PanicSell)
    );
    assert_eq!(
        cfg.action_for_gesture(is(MouseGestureBinding::LeftAlt)),
        None
    );
    // A name this build does not know is never a hit, whatever it is bound to — nor is a slot
    // that has no mouse half, however it got into the table.
    cfg.action_clicks
        .insert("from_the_future".into(), MouseGestureBinding::LeftAlt);
    cfg.action_clicks
        .insert("fig_delete".into(), MouseGestureBinding::LeftAlt);
    assert_eq!(
        cfg.action_for_gesture(is(MouseGestureBinding::LeftAlt)),
        None
    );
}
