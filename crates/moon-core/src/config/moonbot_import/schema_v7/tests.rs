use super::build::*;
use super::*;

#[test]
fn parses_full_payload() {
    let cfg = parse_payload(&full_payload(&[])).unwrap();
    assert_eq!(cfg.config_version, 1234);
    let h = &cfg.ui.hotkeys;
    assert_eq!(
        h.order_sizes,
        [100.0, 500.0, 1000.0, 5000.0, 10000.0, 50000.0]
    );
    assert_eq!(h.order_size_sel, 2);
    assert_eq!(h.order_size_keys, [0x70, 0x71, 0x72, 0x73, 0x74, 0x75]);
    assert_eq!(h.fixed_sell_sel, 1);
    assert_eq!(h.fixed_sell_prices, [1.0, 5.0, 10.0, 25.0, 50.0, 100.0]);
    assert_eq!(h.shortcuts.get(ShortcutAction::CancelBuy), 0);
    assert_ne!(h.shortcuts.get(ShortcutAction::PanicSell), 0);
    assert!(cfg.theme.is_dark());
    assert_eq!(
        cfg.theme.colors_light().unwrap().entries[0],
        ("graphBK".to_string(), "16777215".to_string())
    );
    assert_eq!(
        cfg.theme.colors_dark().unwrap().entries[0],
        ("graphBK".to_string(), "1973790".to_string())
    );
    assert_eq!(
        cfg.ini.section("Charts").unwrap().entries[0],
        ("CandleGreen".to_string(), "65280".to_string())
    );
    assert!(cfg.ini.section("ArbColors").unwrap().entries.is_empty());
    assert_eq!(cfg.ui.strat_editor_chapters, "chapters");
    assert_eq!(cfg.ui.markets_table.sort_col, 3);
    assert!(cfg.ui.markets_table.col_visible[0]);
    assert!(!cfg.ui.markets_table.col_visible[1]);
}

#[test]
fn ui_appended_tail_is_allowed() {
    // Append-only extension of the same version: extra bytes in the UI-block tail are valid.
    let cfg = parse_payload(&full_payload(&[1, 2, 3, 4, 5])).unwrap();
    assert_eq!(cfg.ui.main_buttons_index, 4);
}

#[test]
fn unknown_block_is_skipped() {
    let mut p = full_payload(&[]);
    block(&mut p, 42, &[0xCC; 10]); // Unknown kind at the end.
    assert!(parse_payload(&p).is_ok());
}

#[test]
fn duplicate_known_block_rejected() {
    let mut p = full_payload(&[]);
    block(&mut p, 4, &theme_body());
    assert!(matches!(
        parse_payload(&p),
        Err(ImportError::BadValue(ref s)) if s.contains("дважды")
    ));
}

#[test]
fn missing_block_rejected() {
    // Build a payload without block 5 (Ini).
    let mut p = Vec::new();
    p.extend_from_slice(b"MBSP");
    p.push(7u8);
    p.extend_from_slice(&1u16.to_le_bytes());
    block(&mut p, 1, &[]);
    block(&mut p, 2, &[]);
    block(&mut p, 3, &[]);
    block(&mut p, 4, &theme_body());
    block(&mut p, 6, &ui_body([0.0; 6], [0; 6], [0; 6], [0.0; 6]));
    assert!(matches!(
        parse_payload(&p),
        Err(ImportError::Truncated(ref s)) if s.contains("kind=5")
    ));
}

#[test]
fn block_overrun_rejected() {
    // The declared block size exceeds the remaining payload.
    let mut p = Vec::new();
    p.extend_from_slice(b"MBSP");
    p.push(7u8);
    p.extend_from_slice(&1u16.to_le_bytes());
    p.push(1u8);
    p.extend_from_slice(&100u32.to_le_bytes()); // Declares 100 bytes but contains 2.
    p.extend_from_slice(&[0, 0]);
    assert!(matches!(parse_payload(&p), Err(ImportError::Truncated(_))));
}

#[test]
fn bad_magic_and_newer_version() {
    assert!(matches!(
        parse_payload(b"XXXX\x07\x00\x00"),
        Err(ImportError::BadHeader(_))
    ));
    let mut p = Vec::new();
    p.extend_from_slice(b"MBSP");
    p.push(8u8); // Newer container version.
    p.extend_from_slice(&1u16.to_le_bytes());
    assert_eq!(
        parse_payload(&p),
        Err(ImportError::NewerFormat { found: 8 })
    );
}

#[test]
fn ui_version_mismatch_rejected() {
    let mut p = Vec::new();
    p.extend_from_slice(b"MBSP");
    p.push(7u8);
    p.extend_from_slice(&1u16.to_le_bytes());
    block(&mut p, 1, &[]);
    block(&mut p, 2, &[]);
    block(&mut p, 3, &[]);
    block(&mut p, 4, &theme_body());
    block(&mut p, 5, &ini_body());
    let mut ui = ui_body([0.0; 6], [0; 6], [0; 6], [0.0; 6]);
    ui[0] = 4; // Version 4 instead of 3.
    block(&mut p, 6, &ui);
    assert!(matches!(
        parse_payload(&p),
        Err(ImportError::BadValue(ref s)) if s.contains("UI.version")
    ));
}

#[test]
fn nan_order_size_rejected() {
    let mut sizes = [1.0f64; 6];
    sizes[3] = f64::NAN;
    let mut p = Vec::new();
    p.extend_from_slice(b"MBSP");
    p.push(7u8);
    p.extend_from_slice(&1u16.to_le_bytes());
    block(&mut p, 1, &[]);
    block(&mut p, 2, &[]);
    block(&mut p, 3, &[]);
    block(&mut p, 4, &theme_body());
    block(&mut p, 5, &ini_body());
    block(&mut p, 6, &ui_body(sizes, [0; 6], [0; 6], [0.0; 6]));
    assert!(matches!(
        parse_payload(&p),
        Err(ImportError::BadValue(ref s)) if s.contains("OSize")
    ));
}

#[test]
fn full_clipboard_roundtrip_through_transport() {
    // Transport/schema integration: encode → parse_clipboard.
    let text = super::super::transport::encode_mbsc7(&full_payload(&[]));
    let cfg = super::super::parse_clipboard(&text).unwrap();
    assert_eq!(cfg.ui.hotkeys.order_sizes[5], 50000.0);
}
