//! Turning one row's numbers into the exact text a cell draws, and deciding how wide that cell is.
//!
//! Split out of `table.rs`, which draws them: these are pure functions of a value, its unit and the
//! room the column was given, and the tests exercise them without a window.

use moon_core::db::ProfitUnit;
use moon_core::util::fmt::{self, DeltaSign};

/// Bracket characters one `total(last)` suffix wraps its amount in.
const SUFFIX_BRACKETS: usize = 2;
/// Space between an amount and the ticker naming its unit.
const TICKER_GAP: usize = 1;
/// Smallest rounded magnitude that lets a whole column use the abbreviated form.
///
/// Smaller columns keep their configured fixed decimals even when the exact spelling would not fit;
/// compact SI is reserved for genuinely large figures and, once selected, remains one form for the
/// whole column.
const SI_FLOOR: f64 = 100_000.0;

/// The form a whole profit column prints its values in.
///
/// One form for the WHOLE column, never per row: two rows abbreviated differently stop being
/// comparable at a glance, and comparing cores is the only reason this column exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProfitForm {
    /// Whether each cell carries its `total(last)` suffix.
    pub(super) suffix: bool,
    /// Whether each amount is followed by its currency ticker.
    pub(super) ticker: bool,
    /// Whether each amount may be abbreviated (`+1.23M`) instead of printed in full.
    pub(super) si: bool,
}

/// Character counts of a column's widest value, measured per component.
///
/// Per COMPONENT rather than per finished string: the column has six candidate forms, so measuring
/// each spelling once and adding brackets and ticker arithmetically costs at most two formats per
/// row instead of six — one when the amount is below [`SI_FLOOR`], where both spellings are the
/// same text. When the widest amount and the widest suffix sit on DIFFERENT rows the sum overstates
/// the real maximum by a character or two — the direction a money column must err in, because the
/// other one truncates a number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ProfitLen {
    /// Longest amount printed in full.
    plain: usize,
    /// Longest amount in the abbreviated spelling.
    si: usize,
    /// Longest suffix printed in full, brackets included; zero when no row carries one.
    plain_suffix: usize,
    /// Longest abbreviated suffix, brackets included; zero when no row carries one.
    si_suffix: usize,
    /// Whether this measurement contains a magnitude large enough to permit compact SI.
    has_si_magnitude: bool,
}

impl ProfitLen {
    /// Measure one row's contribution to the column's width.
    ///
    /// Args:
    ///     value: Projected profit of the row.
    ///     last: Profit of its newest closed trade, or `None` when the suffix is switched off or
    ///         the row has no such trade.
    ///     unit: Exact quote or percent unit shared by the whole column.
    ///
    /// Returns:
    ///     Per-component character counts for this row alone.
    pub(super) fn measure(value: f64, last: Option<f64>, unit: Option<ProfitUnit>) -> Self {
        let decimals = decimals(unit);
        // The percent sign is not a separable ticker — it prints in every form — so it is counted
        // into the amount itself rather than into the ticker the column may drop.
        let tail = usize::from(matches!(unit, Some(ProfitUnit::Percent)));
        let (plain, si, has_si_magnitude) = spelling_lengths(value, decimals);
        let mut len = Self {
            plain: plain + tail,
            si: si + tail,
            plain_suffix: 0,
            si_suffix: 0,
            has_si_magnitude,
        };
        if let Some(last) = last {
            let (plain, si, has_si_magnitude) = spelling_lengths(last, decimals);
            len.plain_suffix = plain + SUFFIX_BRACKETS;
            len.si_suffix = si + SUFFIX_BRACKETS;
            len.has_si_magnitude |= has_si_magnitude;
        }
        len
    }

    /// Fold another row's measurement into this running maximum.
    ///
    /// Args:
    ///     other: Measurement of one more row.
    pub(super) fn absorb(&mut self, other: Self) {
        self.plain = self.plain.max(other.plain);
        self.si = self.si.max(other.si);
        self.plain_suffix = self.plain_suffix.max(other.plain_suffix);
        self.si_suffix = self.si_suffix.max(other.si_suffix);
        self.has_si_magnitude |= other.has_si_magnitude;
    }

