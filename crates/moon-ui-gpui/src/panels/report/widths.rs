//! Report column-width persistence helpers.

use super::*;

/// Complete a partially persisted width map with defaults for every current column.
///
/// Partial maps from older sessions or renamed columns caused the table engine to rescale untouched
/// neighbors on every drag. Completing a non-empty map lets later observations recognize a
/// single-column drag once the previous snapshot has the same membership. Leave an empty map
/// untouched so automatic fill remains active; the first drag then snapshots all widths and may use
/// the proportional overflow path.
pub(super) fn complete_widths(
    widths: &mut std::collections::HashMap<String, f32>,
    cols: &[String],
) {
    if widths.is_empty() {
        return;
    }
    for c in cols {
        widths
            .entry(c.clone())
            .or_insert_with(|| columns::width_for(c));
    }
}
