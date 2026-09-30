//! SQLite's `SUM` reproduced in Rust, so one row pass can be summed into several groupings and
//! still state, bit for bit, what one `GROUP BY` statement per grouping would have stated.
//!
//! The bundled SQLite (3.53) sums exactly while every input is an integer, and switches to
//! Kahan-Babuska-Neumaier compensation at the first real (or integer overflow). The fold below is
//! a transcription of `sumStep`/`sumFinalize` in `func.c`; the sibling tests hold it against the
//! real `SUM` on random inputs. Anything the transcription does not cover -- a TEXT or BLOB value,
//! which SQLite would first coerce -- is refused rather than guessed, and the caller falls back to
//! asking SQLite.
//!
//! # Row order
//!
//! A caller folds rows in its own order, not in the order SQLite would have fed the same group,
//! and a compensated sum of reals is not order-free in general. What makes it order-free here is
//! checked, not assumed. Each compensation step is exact, so in ANY order the running sum plus the
//! exact corrections equals the exact sum `S` of the inputs, and SQLite returns `fl(S + d)` where
//! `d` is only the rounding of the corrections' own accumulation: `|d| <= n^2 u^2 sum|x|` (Higham,
//! recursive summation, applied to corrections each at most `u` times a partial sum). The fold
//! keeps `S` exactly (a Shewchuk expansion, as Python's `math.fsum`) and answers only when `S`
//! lies far enough from every rounding boundary that every `d` within that bound rounds to the
//! same double -- which is then what any order returns.
//!
//! That margin cannot help at an exact tie, which report money hits often: its values carry few
//! significant bits, so their exact sums often need exactly one bit more than a double holds. A
//! second, exact argument covers it. Every input is a multiple of the smallest power of two `q`
//! among their lowest set bits, and so is every partial sum, every correction and every sum of
//! corrections. While the corrections' total magnitude, at most `n u sum|x|`, stays under
//! `2^53 q`, each of those sums is a double, so SQLite accumulates the corrections EXACTLY in any
//! order and returns `fl(S)`, the exact sum rounded half to even -- ties included. Otherwise the
//! fold refuses ([`Unrepresentable::OrderSensitive`]).
//!
//! Both arguments hold only while the integer inputs stay small. From `2^52` on, SQLite splits an
//! integer into two additions and seeds the compensation with a remainder of up to 16383 when it
//! leaves the integer path -- a correction neither bound covers -- and further on some orders
//! overflow where others do not. The fold refuses once the integers' magnitudes sum to `2^52`,
//! which report money never reaches.

use rusqlite::types::Value;

/// What a summed column states for a group with no non-NULL input: the `COALESCE` default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::db) enum SumZero {
    /// `0`, an INTEGER.
    Integer,
    /// `0.0`, a REAL.
    Real,
}

impl SumZero {
    /// The SQL literal.
    fn literal(self) -> &'static str {
        match self {
            Self::Integer => "0",
            Self::Real => "0.0",
        }
    }

    /// The value the literal evaluates to.
    fn value(self) -> Value {
        match self {
            Self::Integer => Value::Integer(0),
            Self::Real => Value::Real(0.0),
        }
    }
}

/// One column of a grouped totals statement, described once for both of its shapes.
///
/// [`Self::aggregate_sql`] is what a `GROUP BY` statement selects; [`Self::row_sql`] is what a
/// row pass selects so that [`SqliteSum`] can fold it into the same value. Building both from one
/// description is what keeps the two from ever summing different expressions.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::db) enum SumColumn {
    /// `COUNT(*)`.
    Count,
    /// `COALESCE(SUM(term), zero)`.
    Sum {
        /// Per-row SQL expression.
        term: String,
        /// Default for a group whose every term is NULL.
        zero: SumZero,
    },
    /// The literal `0.0`, for a source that cannot carry the column at all.
    ZeroReal,
}

impl SumColumn {
    /// `COALESCE(SUM(term), zero)`.
    pub(in crate::db) fn sum(term: impl Into<String>, zero: SumZero) -> Self {
        Self::Sum {
            term: term.into(),
            zero,
        }
    }