    /// Return how many characters the widest measured value needs in one form.
    ///
    /// Args:
    ///     form: Form the column is considering.
    ///     ticker: Character count of the currency ticker, zero when there is no separable one.
    ///
    /// Returns:
    ///     Character count of the widest cell that form would draw.
    fn chars(self, form: ProfitForm, ticker: usize) -> usize {
        let mut count = if form.si { self.si } else { self.plain };
        if form.suffix {
            count += if form.si {
                self.si_suffix
            } else {
                self.plain_suffix
            };
        }
        if form.ticker && ticker > 0 {
            count += TICKER_GAP + ticker;
        }
        count
    }
}

/// Everything outside the values themselves that decides the profit column's width.
///
/// Every field is in DESIGN units, like the fixed columns beside it, so the caller divides its
/// measured pixel advances by the active UI scale before filling this in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ColumnMetrics {
    /// Advance of one monospace character at the data rows' size.
    pub(super) row_char: f32,
    /// Advance of one monospace character at the footer's larger size.
    pub(super) total_char: f32,
    /// Measured width of the plain heading, sort arrow included.
    pub(super) heading: f32,
    /// Measured width of the heading that names the unit, used when the ticker is dropped.
    pub(super) heading_with_unit: f32,
    /// Characters in the currency ticker; zero when the unit carries no separable one.
    pub(super) ticker: usize,
    /// Width the column may take before it would eat the name column's promised minimum.
    pub(super) available: f32,
}

/// How far the column has already been pushed within one period.
///
/// Both halves ratchet together: the width, so an ordinary refresh cannot pull every name sideways
/// for one digit; the rung, so one core crossing a digit boundary cannot strip the ticker from
/// every cell and put it back a refresh later.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ProfitFloor {
    /// Widest design-reference width seen since the floor was last released.
    pub(super) width: f32,
    /// Lowest ladder rung reached since the floor was last released.
    pub(super) rung: usize,
}

/// A retained floor together with the measurement it was taken under.
///
/// A ratchet that never releases is a bug of its own: a window narrowed for a moment would keep the
/// column degraded, and a widened one would never get its ticker back. So the floor is carried only
/// while the QUESTION it answered is unchanged — same unit, same room, same glyph advance — which
/// also covers the Font slider and the UI scale, neither of which touches the data.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ColumnFloor {
    /// Unit the floor was measured in; another unit measures different text entirely.
    pub(super) unit: Option<ProfitUnit>,
    /// Room the column was offered when the floor was taken.
    pub(super) available: f32,
    /// Glyph advance the floor was measured with.
    pub(super) row_char: f32,
    /// How far the column was pushed under exactly those conditions.
    pub(super) floor: ProfitFloor,
}

impl ColumnFloor {
    /// Return the floor that still applies, which is none at all when the measurement moved.
    ///
    /// Args:
    ///     unit: Unit the incoming snapshot is measured in.
    ///     metrics: Metrics the incoming measurement runs under.
    ///
    /// Returns:
    ///     The retained floor, or an empty one when it describes different conditions.
    pub(super) fn carried(self, unit: Option<ProfitUnit>, metrics: &ColumnMetrics) -> ProfitFloor {
        let same = self.unit == unit
            && self.available == metrics.available
            && self.row_char == metrics.row_char;
        if same {
            self.floor
        } else {
            ProfitFloor::default()
        }
    }

    /// Record one resolved column as the floor for the next measurement under the same conditions.
    ///
    /// Args:
    ///     unit: Unit that was measured.
    ///     metrics: Metrics the measurement ran under.
    ///     column: The column that was resolved.
    ///
    /// Returns:
    ///     The floor to retain until the conditions change.
    pub(super) fn taken(
        unit: Option<ProfitUnit>,
        metrics: &ColumnMetrics,
        column: ProfitColumn,
    ) -> Self {
        Self {
            unit,
            available: metrics.available,
            row_char: metrics.row_char,
            floor: ProfitFloor {
                width: column.width,
                rung: column.rung,
            },
        }
    }
}

