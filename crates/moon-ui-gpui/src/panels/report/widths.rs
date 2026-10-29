//! Report column-width persistence helpers.

use super::*;
use moon_core::db::ReportAxis;
use std::hash::{Hash, Hasher};

/// Exact row contents with an off-thread hash; equality still checks bytes on a hash collision.
#[derive(Eq, PartialEq)]
pub(super) struct RowIdentity {
    hash: u64,
    bytes: Vec<u8>,
}

impl Hash for RowIdentity {
    /// Reuse the background hash when publication reconciles the small visible page.
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

/// Encode exact width inputs on the reader thread, including each row's quote and core.
pub(super) fn row_identities(rows: &[Vec<Value>], cores: &[u64]) -> Vec<Arc<RowIdentity>> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let mut bytes = cores
                .get(index)
                .copied()
                .unwrap_or(0)
                .to_le_bytes()
                .to_vec();
            for value in row {
                match value {
                    Value::Null => bytes.push(0),
                    Value::Integer(value) => {
                        bytes.push(1);
                        bytes.extend(value.to_le_bytes());
                    }
                    Value::Real(value) => {
                        bytes.push(2);
                        bytes.extend(value.to_bits().to_le_bytes());
                    }
                    Value::Text(value) => {
                        bytes.push(3);
                        bytes.extend((value.len() as u64).to_le_bytes());
                        bytes.extend(value.as_bytes());
                    }
                    Value::Blob(value) => {
                        bytes.push(4);
                        bytes.extend((value.len() as u64).to_le_bytes());
                        bytes.extend(value);
                    }
                }
            }
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hash);
            Arc::new(RowIdentity {
                hash: hash.finish(),
                bytes,
            })
        })
        .collect()
}

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

/// Content-derived Report widths that grow across publications under one rendering identity.
#[derive(Default)]
pub(super) struct NaturalWidthsCache {
    /// Base widths keyed by runtime column name.
    pub(super) widths: std::collections::HashMap<String, f32>,
    environment: Option<NaturalWidthsEnvironment>,
    axis: Option<ReportAxis>,
    zone: Option<Tz>,
    identities: std::collections::HashMap<Arc<RowIdentity>, u64>,
    row_tokens: Vec<u64>,
    next_token: u64,
    revision: u64,
    measured: std::collections::HashMap<String, (u64, std::collections::HashSet<u64>)>,
}

impl NaturalWidthsCache {
    /// Invalidate all measurements after a formatting or schema change.
    ///
    /// Returns:
    ///     Nothing; the next refresh recomputes every width.
    pub(super) fn clear(&mut self) {
        self.widths.clear();
        self.measured.clear();
    }

    /// Reconcile exact background row identities without formatting unchanged rows on the UI thread.
    ///
    /// Args:
    ///     rows: Exact identities parallel to the newly published Report rows.
    ///     schema_changed: Whether column membership or order requires a full measurement.
    pub(super) fn publish(&mut self, rows: Vec<Arc<RowIdentity>>, schema_changed: bool) {
        if schema_changed {
            self.clear();
        }
        let previous_next = self.next_token;
        let mut identities = std::collections::HashMap::with_capacity(rows.len());
        self.row_tokens = rows
            .into_iter()
            .map(|row| {
                *identities.entry(row.clone()).or_insert_with(|| {
                    self.identities.get(&row).copied().unwrap_or_else(|| {
                        let token = self.next_token;
                        self.next_token = self.next_token.wrapping_add(1);
                        token
                    })
                })
            })
            .collect();
        let unchanged =
            self.next_token == previous_next && identities.len() == self.identities.len();
        self.identities = identities;
        if unchanged {
            return;
        }
        let current = self
            .row_tokens
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        for (_, seen) in self.measured.values_mut() {
            seen.retain(|token| current.contains(token));
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Refresh cached widths when data, locale, or resolved Report typography changed.
    ///
    /// Args:
    ///     cols: Complete runtime Report schema.
    ///     rows: Current formatted-data source.
    ///     core_uids: Each row's core, PARALLEL to `rows`. Measurement must correct a replicated
    ///         timestamp exactly as the renderer does, and `core_uid` is a service column the
    ///         report schema does not carry, so it has to travel beside the rows.
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
        core_uids: &[u64],
        visible: &[usize],
        p: MoonPalette,
        axis: &ReportAxis,
        display_zone: Tz,
        cx: &App,
    ) {
        let mut measurer = design::MonoBodyTextMeasurer::new(cx);
        let environment = natural_widths_environment(&rust_i18n::locale(), measurer.signature());
        if self.environment.as_ref() != Some(&environment) {
            self.clear();
            self.environment = Some(environment);
        }
        if self.axis.as_ref() != Some(axis) || self.zone != Some(display_zone) {
            self.clear();
            self.axis = Some(axis.clone());
            self.zone = Some(display_zone);
        }
        for &index in visible {
            let Some(column) = cols.get(index) else {
                continue;
            };
            if self
                .measured
                .get(column)
                .is_some_and(|(revision, _)| *revision == self.revision)
            {
                continue;
            }
            let (revision, seen) = self
                .measured
                .entry(column.clone())
                .or_insert_with(|| (self.revision.wrapping_sub(1), Default::default()));
            let changed = self
                .row_tokens
                .iter()
                .enumerate()
                .filter_map(|(row, token)| (!seen.contains(token)).then_some(row))
                .collect::<Vec<_>>();
            if !changed.is_empty() || !self.widths.contains_key(column) {
                for (column, width) in natural_widths(
                    cols,
                    rows,
                    core_uids,
                    &[index],
                    &changed,
                    &self.widths,
                    p,
                    axis,
                    display_zone,
                    &mut measurer,
                ) {
                    self.widths
                        .entry(column)
                        .and_modify(|previous| *previous = previous.max(width))
                        .or_insert(width);
                }
            }
            seen.extend(self.row_tokens.iter().copied());
            *revision = self.revision;
        }
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
///     core_uids: Each row's core, PARALLEL to `rows`.
///     visible: Source-column indices requiring measurement.
///     row_indices: New or changed rows whose formatted values have not yet been measured.
///     bases: Prior valid measurements; their headers need not be measured again.
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
    core_uids: &[u64],
    visible: &[usize],
    row_indices: &[usize],
    bases: &std::collections::HashMap<String, f32>,
    p: MoonPalette,
    axis: &ReportAxis,
    display_zone: Tz,
    measurer: &mut design::MonoBodyTextMeasurer<'_>,
) -> std::collections::HashMap<String, f32> {
    visible
        .iter()
        .filter_map(|&column_index| {
            let column = cols.get(column_index)?;
            let (floor, ceiling) = width_bounds(column);
            let mut width = bases.get(column).map_or_else(
                || measurer.text_width(&columns::header_label(column), FontWeight::SEMIBOLD),
                |base| base - 28.0,
            );
            // Generic cells use the same predicate in the renderer, so the two emphasized profit
            // columns cannot be measured light and then clip the wider glyphs they paint. Resolve it
            // once per column because it does not depend on row values.
            let weight = columns::cell_weight(column);
            for &row_index in row_indices {
                // No remaining row can increase an already ceiling-clamped width.
                if (width + 28.0).ceil() >= ceiling {
                    break;
                }
                let Some(row) = rows.get(row_index) else {
                    continue;
                };
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
                    core_uids.get(row_index).copied().unwrap_or(0),
                    display_zone,
                )
                .0;
                width = width.max(measurer.text_width(&text, weight));
            }
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
        "lev" | "emulator" => (52.0, 90.0),
        "isshort" => (60.0, 110.0),
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