    /// The column as a grouped statement selects it.
    pub(in crate::db) fn aggregate_sql(&self) -> String {
        match self {
            Self::Count => "COUNT(*)".to_string(),
            Self::Sum { term, zero } => format!("COALESCE(SUM({term}),{})", zero.literal()),
            Self::ZeroReal => "0.0".to_string(),
        }
    }

    /// The column as a row pass selects it, one value per row for [`SqliteSum::step`].
    pub(in crate::db) fn row_sql(&self) -> String {
        match self {
            Self::Count => "1".to_string(),
            Self::Sum { term, .. } => format!("({term})"),
            Self::ZeroReal => "NULL".to_string(),
        }
    }

    /// The grouped value of this column, given the fold of its row values.
    ///
    /// Args:
    ///     sum: Fold of every row value this group admitted, in the caller's order.
    ///
    /// Returns:
    ///     Exactly the value the grouped statement would have returned in whatever order it fed
    ///     the rows (see "Row order" in the module docs).
    ///
    /// Errors:
    ///     [`Unrepresentable::OrderSensitive`] where the order could change the result, an
    ///     integer overflow among them.
    pub(in crate::db) fn finish(&self, sum: &SqliteSum) -> Result<Value, Unrepresentable> {
        match self {
            Self::Count => Ok(Value::Integer(sum.cnt)),
            Self::Sum { zero, .. } => sum.finish(*zero),
            Self::ZeroReal => Ok(Value::Real(0.0)),
        }
    }
}

/// Why a fold cannot state what SQLite would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::db) enum Unrepresentable {
    /// A TEXT or BLOB input, which SQLite coerces by rules this fold does not reproduce.
    NonNumeric,
    /// The result could depend on the order SQLite fed the rows in -- including whether the
    /// statement fails with an integer overflow.
    OrderSensitive,
}

/// The running state of one SQLite `SUM`: `SumCtx` in `func.c`.
#[derive(Clone, Debug)]
pub(in crate::db) struct SqliteSum {
    /// Non-NULL inputs seen.
    cnt: i64,
    /// Exact integer sum, valid while `approx` is false.
    i_sum: i64,
    /// Compensated running sum, once `approx`.
    r_sum: f64,
    /// Accumulated compensation, once `approx`.
    r_err: f64,
    /// Whether the sum left the exact integer path.
    approx: bool,
    /// An integer overflow that no later real input has cleared.
    overflow: bool,
    /// The exact sum of every input, as non-overlapping partials in increasing magnitude.
    exact: Vec<f64>,
    /// Sum of the inputs' magnitudes, the scale of the order bound.
    magnitude: f64,
    /// Sum of the integer inputs' magnitudes: under `2^52`, no order overflows, splits an
    /// integer or seeds the compensation with a remainder (module docs, "Row order").
    integer_magnitude: u128,
    /// Additions the compensated path performs, `n` in the order bound.
    steps: u64,
    /// Smallest lowest-set-bit value among the nonzero inputs, `q` in the module docs.
    grain: f64,
}

impl Default for SqliteSum {
    fn default() -> Self {
        Self {
            cnt: 0,
            i_sum: 0,
            r_sum: 0.0,
            r_err: 0.0,
            approx: false,
            overflow: false,
            exact: Vec::new(),
            magnitude: 0.0,
            integer_magnitude: 0,
            steps: 0,
            grain: f64::INFINITY,
        }
    }
}

/// Magnitude from which an `i64` is split before it is added as a double (`2^52`).
const SPLIT_AT: i64 = 4_503_599_627_370_496;

