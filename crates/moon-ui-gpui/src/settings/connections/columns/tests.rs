//! The indent parts of the core tree.

use super::{CONN_INDENT_BORDER, CONN_INDENT_MARGIN, CONN_INDENT_PAD, CONN_TABLE_INSET};

/// The three indent parts sum to the header inset; a drift would shift indented core rows away
/// from their group header.
#[test]
fn indent_parts_match_the_header_inset() {
    assert_eq!(
        CONN_INDENT_MARGIN + CONN_INDENT_BORDER + CONN_INDENT_PAD,
        CONN_TABLE_INSET
    );
}
