use super::*;

/// EVERY constructor field survives a TOML round trip (the path used to read/write
/// detects_view.toml and for Copy/Paste): active size, per-size w/h/chart/rail,
/// and every flag in every slot.
#[test]
// Nested slot writes cannot move into one literal; each field stays beside the round-trip it proves.
#[allow(clippy::field_reassign_with_default)]
fn detect_view_roundtrip_preserves_every_field() {
    let mut cfg = DetectViewCfg::default();
    cfg.size = DETECT_SIZE_LARGE;
    cfg.delta_decimals = 0;
    cfg.show_add_to_chart = true;
    cfg.mini.w = 77;
    cfg.mini.h = 33;
    cfg.mini.chart = DetectChart::Line;
    cfg.mini.rail_w = 5;
    cfg.mini.rail_grad = 61;
    cfg.medium.chart = DetectChart::None;
    for (i, slot) in cfg.large.slots.iter_mut().enumerate() {
        slot.field = DetectField::ALL[i % DetectField::ALL.len()];
        slot.over = i % 2 == 0;
        slot.right = i % 3 == 0;
        slot.below = i % 4 == 0;
    }

    // Shared format (Copy/Paste = one group).
    let text = cfg.to_share_string().expect("serialize");
    let back = DetectViewCfg::parse_share(&text).expect("parse");
    assert_eq!(cfg, back);

    // Entire file (including an empty group name, which is the window's default group).
    let mut file = DetectViewFile::default();
    file.set_group("", cfg);
    file.set_group("Группа 2", DetectViewCfg::default());
    let text = toml::to_string_pretty(&file).expect("serialize file");
    let back: DetectViewFile = toml::from_str(&text).expect("parse file");
    assert_eq!(back.group(""), cfg);
    assert_eq!(back.group("Группа 2"), DetectViewCfg::default());
    // Unknown group → default.
    assert_eq!(back.group("нет такой"), DetectViewCfg::default());
}

/// A partial entry (old/foreign file without new fields) is completed with defaults.
#[test]
fn detect_view_partial_toml_fills_defaults() {
    let cfg: DetectViewCfg =
        toml::from_str("size = 2\n[medium]\nw = 150\n").expect("partial parse");
    assert_eq!(cfg.size, DETECT_SIZE_LARGE);
    assert_eq!(cfg.medium.w, 150);
    // A file written before the setting existed keeps the historical feed: chart-routed detects
    // stay out of it until the operator asks for them.
    assert!(!cfg.show_add_to_chart);
    // Everything else comes from the defaults.
    assert_eq!(cfg.mini, DetectViewCfg::default().mini);
    assert_eq!(cfg.medium.h, DetectViewCfg::default().medium.h);
}

/// EVERY assignable field survives the share round trip, independent of how many slots a size has.
///
/// The round-trip test above walks slots, not variants, so it can only ever cover as many fields
/// as the largest size has slots — nine against ten fields today, and the gap grows with each
/// field added. A field that fails to serialize would silently read back as `None`, emptying the
/// slot the user configured.
#[test]
fn detect_view_roundtrip_preserves_every_assignable_field() {
    for field in DetectField::ALL {
        let mut cfg = DetectViewCfg::default();
        cfg.large.slots[0].field = field;
        let text = cfg.to_share_string().expect("serialize");
        let back = DetectViewCfg::parse_share(&text).expect("parse");
        assert_eq!(
            back.large.slots[0].field, field,
            "{field:?} lost in transit"
        );
    }
}

/// Mini is a 2×2 grid, medium a 2×3, and large a 3×3. Anything past large keeps the large grid.
///
/// Breakage: handing medium four slots, or an unknown size the mini grid, draws the wrong number
/// of fields on the card. The products are the grids named on `detect_slot_count`, not its arms.
#[test]
fn slot_count_follows_the_card_grid() {
    assert_eq!(detect_slot_count(DETECT_SIZE_MINI), 2 * 2);
    assert_eq!(detect_slot_count(DETECT_SIZE_MEDIUM), 2 * 3);
    assert_eq!(detect_slot_count(DETECT_SIZE_LARGE), 3 * 3);
    assert_eq!(detect_slot_count(DETECT_SIZE_LARGE + 1), 3 * 3);
}