impl SqliteSum {
    /// Add one row value: `sumStep`.
    ///
    /// Errors:
    ///     [`Unrepresentable::NonNumeric`] for TEXT and BLOB, leaving the fold unusable.
    pub(in crate::db) fn step(&mut self, value: &Value) -> Result<(), Unrepresentable> {
        match *value {
            Value::Null => Ok(()),
            Value::Integer(i) => {
                self.cnt += 1;
                self.integer_magnitude += u128::from(i.unsigned_abs());
                self.magnitude += i.unsigned_abs() as f64;
                if i <= -SPLIT_AT || i >= SPLIT_AT {
                    let small = i % 16_384;
                    self.add_exact((i - small) as f64);
                    self.add_exact(small as f64);
                    self.steps += 2;
                } else {
                    self.add_exact(i as f64);
                    self.steps += 1;
                }
                if self.approx {
                    self.step_i64(i);
                } else if let Some(sum) = self.i_sum.checked_add(i) {
                    self.i_sum = sum;
                } else {
                    self.overflow = true;
                    self.init(self.i_sum);
                    self.approx = true;
                    self.step_i64(i);
                }
                Ok(())
            }
            Value::Real(r) => {
                self.cnt += 1;
                self.magnitude += r.abs();
                self.add_exact(r);
                self.steps += 1;
                if self.approx {
                    self.overflow = false;
                } else {
                    self.init(self.i_sum);
                    self.approx = true;
                }
                self.step_f64(r);
                Ok(())
            }
            Value::Text(_) | Value::Blob(_) => Err(Unrepresentable::NonNumeric),
        }
    }

    /// `COALESCE(SUM(..), zero)` over the values stepped so far: `sumFinalize`.
    fn finish(&self, zero: SumZero) -> Result<Value, Unrepresentable> {
        if self.cnt == 0 {
            return Ok(zero.value());
        }
        // Checked first: it is what rules out an overflow in ANY order, this one included.
        if self.integer_magnitude >= SPLIT_AT as u128 || self.overflow {
            return Err(Unrepresentable::OrderSensitive);
        }
        if !self.approx {
            return Ok(Value::Integer(self.i_sum));
        }
        // `sqlite3IsOverflow`: an infinite or NaN compensation is dropped, not added.
        let sum = if self.r_err.is_finite() {
            self.r_sum + self.r_err
        } else {
            self.r_sum
        };
        if self.order_free(sum) {
            Ok(Value::Real(sum))
        } else {
            Err(Unrepresentable::OrderSensitive)
        }
    }

    /// Whether every feeding order returns `sum` (module docs, "Row order").
    fn order_free(&self, sum: f64) -> bool {
        let unit = f64::EPSILON / 2.0;
        let n = (self.steps + 1) as f64;
        // Four times the bound: slack for `magnitude` and the residual being rounded themselves.
        let bound = 4.0 * n * n * unit * unit * self.magnitude;
        if !sum.is_finite() || !bound.is_finite() || self.exact.iter().any(|p| !p.is_finite()) {
            return false;
        }
        // Two additions leave nothing to reorder: `fl(a + b)` commutes and its correction is
        // exact, so the compensation is accumulated without rounding in either order.
        if self.steps <= 2 {
            return true;
        }
        let nearest = round_expansion(&self.exact);
        if nearest != sum {
            return false;
        }
        // Corrections accumulated exactly in every order: `fl(S)`, ties included.
        let corrections = 2.0 * n * unit * (1.0 + 2.0 * n * unit) * self.magnitude;
        if corrections < self.grain * TWO_POW_53 {
            return true;
        }
        // `S - nearest`, from the same expansion with the rounded value taken out.
        let mut rest = self.exact.clone();
        expansion_add(&mut rest, -nearest);
        let residual = rest.iter().rev().fold(0.0, |acc, part| acc + part);
        // Gaps to the neighbouring doubles, compared doubled rather than halved: half the gap
        // above zero underflows to zero itself.
        let up = nearest.next_up() - nearest;
        let down = nearest - nearest.next_down();
        // Strictly inside the rounding interval on the side the residual lies, on both sides when
        // it is zero: a tie could round either way.
        let gap = if residual > 0.0 {
            up
        } else if residual < 0.0 {
            down
        } else {
            up.min(down)
        };
        2.0 * (residual.abs() * (1.0 + 4.0 * f64::EPSILON) + bound) < gap
    }

    /// Add one input to the exact sum.
    fn add_exact(&mut self, value: f64) {
        if value != 0.0 {
            self.grain = self.grain.min(lowest_bit(value));
        }
        expansion_add(&mut self.exact, value);
    }

