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

/// `columns.rs:CONN_COLS` must preserve the three Micro-trigger columns. Replacing `h-preset`'s
/// 72px `MicroTrigger` policy with a 92px `Raw` width makes the control stop scaling with its
/// rendered trigger and lets the header and server row disagree at non-default font scales.
#[test]
fn widths_follow_the_frozen_per_column_policy() {
    const MICRO_COLUMNS: [ConnColId; 3] = [ConnColId::Proto, ConnColId::Preset, ConnColId::Data];
    const SCALES: [f32; 3] = [0.75, 1.0, 1.3];

    for column in ConnColId::ALL {
        let basis = column.spec().basis;
        let is_micro = MICRO_COLUMNS.contains(&column);
        let expected_policy = if is_micro {
            ConnColWidth::MicroTrigger
        } else {
            ConnColWidth::Raw
        };
        for scale in SCALES {
            let expected_width = if is_micro { basis * scale } else { basis };
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

/// `columns.rs:CONN_COLS` must let only Name and Group absorb spare width: Key is excluded because
/// its masked content has no readable length to reward with width. Turning `h-key.grow` back on
/// wastes space on dots, while removing growth from Name or Group truncates user-entered text;
/// every visible header label also needs help text.
#[test]
fn growth_and_tooltips_match_the_text_column_contract() {
    let growing: Vec<_> = ConnColId::ALL
        .into_iter()
        .filter(|column| column.spec().grow)
        .collect();
    assert_eq!(growing, [ConnColId::Name, ConnColId::Group]);

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
