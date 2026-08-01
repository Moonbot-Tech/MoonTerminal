//! Report column-width persistence helpers.

use super::*;

/// Cached content-derived Report widths for one published data snapshot and font scale.
#[derive(Default)]
pub(super) struct NaturalWidthsCache {
    /// Base widths keyed by runtime column name.
    pub(super) widths: std::collections::HashMap<String, f32>,
    scale: f32,
}

impl NaturalWidthsCache {
    /// Invalidate widths after a new database result is published.
    ///
    /// Returns:
    ///     Nothing; the next refresh recomputes every width.
    pub(super) fn clear(&mut self) {
        self.widths.clear();
    }

    /// Refresh cached widths when data is new or the UI font scale changed.
    ///
    /// Args:
    ///     cols: Complete runtime Report schema.
    ///     rows: Current formatted-data source.
    ///     visible: Columns currently rendered; newly shown columns are measured lazily.
    ///     p: Active palette used by cell formatting.
    ///     cx: Application context used for text measurement and scale.
    ///
    /// Returns:
    ///     Nothing; `widths` holds the resolved bases.
    pub(super) fn refresh(
        &mut self,
        cols: &[String],
        rows: &[Vec<Value>],
        visible: &[usize],
        p: MoonPalette,
        cx: &App,
    ) {
        let scale = design::font_scale(cx);
        if (self.scale - scale).abs() >= f32::EPSILON {
            self.widths.clear();
            self.scale = scale;
        }
        let missing: Vec<usize> = visible
            .iter()
            .copied()
            .filter(|index| {
                cols.get(*index)
                    .is_some_and(|column| !self.widths.contains_key(column))
            })
            .collect();
        self.widths
            .extend(natural_widths(cols, rows, &missing, p, cx));
    }
}

/// Measure displayed Report content and return purpose-clamped base widths.
///
/// At most the visible query result is measured, and long free-text columns are capped so one
/// comment cannot consume the complete viewport. MoonDataTable's Preserve policy grows these
/// distinct bases proportionally when space is available and keeps horizontal overflow otherwise.
///
/// Args:
///     cols: Complete runtime Report schema.
///     rows: Current query rows in schema order.
///     visible: Source-column indices requiring measurement.
///     p: Active palette used by cell formatting.
///     cx: Application context used for text measurement.
///
/// Returns:
///     Content-derived base width for every runtime column.
fn natural_widths(
    cols: &[String],
    rows: &[Vec<Value>],
    visible: &[usize],
    p: MoonPalette,
    cx: &App,
) -> std::collections::HashMap<String, f32> {
    visible
        .iter()
        .filter_map(|&column_index| {
            let column = cols.get(column_index)?;
            let header = columns::header_for(column);
            let mut width = design::mono_body_text_width(cx, &header, FontWeight::SEMIBOLD.0);
            for row in rows.iter().take(query::MAX_REPORT_ROWS) {
                let value = row.get(column_index).unwrap_or(&Value::Null);
                let text = columns::cell(column, value, p).0;
                width = width.max(design::mono_body_text_width(
                    cx,
                    &text,
                    FontWeight::NORMAL.0,
                ));
            }
            let (floor, ceiling) = width_bounds(column);
            Some((column.clone(), (width + 28.0).ceil().clamp(floor, ceiling)))
        })
        .collect()
}

/// Return the readable floor and anti-domination ceiling for one Report column.
fn width_bounds(column: &str) -> (f32, f32) {
    match column {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => (116.0, 150.0),
        "comment" => (110.0, 360.0),
        "sellreason" => (120.0, 280.0),
        "channelname" | "signaltype" | "fname" | "exorderid" => (90.0, 240.0),
        "core_name" => (88.0, 260.0),
        "coin" => (72.0, 140.0),
        "profitbtc" | "profitpct" | "gainedbtc" | "spentbtc" => (78.0, 130.0),
        "lev" | "isshort" | "emulator" => (52.0, 90.0),
        _ => (68.0, 180.0),
    }
}

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

#[cfg(test)]
mod tests;