/// One resolved profit column: how it prints, how far down the ladder that was, and how wide it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ProfitColumn {
    /// Form every cell in the column draws in.
    pub(super) form: ProfitForm,
    /// Rung of the degradation ladder this form sits on, counted from the richest at zero.
    ///
    /// Carried so the caller can hold the column to it: a value that grows one digit and shrinks
    /// back would otherwise strip the ticker from every cell and restore it a refresh later.
    pub(super) rung: usize,
    /// Design-reference width the column claims.
    pub(super) width: f32,
}

/// Return every form the column may fall back to, best first.
///
/// The order ranks what each step COSTS the reader. Dropping the ticker costs nothing — the whole
/// table is in one currency and the heading names it instead — so it goes first. Dropping the
/// suffix costs a number printed nowhere else, and it buys the ticker back, so it goes second.
/// Abbreviating is last: it is the only step that takes precision away from the figure the window
/// exists to show.
///
/// Args:
///     want_suffix: Whether the user asked for the `total(last)` suffix at all.
///     has_ticker: Whether the unit carries a ticker that can be dropped separately.
///     allow_si: Whether at least one displayed magnitude reaches [`SI_FLOOR`].
///
/// Returns:
///     The applicable candidate forms, in preference order.
fn candidate_forms(
    want_suffix: bool,
    has_ticker: bool,
    allow_si: bool,
) -> impl Iterator<Item = ProfitForm> + Clone {
    const LADDER: [ProfitForm; 6] = [
        ProfitForm {
            suffix: true,
            ticker: true,
            si: false,
        },
        ProfitForm {
            suffix: true,
            ticker: false,
            si: false,
        },
        ProfitForm {
            suffix: false,
            ticker: true,
            si: false,
        },
        ProfitForm {
            suffix: false,
            ticker: false,
            si: false,
        },
        ProfitForm {
            suffix: false,
            ticker: true,
            si: true,
        },
        ProfitForm {
            suffix: false,
            ticker: false,
            si: true,
        },
    ];
    LADDER.into_iter().filter(move |form| {
        (!form.suffix || want_suffix) && (!form.ticker || has_ticker) && (!form.si || allow_si)
    })
}

/// Return the width one form needs for its VALUES alone.
///
/// The footer is measured on its own larger step: the grand total is drawn at title size, so the
/// same character count claims more room there than in the rows above it.
///
/// Args:
///     rows: Widest data row and subtotal.
///     total: The footer's own value.
///     form: Form being considered.
///     metrics: Ticker length and character advances.
///
/// Returns:
///     Design-reference width the cells need.
fn value_width(
    rows: ProfitLen,
    total: ProfitLen,
    form: ProfitForm,
    metrics: &ColumnMetrics,
) -> f32 {
    let body = rows.chars(form, metrics.ticker) as f32 * metrics.row_char;
    let footer = total.chars(form, metrics.ticker) as f32 * metrics.total_char;
    body.max(footer)
}

/// Return the heading one form is drawn under.
///
/// Args:
///     form: Form being considered.
///     metrics: Both measured headings.
///
/// Returns:
///     Design-reference width of that heading.
fn heading_width(form: ProfitForm, metrics: &ColumnMetrics) -> f32 {
    if form.ticker || metrics.ticker == 0 {
        metrics.heading
    } else {
        metrics.heading_with_unit
    }
}

