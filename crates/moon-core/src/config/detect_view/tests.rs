use super::*;

/// EVERY constructor field survives a TOML round trip (the path used to read/write
/// detects_view.toml and for Copy/Paste): active size, per-size w/h/chart/rail,
/// and every flag in every slot.
#[test]
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
