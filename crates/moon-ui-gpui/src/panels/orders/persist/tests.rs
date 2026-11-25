//! Restoring the Orders view from a saved `docks.json` entry.

use moon_ui::PanelInfo;

use super::view_from_info;
use crate::panels::orders::view::{OrdCol, OrderKind, OrderOrigin, PrimarySort};

/// An Orders entry exactly as a layout saved before the origin filter existed writes it: the
/// `PanelInfo` shape of a real `docks.json`, field for field, with no `origin` key. `primary` (3,
/// ProfitFirst) and `kind` (2, Emulator) are deliberately NOT the defaults, so the test below can
/// tell a restored value from an untouched default.
const PRE_ORIGIN_LAYOUT: &str = r#"{"panel": {"group": "Main", "primary": 3, "kind": 2,
    "newest_first": true, "only_current": false, "coin": "", "main_on_top": 2,
    "columns": ["core", "side", "token", "fill", "pnl", "pnl.pct", "sl", "ts", "strat"],
    "column_order": []}}"#;

/// Parse a stored entry through the same serde shape the dock loader reads.
fn stored(json: &str) -> PanelInfo {
    serde_json::from_str(json).expect("a stored Orders entry must parse as PanelInfo")
}

/// An upgrade must change nothing for an existing layout: no `origin` key reads as All, and the
/// filters the old layout did store still come back.
#[test]
fn a_layout_saved_before_the_origin_filter_shows_every_origin() {
    let view = view_from_info(&stored(PRE_ORIGIN_LAYOUT));

    assert_eq!(view.origin, OrderOrigin::All);
    assert!(
        view.kind == OrderKind::Emu,
        "the stored kind still restores"
    );
    assert!(
        view.primary == PrimarySort::ProfitFirst,
        "the stored sort still restores"
    );
    assert!(view.shows(OrdCol::Strat) && !view.shows(OrdCol::Buy));
}

/// A stored choice survives a restart. The codes are literals on purpose: they are what layouts
/// already on disk hold, so renumbering the enum must fail here rather than flip saved filters.
#[test]
fn a_stored_origin_restores() {
    let with = |code: u64| {
        let mut info: serde_json::Value = serde_json::from_str(PRE_ORIGIN_LAYOUT).unwrap();
        info["panel"]["origin"] = code.into();
        view_from_info(&serde_json::from_value(info).unwrap()).origin
    };

    assert_eq!(with(1), OrderOrigin::Auto);
    assert_eq!(with(2), OrderOrigin::Manual);
}

/// A code past `u8` must not wrap into a real choice: 258 truncated to a byte is 2, Manual, which
/// would hide every strategy order behind a filter the user never picked.
#[test]
fn an_out_of_range_origin_code_reads_as_all() {
    let mut info: serde_json::Value = serde_json::from_str(PRE_ORIGIN_LAYOUT).unwrap();
    info["panel"]["origin"] = 258u64.into();

    assert_eq!(
        view_from_info(&serde_json::from_value(info).unwrap()).origin,
        OrderOrigin::All
    );
}
