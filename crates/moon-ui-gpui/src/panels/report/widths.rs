//! Report column-width persistence helpers.

use super::*;
use moon_core::db::ReportAxis;

/// Rendering identity for one content-derived Report width batch.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NaturalWidthsEnvironment {
    locale: String,
    font: design::MonoBodyFontSignature,
}

/// Build the exact rendering identity that owns one natural-width cache.
///
/// Args:
///     locale: Active translation locale used by Report headers and cells.
///     font: Resolved normal/semibold font identity and rendered size.
///
/// Returns:
///     Comparable cache environment.
fn natural_widths_environment(
    locale: &str,
    font: design::MonoBodyFontSignature,
) -> NaturalWidthsEnvironment {
    NaturalWidthsEnvironment {
        locale: locale.to_string(),
        font,
    }
}

/// Cached content-derived Report widths for one published data snapshot and rendering identity.
#[derive(Default)]
pub(super) struct NaturalWidthsCache {
    /// Base widths keyed by runtime column name.
    pub(super) widths: std::collections::HashMap<String, f32>,
    environment: Option<NaturalWidthsEnvironment>,
}

impl NaturalWidthsCache {
    /// Invalidate widths after a new database result is published.
    ///
    /// Returns:
    ///     Nothing; the next refresh recomputes every width.
    pub(super) fn clear(&mut self) {
        self.widths.clear();
    }

    /// Refresh cached widths when data, locale, or resolved Report typography changed.
    ///
    /// Args:
    ///     cols: Complete runtime Report schema.
    ///     rows: Current formatted-data source.
    ///     visible: Columns currently rendered; newly shown columns are measured lazily.
    ///     p: Active palette used by cell formatting.
    ///     axis: Time axis for replicated timestamp columns. Measurement MUST format
    ///         exactly as the renderer does, or a column is sized for text it never paints.
    ///     display_zone: User-selected zone for terminal-written timestamp columns.
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
        axis: &ReportAxis,
        display_zone: Tz,
        cx: &App,
    ) {
        let mut measurer = design::MonoBodyTextMeasurer::new(cx);
        let environment = natural_widths_environment(&rust_i18n::locale(), measurer.signature());
        if self.environment.as_ref() != Some(&environment) {
            self.widths.clear();
            self.environment = Some(environment);
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
            .extend(natural_widths(cols, rows, &missing, p, axis, display_zone, &mut measurer));
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
///     axis: Time axis for replicated timestamp columns, matching the renderer exactly.
///     display_zone: User-selected zone for terminal-written timestamp columns.
///     measurer: Exact per-refresh text measurer shared across every visible column.
///
/// Returns:
///     Content-derived base width for every runtime column.
fn natural_widths(
    cols: &[String],
    rows: &[Vec<Value>],
    visible: &[usize],
    p: MoonPalette,
    axis: &ReportAxis,
    display_zone: Tz,
    measurer: &mut design::MonoBodyTextMeasurer<'_>,
) -> std::collections::HashMap<String, f32> {
    visible
        .iter()
        .filter_map(|&column_index| {
            let column = cols.get(column_index)?;
            let header = columns::header_for(column);
            let mut width = measurer.text_width(&header, FontWeight::SEMIBOLD);
            // Generic cells use the same predicate in the renderer, so the two emphasized profit
            // columns cannot be measured light and then clip the wider glyphs they paint. Resolve it
            // once per column because it does not depend on row values.
            let weight = columns::cell_weight(column);
            for row in rows.iter().take(query::MAX_REPORT_ROWS) {
                let value = row.get(column_index).unwrap_or(&Value::Null);
                // Measured with the row's own quote, exactly as the renderer formats it: a
                // BTC-denominated row prints eight decimals, and measuring it at two would size the
                // column to clip them.
                let text = columns::cell(
                    column,
                    value,
                    columns::row_quote(cols, row),
                    p,
                    axis,
                    columns::row_core_uid(cols, row),
                    display_zone,
                )
                .0;
                width = width.max(measurer.text_width(&text, weight));
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
        "profitbtc" | "profitpct" | "gainedbtc" | "spentbtc" | "valuation_profit_usdt" => {
            (78.0, 130.0)
        }
        // A rate needs more room than a profit: eight significant digits, no thousands grouping.
        "valuation_rate" => (86.0, 150.0),
        "valuation_rate_source" => (100.0, 220.0),
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