/// Choose the best form the available room allows, and the width it claims.
///
/// The column is sized from its CONTENT, not from a worst case nobody is showing: whatever the
/// values do not need is left to the name column, which is the one that truncates otherwise.
///
/// Only the VALUES decide which form fits. The heading is a floor on the resulting width, never a
/// reason to degrade: it is one ellipsis-tolerant label, and letting it push a money value off its
/// exact spelling would trade precision for a word — and would make that trade differently in every
/// locale, since the translations are not the same length.
///
/// Args:
///     rows: Folded measurement of every data row and subtotal in the snapshot.
///     total: Measurement of the grand total drawn in the footer.
///     want_suffix: Whether the user asked for the `total(last)` suffix.
///     metrics: Headings, ticker, character advances and the room the column may take.
///     floor: Widest width and lowest rung the column already reached within this period; neither
///         is given back until the measurement is released.
///
/// Returns:
///     The first form at or below the floor rung that fits, or the last rung clamped to the room
///     available when none does.
pub(super) fn plan_profit_column(
    rows: ProfitLen,
    total: ProfitLen,
    want_suffix: bool,
    metrics: &ColumnMetrics,
    floor: ProfitFloor,
) -> ProfitColumn {
    let allow_si = rows.has_si_magnitude || total.has_si_magnitude;
    let ladder = candidate_forms(want_suffix, metrics.ticker > 0, allow_si);
    // Held to the LAST rung rather than past it: the ladder shortens when a unit carries no
    // ticker, and a floor recorded on the longer one must still name a form that exists.
    let start = floor.rung.min(ladder.clone().count().saturating_sub(1));
    let mut chosen = None;
    for (rung, form) in ladder.enumerate().skip(start) {
        let values = value_width(rows, total, form, metrics);
        chosen = Some(ProfitColumn {
            form,
            rung,
            // The ratchet only ever widens the column, and never past the room it was offered.
            width: values
                .max(heading_width(form, metrics))
                .ceil()
                .max(floor.width)
                .min(metrics.available),
        });
        if values <= metrics.available {
            break;
        }
    }
    // When nothing fits — an extreme window or an extreme value — the last rung is what stays and
    // the cell's own ellipsis takes over, with the column still held to the room it was given
    // rather than pushing the name column below its promised minimum.
    chosen.expect("the ladder always holds the plain bare form")
}

/// Return the ticker a column may print beside its amounts, when the unit has a separable one.
///
/// Args:
///     unit: Exact comparable unit, or `None` for an empty result.
///
/// Returns:
///     The quote ticker, or `None` for percent and unknown units.
pub(super) fn unit_ticker(unit: Option<ProfitUnit>) -> Option<&'static str> {
    match unit {
        Some(ProfitUnit::Quote(currency)) => Some(currency.ticker()),
        Some(ProfitUnit::Percent) | None => None,
    }
}

/// Return the display decimals one unit rounds every amount in the column to.
///
/// Args:
///     unit: Exact comparable unit, or `None` for an empty result.
///
/// Returns:
///     The quote's own precision, or two places for percent and unknown units.
fn decimals(unit: Option<ProfitUnit>) -> usize {
    match unit {
        Some(ProfitUnit::Quote(currency)) => currency.display_decimals(),
        Some(ProfitUnit::Percent) | None => 2,
    }
}

/// Return one amount's spelling lengths and whether it permits compact SI.
///
/// Costs ONE format below [`SI_FLOOR`], where the abbreviated spelling is the plain one.
///
/// Args:
///     value: Raw signed amount.
///     decimals: Places the unit rounds to.
///
/// Returns:
///     Characters in the full and abbreviated spellings, then whether the rounded magnitude
///     reaches [`SI_FLOOR`].
fn spelling_lengths(value: f64, decimals: usize) -> (usize, usize, bool) {
    // ASCII throughout — sign, digits, separator and the K/M/B/T marker — so the byte length is
    // the character count without walking the string a second time.
    let plain = fmt::signed_fixed(value, decimals)
        .or_else(|| fmt::signed_fixed(0.0, decimals))
        .expect("zero is always a finite fixed amount")
        .0
        .len();
    let abbreviated = abbreviated(value, decimals);
    let si = abbreviated.as_ref().map_or(plain, String::len);
    (plain, si, abbreviated.is_some())
}

