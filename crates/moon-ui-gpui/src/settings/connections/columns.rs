//! The core tree's indent geometry, shared by the Storage page's core-data tree.
//!
//! The 14-column Connections table these once laid out is gone (the Cores page is a list and a
//! card); the indent parts stay because `storage/core_data.rs` draws the same group/core tree.

/// Left inset shared by the header row and by an indented core row.
///
/// An indented core row spends it as `ml(8) + pl(11) + border_l_1`; a group header spends it as
/// plain padding.
pub(in crate::settings) const CONN_TABLE_INSET: f32 = 20.0;

/// Left margin of an indented core row, the first part of [`CONN_TABLE_INSET`].
pub(in crate::settings) const CONN_INDENT_MARGIN: f32 = 8.0;

/// Border width of the branch guide drawn down an indented core row.
pub(super) const CONN_INDENT_BORDER: f32 = 1.0;

/// Left padding between the branch guide and an indented core row's first cell.
///
/// The three indent parts sum to [`CONN_TABLE_INSET`]; `tests` proves it.
pub(in crate::settings) const CONN_INDENT_PAD: f32 =
    CONN_TABLE_INSET - CONN_INDENT_MARGIN - CONN_INDENT_BORDER;

#[cfg(test)]
mod tests;