    /// `kahanBabuskaNeumaierInit`.
    fn init(&mut self, value: i64) {
        if value <= -SPLIT_AT || value >= SPLIT_AT {
            let small = value % 16_384;
            self.r_sum = (value - small) as f64;
            self.r_err = small as f64;
        } else {
            self.r_sum = value as f64;
            self.r_err = 0.0;
        }
    }

    /// `kahanBabuskaNeumaierStepInt64`.
    fn step_i64(&mut self, value: i64) {
        if value <= -SPLIT_AT || value >= SPLIT_AT {
            let small = value % 16_384;
            self.step_f64((value - small) as f64);
            self.step_f64(small as f64);
        } else {
            self.step_f64(value as f64);
        }
    }

    /// `kahanBabuskaNeumaierStep`.
    fn step_f64(&mut self, value: f64) {
        let sum = self.r_sum;
        let next = sum + value;
        if sum.abs() > value.abs() {
            self.r_err += (sum - next) + value;
        } else {
            self.r_err += (value - next) + sum;
        }
        self.r_sum = next;
    }
}

/// `2^53`: the integers a double holds exactly run up to it.
const TWO_POW_53: f64 = 9_007_199_254_740_992.0;

/// The value of the lowest set bit of a finite nonzero double: the power of two it is an integer
/// multiple of.
fn lowest_bit(value: f64) -> f64 {
    let bits = value.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1_u64 << 52) - 1);
    if exponent == 0x7ff {
        return f64::NAN;
    }
    let (significand, scale) = if exponent == 0 {
        (mantissa, -1074)
    } else {
        (mantissa | (1_u64 << 52), exponent - 1075)
    };
    let power = scale + significand.trailing_zeros() as i32;
    if power >= -1022 {
        f64::from_bits(((power + 1023) as u64) << 52)
    } else {
        f64::from_bits(1_u64 << (power + 1074))
    }
}

/// Add `value` to an exact sum held as non-overlapping partials (Shewchuk's `msum`).
fn expansion_add(partials: &mut Vec<f64>, value: f64) {
    let mut x = value;
    let mut kept = 0;
    for index in 0..partials.len() {
        let mut y = partials[index];
        if x.abs() < y.abs() {
            std::mem::swap(&mut x, &mut y);
        }
        let hi = x + y;
        let lo = y - (hi - x);
        if lo != 0.0 {
            partials[kept] = lo;
            kept += 1;
        }
        x = hi;
    }
    partials.truncate(kept);
    partials.push(x);
}

/// The exact sum held in `partials`, correctly rounded to the nearest double, ties to even
/// (the finish of Python's `math.fsum`).
fn round_expansion(partials: &[f64]) -> f64 {
    let Some((&top, below)) = partials.split_last() else {
        return 0.0;
    };
    let mut hi = top;
    let mut lo = 0.0;
    let mut remaining = below.len();
    while remaining > 0 {
        let x = hi;
        let y = below[remaining - 1];
        remaining -= 1;
        hi = x + y;
        lo = y - (hi - x);
        if lo != 0.0 {
            break;
        }
    }
    if remaining > 0
        && ((lo < 0.0 && below[remaining - 1] < 0.0) || (lo > 0.0 && below[remaining - 1] > 0.0))
    {
        let y = lo * 2.0;
        let x = hi + y;
        if y == x - hi {
            hi = x;
        }
    }
    hi
}

/// A `GROUP BY` key this fold can order exactly as SQLite does: NULL first, then integers
/// ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::db) enum GroupKey {
    /// SQL NULL.
    Null,
    /// An INTEGER key.
    Integer(i64),
}

impl GroupKey {
    /// The key of one row value, or `None` for a REAL, TEXT or BLOB key, whose grouping and order
    /// would need SQLite's cross-type comparison.
    pub(in crate::db) fn of(value: &Value) -> Option<Self> {
        match *value {
            Value::Null => Some(Self::Null),
            Value::Integer(i) => Some(Self::Integer(i)),
            Value::Real(_) | Value::Text(_) | Value::Blob(_) => None,
        }
    }

    /// The key back as the value a grouped statement selects for it.
    pub(in crate::db) fn value(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Integer(i) => Value::Integer(i),
        }
    }
}

#[cfg(test)]
mod tests;
