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

/// What the natural-width pass adds to the widest measured text: `MoonDataTable`'s own cell
/// padding (12 left + 8 right — MoonUI `moon/table.rs`, `MoonTableColumn::new`) plus
/// [`NATURAL_SLACK`]. Logical pixels that no font or UI setting scales, unlike the text beside
/// them, which is why they are added to a measurement instead of being folded into a fixed width.
const NATURAL_PAD: f32 = 28.0;

/// The part of [`NATURAL_PAD`] that is breathing room rather than cell padding.
///
/// A column narrower than its natural width by no more than this still holds its widest text, so
/// this is also the tolerance [`NaturalWidthsCache::dates_without_room_for_seconds`] allows a
/// stored width: the first drag snapshots every column at its RENDERED width, which a fractional
/// display scale may round a pixel under the natural one.
const NATURAL_SLACK: f32 = 8.0;

/// Whole-string advances for one resolved font and size, bounded even for long comments.
#[derive(Default)]
struct TextWidths {
    normal: FontWidths,
    semibold: FontWidths,
}

/// Exact glyph advances and whole strings under one resolved weight and size.
struct FontWidths {
    ascii: [Option<f32>; 128],
    strings: std::collections::HashMap<String, f32>,
}

impl Default for FontWidths {
    /// A fresh typography environment has no remembered advances.
    fn default() -> Self {
        Self {
            ascii: [None; 128],
            strings: Default::default(),
        }
    }
}

impl TextWidths {
    /// Reuse the exact ordered glyph sum for repeated cell or header text.
    fn measure(
        &mut self,
        measurer: &mut design::MonoBodyTextMeasurer<'_>,
        text: &str,
        weight: FontWeight,
    ) -> f32 {
        let widths = if weight == FontWeight::NORMAL {
            &mut self.normal
        } else if weight == FontWeight::SEMIBOLD {
            &mut self.semibold
        } else {
            return measurer.text_width(text, weight);
        };
        if let Some(&width) = widths.strings.get(text) {
            return width;
        }
        let width = if text.is_ascii() {
            // Sum in the same glyph order as the general measurer. Most Report cells use ASCII,
            // and an array lookup saves a hash for every character of a unique coin or comment.
            text.bytes()
                .map(|byte| {
                    *widths.ascii[usize::from(byte)].get_or_insert_with(|| {
                        measurer.text_width(char::from(byte).encode_utf8(&mut [0; 4]), weight)
                    })
                })
                .sum()
        } else {
            measurer.text_width(text, weight)
        };
        if text.len() <= 128 && widths.strings.len() < 2048 {
            widths.strings.insert(text.to_owned(), width);
        }
        width
    }
}

/// Rendering identity for one content-derived Report width batch.
#[derive(Clone, Debug, Eq, PartialEq)]
struct NaturalWidthsEnvironment {
    locale: String,
    /// The spelling the volume cells were measured in: `12.3 k` is a character wider than `12.3K`,
    /// and widths only ever grow within one environment, so without it a respelled column would
    /// keep the other notation's width (#1390).
    notation: moon_core::util::fmt::Notation,
    font: design::MonoBodyFontSignature,
}

/// Build the exact rendering identity that owns one natural-width cache.
///
/// Args:
///     locale: Active translation locale used by Report headers and cells.
///     notation: Number notation the shortened cells are printed in.
///     font: Resolved normal/semibold font identity and rendered size.
///
/// Returns:
///     Comparable cache environment.
fn natural_widths_environment(
    locale: &str,
    notation: moon_core::util::fmt::Notation,
    font: design::MonoBodyFontSignature,
) -> NaturalWidthsEnvironment {
    NaturalWidthsEnvironment {
        locale: locale.to_string(),
        notation,
        font,
    }
}

/// Content-derived Report widths that grow across publications under one rendering identity.
#[derive(Default)]
pub(super) struct NaturalWidthsCache {
    /// Base widths keyed by runtime column name.
    pub(super) widths: std::collections::HashMap<String, f32>,
    environment: Option<NaturalWidthsEnvironment>,
    text_widths: TextWidths,
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