/// The colored server rail is at most five pixels. Zero stays off; one past the cap does not grow.
///
/// Breakage: dropping the cap, or stopping a pixel early, paints the server color across the coin.
#[test]
fn rail_width_clamps_to_five_pixels() {
    let mut card = DetectSizeCfg {
        rail_w: 0,
        ..Default::default()
    };
    assert_eq!(card.rail_w_clamped(), 0);
    card.rail_w = 5;
    assert_eq!(card.rail_w_clamped(), 5);
    card.rail_w = 6;
    assert_eq!(card.rail_w_clamped(), 5);
}

/// The rail's gradient fade stops at the card's own width.
///
/// Breakage: forgetting the card width lets a stored gradient paint past the card.
#[test]
fn rail_gradient_clamps_to_the_card_width() {
    let mut card = DetectSizeCfg {
        w: 100,
        rail_grad: 0,
        ..Default::default()
    };
    assert_eq!(card.rail_grad_clamped(), 0);
    card.rail_grad = 100;
    assert_eq!(card.rail_grad_clamped(), 100);
    card.rail_grad = 101;
    assert_eq!(card.rail_grad_clamped(), 100);
}

/// The active size is mini, medium, or large. One past large stays large.
///
/// Breakage: clamping only as far as medium makes a stored large read as medium, and dropping the
/// clamp lets a corrupt size through to the card picker.
#[test]
fn card_size_clamps_to_large() {
    let mut cfg = DetectViewCfg {
        size: DETECT_SIZE_MINI,
        ..Default::default()
    };
    assert_eq!(cfg.size_clamped(), DETECT_SIZE_MINI);
    cfg.size = DETECT_SIZE_MEDIUM;
    assert_eq!(cfg.size_clamped(), DETECT_SIZE_MEDIUM);
    cfg.size = DETECT_SIZE_LARGE;
    assert_eq!(cfg.size_clamped(), DETECT_SIZE_LARGE);
    cfg.size = DETECT_SIZE_LARGE + 1;
    assert_eq!(cfg.size_clamped(), DETECT_SIZE_LARGE);
}

/// Delta figures show at most two decimal places. Zero stays a whole number.
///
/// Breakage: a cap of one hides a second place the user stored, and no cap prints a third.
#[test]
fn delta_decimals_clamp_to_two_places() {
    let mut cfg = DetectViewCfg {
        delta_decimals: 0,
        ..Default::default()
    };
    assert_eq!(cfg.delta_decimals_clamped(), 0);
    cfg.delta_decimals = 2;
    assert_eq!(cfg.delta_decimals_clamped(), 2);
    cfg.delta_decimals = 3;
    assert_eq!(cfg.delta_decimals_clamped(), 2);
}

/// Chart-routed detects join the feed only for the group that asked, including the unnamed default
/// group. A neighbour's flag, and a group that was never stored, stay out.
///
/// Breakage: reading any group's flag, or treating the empty name as "missing", either hides the
/// window-default group's detects or shows another group's.
#[test]
fn add_to_chart_follows_only_the_named_group() {
    let empty = DetectViewFile::default();
    assert!(!empty.shows_add_to_chart(""));
    assert!(!empty.shows_add_to_chart("alpha"));

    let opted_in = DetectViewCfg {
        show_add_to_chart: true,
        ..Default::default()
    };
    let mut file = DetectViewFile::default();
    file.set_group("alpha", opted_in);
    assert!(file.shows_add_to_chart("alpha"));
    assert!(!file.shows_add_to_chart("beta"));
    assert!(!file.shows_add_to_chart(""));

    let window_default = DetectViewCfg {
        show_add_to_chart: true,
        ..Default::default()
    };
    file.set_group("", window_default);
    assert!(file.shows_add_to_chart(""));
    assert!(!file.shows_add_to_chart("beta"));
}
