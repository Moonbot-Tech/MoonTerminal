//! Contract tests for the Connections core-table column specification.
//!
//! `columns.rs:CONN_COLS` must keep `h-preset` at a 72px `MicroTrigger` basis. Changing it to a
//! 92px `Raw` width recreates the hand-guessed fix that lets a scaled dropdown disagree with its
//! header, shifting later headings away from the controls they describe.

use super::{
    CONN_INDENT_BORDER, CONN_INDENT_MARGIN, CONN_INDENT_PAD, CONN_TABLE_INSET, ConnColId,
    ConnColWidth, MicroTriggerMetrics,
};

/// `ConnColId::ALL` is the one left-to-right geometry order; a reordered or mis-discriminated
/// variant would make `spec()` select another column's layout and place a heading over the wrong
/// server-row control.
#[test]
fn column_ids_are_the_complete_spec_indices() {
    assert_eq!(ConnColId::ALL.len(), 13);

    for (index, column) in ConnColId::ALL.into_iter().enumerate() {
        assert_eq!(column as usize, index, "{column:?} must index its own spec");
    }
}

/// A duplicate or omitted `ConnColId::ALL` variant would leave a server-row control without the
/// corresponding header geometry, making the Connections table silently drift again.
#[test]
fn every_column_id_appears_once_in_the_shared_order() {
    let mut seen = [false; ConnColId::ALL.len()];

    for column in ConnColId::ALL {
        let index = column as usize;
        assert!(!seen[index], "{column:?} appears more than once");
        seen[index] = true;
    }

    assert!(seen.into_iter().all(|present| present));
}

/// The indented row's tree guide must consume the same inset as the header; changing one part
/// would shift grouped server rows left or right of their column headings.
#[test]
fn indent_parts_match_the_header_inset() {
    assert_eq!(
        CONN_INDENT_MARGIN + CONN_INDENT_BORDER + CONN_INDENT_PAD,
        CONN_TABLE_INSET
    );
}

/// `columns.rs:CONN_COLS` must preserve each column's declared scaling policy. Replacing
/// any of `h-name`, `h-key` or `h-group`'s `TextScaled` policies with `Raw`, or `h-preset`'s
/// `MicroTrigger` policy with `Raw`, makes the header and server row disagree with their controls
/// at non-default font scales.
#[test]
fn widths_follow_the_frozen_per_column_policy() {
    const MICRO_COLUMNS: [ConnColId; 3] = [ConnColId::Proto, ConnColId::Preset, ConnColId::Data];
    const TEXT_SCALED_COLUMNS: [ConnColId; 3] = [ConnColId::Name, ConnColId::Key, ConnColId::Group];
    const SCALES: [f32; 3] = [0.75, 1.0, 1.3];

    for column in ConnColId::ALL {
        let basis = column.spec().basis;
        let is_micro = MICRO_COLUMNS.contains(&column);
        let is_text_scaled = TEXT_SCALED_COLUMNS.contains(&column);
        let expected_policy = if is_micro {
            ConnColWidth::MicroTrigger
        } else if is_text_scaled {
            ConnColWidth::TextScaled
        } else {
            ConnColWidth::Raw
        };
        for scale in SCALES {
            let expected_width = match expected_policy {
                ConnColWidth::Raw => basis,
                ConnColWidth::MicroTrigger | ConnColWidth::TextScaled => basis * scale,
            };
            assert_eq!(
                column.width(MicroTriggerMetrics {
                    scale,
                    min_width: 0.0,
                }),
                expected_width,
                "{column:?} at scale {scale}"
            );
        }

        assert_eq!(
            column.spec().width,
            expected_policy,
            "{column:?} width policy"
        );
    }
}

/// `columns.rs:CONN_COLS` must let Name, Key and Group absorb spare width without making any of
/// them rigid at the default window size. Removing growth from one truncates user-entered text;
/// every visible header label also needs help text.
#[test]
fn growth_and_tooltips_match_the_text_column_contract() {
    let growing: Vec<_> = ConnColId::ALL
        .into_iter()
        .filter(|column| column.spec().grow)
        .collect();
    assert_eq!(growing, [ConnColId::Name, ConnColId::Key, ConnColId::Group]);

    for column in ConnColId::ALL {
        let spec = column.spec();
        if spec.label.is_some() {
            assert!(
                spec.tip.is_some(),
                "{column:?} has a label without a tooltip"
            );
        }
    }
}

/// `columns.rs:CONN_COLS` must cap only Key and Group through their own policies. Removing either
/// cap lets that column consume wide-window space, while resolving either cap as raw pixels makes
/// its readable character count shrink when the user raises the Font setting.
#[test]
fn caps_match_the_text_column_contract_at_each_font_scale() {
    const MICRO_COLUMNS: [ConnColId; 3] = [ConnColId::Proto, ConnColId::Preset, ConnColId::Data];
    const TEXT_SCALED_COLUMNS: [ConnColId; 3] = [ConnColId::Name, ConnColId::Key, ConnColId::Group];
    const SCALES: [f32; 3] = [0.75, 1.0, 1.3];

    for column in ConnColId::ALL {
        let expected_cap = match column {
            ConnColId::Key => Some(260.0),
            ConnColId::Group => Some(140.0),
            _ => None,
        };
        let expected_policy = if MICRO_COLUMNS.contains(&column) {
            ConnColWidth::MicroTrigger
        } else if TEXT_SCALED_COLUMNS.contains(&column) {
            ConnColWidth::TextScaled
        } else {
            ConnColWidth::Raw
        };

        assert_eq!(column.spec().max, expected_cap, "{column:?} cap reference");
        for scale in SCALES {
            let metrics = MicroTriggerMetrics {
                scale,
                min_width: 0.0,
            };
            let expected_max = expected_cap.map(|cap| match expected_policy {
                ConnColWidth::Raw => cap,
                ConnColWidth::MicroTrigger | ConnColWidth::TextScaled => cap * scale,
            });
            assert_eq!(
                column.max_width(metrics),
                expected_max,
                "{column:?} cap at scale {scale}"
            );

            if let Some(expected_max) = expected_max {
                let expected_basis = column.width(metrics);
                assert!(
                    expected_max > expected_basis,
                    "{column:?} cap must exceed its basis at scale {scale}"
                );
            }
        }
    }
}

/// `columns.rs:CONN_COLS` must leave Name as the only uncapped growing column and resolve Name
/// wider than Key wider than Group at every finite Font scale. Changing `h-key` from `TextScaled`
/// to `Raw` makes Group outrank the masked Key at a hand-edited +10 Font delta, leaving less room
/// for the user-entered name.
#[test]
fn name_is_the_only_uncapped_growing_column_with_the_widest_narrow_width() {
    let uncapped_growing: Vec<_> = ConnColId::ALL
        .into_iter()
        .filter(|column| column.spec().grow && column.spec().max.is_none())
        .collect();
    assert_eq!(uncapped_growing, [ConnColId::Name]);

    const SCALES: [f32; 5] = [0.75, 1.0, 1.3, 1.6, 2.0];
    for scale in SCALES {
        let metrics = MicroTriggerMetrics {
            scale,
            min_width: 0.0,
        };
        assert!(
            ConnColId::Name.width(metrics) > ConnColId::Key.width(metrics)
                && ConnColId::Key.width(metrics) > ConnColId::Group.width(metrics),
            "Name, Key and Group must shrink in resolved-width order at scale {scale}"
        );
    }
}