    /// Second-resolution date columns too narrow, at their stored width, to print their seconds.
    ///
    /// Only a column with a STORED width can be. The Report table runs
    /// `MoonDataTableWidthPolicy::Preserve`, which never renders a column without one below its
    /// natural width, and that width measured the full `…:SS` text under the live font with no
    /// ceiling (`width_bounds`). A stored width is what the table renders the column at exactly
    /// (MoonUI `column_layout::ordered_columns` marks every entry `user_sized`), so comparing that
    /// authoritative number with the natural one answers "do the seconds fit" at every font size,
    /// UI zoom and display scale — both sides are logical pixels of the same frame, and the text
    /// side was measured, not assumed.
    ///
    /// A stored width is not only a column the user dragged: the table's first drag snapshots
    /// EVERY column at its rendered width, `complete_widths` fills a column the saved map lacked
    /// with `columns::width_for`, and an older build saved these two under the minute form's
    /// 150 px ceiling. All of those are frozen until the divider is double-clicked, and #1391
    /// settles that case: the frozen column paints whole minutes and the hover keeps the seconds,
    /// rather than an ellipsis through them.
    ///
    /// The natural width also covers the header and the readable floor, so a column whose
    /// translated header is wider than its stamps falls back a little earlier than its text alone
    /// would need. That errs toward whole minutes and never toward an ellipsis through the seconds.
    ///
    /// Args:
    ///     user_widths: `MoonDataTableState::column_widths`, empty until the first resize.
    ///
    /// Returns:
    ///     The columns whose cells paint the minute form in this frame; the hover keeps the
    ///     seconds. Empty while the table has no stored widths at all.
    pub(super) fn dates_without_room_for_seconds(
        &self,
        user_widths: &std::collections::HashMap<String, f32>,
    ) -> Vec<&'static str> {
        columns::SECOND_DATE_COLUMNS
            .into_iter()
            .filter(|column| {
                match (user_widths.get(*column), self.widths.get(*column)) {
                    (Some(stored), Some(natural)) => *stored < natural - NATURAL_SLACK,
                    // No stored width: rendered at its natural width or wider. Not measured:
                    // hidden, so there is no cell to decide for.
                    _ => false,
                }
            })
            .collect()
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
        let environment = natural_widths_environment(
            &rust_i18n::locale(),
            moon_core::util::fmt::notation(),
            measurer.signature(),
        );
        if self.environment.as_ref() != Some(&environment) {
            self.clear();
            self.environment = Some(environment);
            self.text_widths = TextWidths::default();
        }
        if self.axis.as_ref() != Some(axis) || self.zone != Some(display_zone) {
            self.clear();
            self.axis = Some(axis.clone());
            self.zone = Some(display_zone);
        }
        let quote_index = cols.iter().position(|column| column == "basecurrency");
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
                    quote_index,
                    |text, weight| self.text_widths.measure(&mut measurer, text, weight),
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
///     quote_index: Column holding the row's effective quote currency, when present.
///     measure: Exact text advance under the current Report typography.
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
    quote_index: Option<usize>,
    mut measure: impl FnMut(&str, FontWeight) -> f32,
) -> std::collections::HashMap<String, f32> {
    visible
        .iter()
        .filter_map(|&column_index| {
            let column = cols.get(column_index)?;
            let (floor, ceiling) = width_bounds(column);
            let mut width = bases.get(column).map_or_else(
                || measure(&columns::header_label(column), FontWeight::SEMIBOLD),
                |base| base - NATURAL_PAD,
            );
            // Generic cells use the same predicate in the renderer, so the two emphasized profit
            // columns cannot be measured light and then clip the wider glyphs they paint. Resolve it
            // once per column because it does not depend on row values.
            let weight = columns::cell_weight(column);
            for &row_index in row_indices {
                // No remaining row can increase an already ceiling-clamped width.
                if (width + NATURAL_PAD).ceil() >= ceiling {
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
                    quote_index
                        .and_then(|index| row.get(index))
                        .and_then(moon_core::db::QuoteCurrency::from_report_value),
                    p,
                    axis,
                    core_uids.get(row_index).copied().unwrap_or(0),
                    display_zone,
                )
                .0;
                width = width.max(measure(&text, weight));
                // A date prints one fixed pattern of digits and separators in the monospaced
                // body font, so every non-empty stamp is exactly as wide as the first. This is the
                // early exit the 150 px ceiling used to give these columns for free; without it a
                // full page would format and measure every row of all four date columns.
                if fixed_width_date(column) && !text.is_empty() {
                    break;
                }
            }
            Some((
                column.clone(),
                (width + NATURAL_PAD).ceil().clamp(floor, ceiling),
            ))
        })
        .collect()
}

/// Whether every non-empty cell of a Report column renders text of one identical width.
///
/// True for the date columns: `columns::date_text` prints a fixed `%Y-%m-%d %H:%M[:%S]` pattern,
/// and the Report body font is monospaced, so a column's stamps differ only in which digits they
/// hold. Must name exactly the columns `columns::cell` formats through `date_text`.
fn fixed_width_date(column: &str) -> bool {
    matches!(
        column,
        "buydate" | "closedate" | "sellsetdate" | "last_update_at"
    )
}

/// Return the readable floor and anti-domination ceiling for one Report column.
///
/// The ceiling exists for free text, where one long comment could take the whole viewport. A date
/// has no such failure: its text is a fixed format a few characters long, so its measured width
/// IS its bound. A pixel ceiling there only clips, and it clips differently on every machine —
/// the text scales with the font slider and the theme's mono size while the ceiling does not, so
/// `150` already cut the old minute form at the default 14 px body and cut it harder at every
/// larger setting (#1391: `12:50:3…` is worse than no seconds at all).
fn width_bounds(column: &str) -> (f32, f32) {
    match column {
        "buydate" | "closedate" | "sellsetdate" | "last_update_at" => (116.0, f32::INFINITY),
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
