//! Order lines page: the table's mapping onto `OrdersStyle`, and the layout.

use std::collections::HashSet;
use std::rc::Rc;

use moon_core::config::OrdersStyle;

use super::{
    LINE_KINDS, LINES_ALL_ROWS, LinesParts, line_card, line_style, line_style_mut, lines_body,
    lines_table,
};
use crate::settings::general::probe_host::{all_open, host, stub};
use crate::settings::layout_probe::{ProbeSpec, sweep};

/// Catches two rows editing one line, or a row editing a different line than it names: the ten
/// rows carry ten distinct `orders.toml` keys, and a write through row N lands on exactly the
/// field that key names.
#[test]
fn ten_rows_map_to_ten_distinct_orders_keys() {
    let keys: HashSet<_> = LINE_KINDS.iter().map(|kind| kind.key).collect();
    assert_eq!(keys.len(), 10, "every row needs its own orders key");

    let mut style = OrdersStyle::default();
    for idx in 0..LINE_KINDS.len() {
        line_style_mut(&mut style, idx).thickness = 100.0 + idx as f32;
    }
    let json = serde_json::to_value(&style).expect("OrdersStyle serializes");
    for (idx, kind) in LINE_KINDS.iter().enumerate() {
        let thickness = json[kind.key]["thickness"].as_f64();
        assert_eq!(
            thickness,
            Some(100.0 + idx as f64),
            "row {idx} ({}) must write `{}`",
            kind.name,
            kind.key
        );
        assert_eq!(line_style(&style, idx).thickness, 100.0 + idx as f32);
    }
    // Only the continuous liquidation line has no crosses; only entries have a not-filled look.
    assert_eq!(
        LINE_KINDS
            .iter()
            .filter(|kind| !kind.markers)
            .map(|kind| kind.key)
            .collect::<Vec<_>>(),
        vec!["liq"]
    );
    assert_eq!(
        LINE_KINDS
            .iter()
            .filter(|kind| kind.pending)
            .map(|kind| kind.key)
            .collect::<Vec<_>>(),
        vec!["buy", "buy_short"]
    );
}

/// The Lines page keeps its table rows, the card and the shared box inside the body, every table
/// row apart, the card name and the unfolded path help uncut, in every language at the default
/// and the minimum window.
#[gpui::test]
fn lines_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        host(|_, cx| {
            let orders = OrdersStyle::default();
            let card = line_card(
                rust_i18n::t!(LINE_KINDS[0].name).to_string(),
                stub("probe-sample", 240.0, 12.0),
                vec![stub("probe-card-row", 240.0, 24.0)],
                cx,
            );
            let parts = LinesParts {
                set_note: rust_i18n::t!("iface.set_note.dark").to_string(),
                table: lines_table(&orders, 0, Rc::new(|_, _, _| {}), cx),
                card,
                active_alpha: stub("probe-active", 180.0, 22.0),
                closed_alpha: stub("probe-closed", 180.0, 22.0),
                max_closed: stub("probe-max", 130.0, 28.0),
                pending_dashed: stub("probe-pending", 18.0, 18.0),
                path: stub("probe-path", 240.0, 90.0),
            };
            lines_body(parts, &all_open, cx)
        }),
        |_| {
            let rows: Vec<String> = (0..LINE_KINDS.len())
                .map(|i| format!("lines-row-{i}"))
                .collect();
            let mut inside = rows.clone();
            inside.extend(LINES_ALL_ROWS.map(String::from));
            inside.extend(
                [
                    "lines-table",
                    "lines-card",
                    "probe-card-row",
                    "probe-path",
                    "lines-all",
                ]
                .map(String::from),
            );
            ProbeSpec {
                container: "probe-body",
                inside,
                no_overlap: vec![
                    rows,
                    ["lines-table", "lines-card", "lines-all"]
                        .map(String::from)
                        .to_vec(),
                    LINES_ALL_ROWS.map(String::from).to_vec(),
                ],
                wraps: vec![
                    (
                        "lines-path-more-text".into(),
                        rust_i18n::t!("lines.path_hint").to_string(),
                    ),
                    (
                        "lines-note".into(),
                        rust_i18n::t!("lines.note.after_entry").to_string(),
                    ),
                    (
                        "lines-card-name".into(),
                        rust_i18n::t!(LINE_KINDS[0].name).to_string(),
                    ),
                ],
                last: Some("lines-note".into()),
                ..Default::default()
            }
        },
    );
}
