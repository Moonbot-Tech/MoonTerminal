//! Badges page: column geometry and the layout.

use std::rc::Rc;

use gpui::IntoElement;

use super::{
    BADGE_COLS, BADGE_GAP, BadgeCells, badge_outline_indent, badge_row_el, badge_table_w,
    badges_body, badges_table, badges_wrapped,
};
use crate::settings::general::probe_host::{host, stub};
use crate::settings::layout_probe::{ProbeSpec, body_box, sweep};

/// Catches the outline colours drifting off the colour column, and the expert ordinal column
/// adding a width other than its own.
#[test]
fn the_outline_line_starts_under_the_colour_column() {
    let c = &BADGE_COLS;
    let before = c.active + c.name + c.code + c.short + 4.0 * BADGE_GAP;
    assert_eq!(badge_outline_indent(false), before);
    assert_eq!(badge_outline_indent(true), before + c.ordinal + BADGE_GAP);
    assert_eq!(
        badge_table_w(true) - badge_table_w(false),
        c.ordinal + BADGE_GAP
    );
}

/// Cells of the controls' own widths, tagged per column for the probe.
fn cells(expert: bool) -> BadgeCells {
    let c = &BADGE_COLS;
    BadgeCells {
        ordinal: expert.then(|| stub("probe-ord", c.ordinal, 28.0)),
        active: stub("probe-active", c.active, 28.0),
        name: stub("probe-name", c.name, 28.0),
        code: stub("probe-code", c.code, 28.0),
        short: stub("probe-short", c.short, 28.0),
        color: stub("probe-color", c.color, 28.0),
        outline: stub("probe-outline", c.outline, 28.0),
        look: stub("probe-look", c.look, 20.0),
        delete: stub("probe-delete", c.delete, 28.0),
    }
}

/// At every zoom a body as wide as the scaled one-line table keeps one line and one pixel less
/// wraps; below 100% the colour picker's fixed trigger keeps its own width.
#[test]
fn rows_wrap_only_when_the_table_does_not_fit() {
    for zoom in [0.8_f32, 1.0, 1.25, 1.5] {
        for expert in [false, true] {
            let excess = (BADGE_COLS.color * (1.0 - zoom)).max(0.0);
            let fit = (badge_table_w(expert) + super::BADGE_BOX_PAD) * zoom + excess;
            assert!(
                !super::wraps_at(fit, expert, zoom),
                "zoom {zoom} expert {expert}"
            );
            assert!(
                super::wraps_at(fit - 1.0, expert, zoom),
                "zoom {zoom} expert {expert}"
            );
        }
    }
}

/// The Badges page with the expert disclosure open keeps every row, its last control and the
/// table inside the body — no sideways overflow — plus Add, the disclosure and the warning apart,
/// without cutting the hint or the warning, in every language at the default and the minimum
/// window. Rows carry outline colours, the widest shape.
#[gpui::test]
fn badges_page_fits_the_body(cx: &mut gpui::TestAppContext) {
    sweep(
        cx,
        |window, view, size| {
            host(move |_, cx| {
                let wrapped = badges_wrapped(body_box(size, cx).0, true, cx);
                let rows = (0..3)
                    .map(|idx| {
                        badge_row_el(
                            idx,
                            cells(true),
                            Some((
                                stub("probe-long", BADGE_COLS.color, 28.0),
                                stub("probe-shortc", BADGE_COLS.color, 28.0),
                            )),
                            wrapped,
                            cx,
                        )
                    })
                    .collect();
                badges_body(
                    badges_table(true, wrapped, rows, cx),
                    stub("probe-add", 120.0, 28.0),
                    true,
                    Rc::new(|_, _| {}),
                    cx,
                )
                .into_any_element()
            })(window, view, size)
        },
        |_| ProbeSpec {
            container: "probe-body",
            inside: [
                "badges-table",
                "badges-table-box",
                "badge-row-0",
                "badge-row-2",
                "probe-delete",
                "probe-ord",
                "probe-short",
                "probe-shortc",
                "badges-add",
                "badges-expert",
                "badges-expert-warn",
            ]
            .map(String::from)
            .to_vec(),
            no_overlap: vec![
                [
                    "badges-table",
                    "badges-add",
                    "badges-expert",
                    "badges-expert-warn",
                ]
                .map(String::from)
                .to_vec(),
            ],
            wraps: vec![(
                "badges-expert".into(),
                rust_i18n::t!("badges.expert.hint").to_string(),
            )],
            last: Some("badges-expert-warn".into()),
            ..Default::default()
        },
    );
}