/// Return the abbreviated spelling of an amount, when abbreviating it means anything.
///
/// Rounds to the unit's own precision FIRST, so the digits can never disagree with the sign the
/// cell is coloured by, and refuses to touch anything below [`SI_FLOOR`], where abbreviating would
/// silently restate the number instead of shortening it. The sign comes from
/// [`fmt::signed_fixed`], so switching forms never changes how the row is classified or coloured.
///
/// Args:
///     value: Raw signed amount.
///     decimals: Places the unit rounds to.
///
/// Returns:
///     Abbreviated text, or `None` when the full spelling is what this form prints.
fn abbreviated(value: f64, decimals: usize) -> Option<String> {
    let rounded = fmt::round_to(value, decimals)?;
    if rounded.abs() < SI_FLOOR {
        return None;
    }
    let sign = fmt::signed_fixed(value, decimals)?.1;
    let prefix = sign.pick("+", "-", "");
    Some(format!("{prefix}{}", fmt::compact_si(rounded.abs())))
}

/// Format profit in the column's chosen form, optionally carrying the newest closed trade.
///
/// The suffix goes INSIDE the unit — `-57.11(-0.60) USDT`, not `-57.11 USDT (-0.60)` — so the two
/// amounts read as one measurement in one currency, which is what they are. Both are rounded to
/// the same unit decimals, so the bracket can never claim precision the total does not have.
///
/// The returned sign describes the TOTAL. The suffix is a different trade and may disagree; the
/// cell is coloured by the number it is about.
///
/// Args:
///     value: Projected profit.
///     last: Profit of the newest closed trade, when one exists.
///     unit: Exact quote or percent unit.
///     form: Form the whole column agreed on.
///
/// Returns:
///     Signed text carrying whatever its form keeps, and the sign represented after display
///     rounding.
pub(super) fn format_profit(
    value: f64,
    last: Option<f64>,
    unit: Option<ProfitUnit>,
    form: ProfitForm,
) -> (String, DeltaSign) {
    let decimals = decimals(unit);
    let spell = |value: f64| {
        let (plain, sign) = fmt::signed_fixed(value, decimals)
            .or_else(|| fmt::signed_fixed(0.0, decimals))
            .expect("zero is always a finite fixed amount");
        let text = form
            .si
            .then(|| abbreviated(value, decimals))
            .flatten()
            .unwrap_or(plain);
        (text, sign)
    };
    let (text, sign) = spell(value);
    let text = match last.filter(|_| form.suffix) {
        Some(last) => format!("{text}({})", spell(last).0),
        None => text,
    };
    let text = match unit {
        Some(ProfitUnit::Quote(currency)) if form.ticker => {
            format!("{text} {}", currency.ticker())
        }
        Some(ProfitUnit::Percent) => format!("{text}%"),
        Some(ProfitUnit::Quote(_)) | None => text,
    };
    (text, sign)
}

/// Format a monitor trade count with the terminal's shared thousands grouping.
///
/// Args:
///     value: Closed-trade count.
///
/// Returns:
///     ASCII digits separated into space-grouped thousands.
pub(super) fn format_trade_count(value: i64) -> String {
    fmt::group_thousands(&value.to_string())
}

/// Format win rate with the terminal's shared half-away-from-zero percentage rounding.
///
/// Args:
///     value: Win percentage in `0..=100`.
///
/// Returns:
///     Percentage with one decimal place.
pub(super) fn format_win_rate(value: f64) -> String {
    fmt::pct(value, 1)
        .map(|(text, _)| text)
        .unwrap_or_else(|| "0.0%".to_string())
}

/// Format average order spend in the query's comparable quote unit.
///
/// Args:
///     value: Average positive spend.
///     unit: Exact query unit.
///
/// Returns:
///     Compact unsigned order size with a quote ticker when known.
pub(super) fn format_amount(value: f64, unit: Option<ProfitUnit>) -> String {
    let amount = fmt::compact(value, decimals(unit));
    match unit_ticker(unit) {
        Some(ticker) => format!("{amount} {ticker}"),
        None => amount,
    }
}
