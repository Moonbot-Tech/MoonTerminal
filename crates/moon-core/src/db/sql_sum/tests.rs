//! The fold must state what SQLite's own `SUM` states, bit for bit, on the inputs it accepts.

use rusqlite::Connection;
use rusqlite::types::Value;

use super::{GroupKey, SqliteSum, SumColumn, SumZero, Unrepresentable};

/// Deterministic xorshift, so a failing case replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// One input of the kinds a totals column sees: NULLs, small and huge integers, and reals
    /// spanning magnitudes, signs and exact cancellations.
    fn value(&mut self) -> Value {
        match self.below(9) {
            0 => Value::Null,
            1 => Value::Integer(self.below(3) as i64),
            2 => Value::Integer(self.next() as i64 >> self.below(64)),
            3 => Value::Integer(if self.below(2) == 0 {
                i64::MAX - self.below(4) as i64
            } else {
                i64::MIN + self.below(4) as i64
            }),
            4 => Value::Real(0.0),
            5 => Value::Real((self.next() as i64 as f64) * 1e-12),
            6 => Value::Real(
                f64::from_bits(self.next() >> 2) * if self.below(2) == 0 { 1.0 } else { -1.0 },
            ),
            7 => Value::Real((self.below(2_000_000) as f64 - 1_000_000.0) / 7.0),
            _ => Value::Real(1e16 * (self.below(3) as f64 - 1.0) + 0.1),
        }
    }
}

/// Random sets summed by SQLite and by the fold agree exactly whenever the fold answers, both
/// with SQLite reading the rows in the fold's own order and in a shuffled one: the integer path,
/// the switch to compensation, split big integers and overflow all included.
#[test]
fn fold_matches_sqlite_sum_bit_for_bit_in_any_order() {
    for shuffled in [false, true] {
        let (compared, refused, overflowed) = compare_with_sqlite(shuffled, |rng| rng.value());
        // The generator is adversarial -- bit patterns up to 1e300 beside `i64::MAX` -- so most
        // sets are refused; what matters is that no answer given differs.
        assert!(
            compared > 300 && overflowed > 0,
            "shuffled {shuffled}: {compared} compared, {refused} refused, {overflowed} overflowed"
        );
    }
}

/// Money-shaped sets -- report profits and spends across magnitudes, never adversarial bit
/// patterns -- are refused only at a genuine tie, rare enough that re-reading those slices costs
/// the Mini App nothing.
#[test]
fn money_shaped_sets_are_rarely_refused() {
    let (compared, refused, _) = compare_with_sqlite(true, |rng| match rng.below(4) {
        0 => Value::Real(((rng.below(2_000_001) as f64) - 1_000_000.0) / 997.0),
        1 => Value::Real((rng.below(100_000) as f64) * 1e-8),
        2 => Value::Real(0.0),
        _ => Value::Integer(rng.below(2) as i64),
    });
    assert!(
        refused < 30 && compared > 2_900,
        "{compared} compared, {refused} refused"
    );
}

/// Short values -- multiples of 1/8 near 2^47 -- whose sums outgrow a double's precision, so
/// they round and tie constantly: the exact-accumulation argument answers every one, and SQLite
/// reading them shuffled agrees bit for bit.
#[test]
fn ties_of_short_values_are_answered_and_agree() {
    let (compared, refused, _) = compare_with_sqlite(true, |rng| {
        Value::Real((rng.below(1 << 50) as f64 + 0.5 * rng.below(4) as f64) * 0.125)
    });
    assert!(
        refused == 0 && compared == 3_000,
        "{compared} compared, {refused} refused"
    );
}

/// An integer past `2^52` seeds SQLite's compensation with a remainder the order bounds do not
/// cover: fed `A, 0.5, 2^-40` SQLite returns `A`, fed `0.5, A, 2^-40` it returns `A + 1`. The
/// fold refuses rather than state either.
#[test]
fn big_integers_are_refused_before_their_remainder_can_matter() {
    let big = Value::Integer((1_i64 << 52) + 10_000);
    for order in [
        [big.clone(), Value::Real(0.5), Value::Real(2f64.powi(-40))],
        [Value::Real(0.5), big.clone(), Value::Real(2f64.powi(-40))],
    ] {
        let mut sum = SqliteSum::default();
        for value in &order {
            sum.step(value).expect("numeric input");
        }
        assert_eq!(
            SumColumn::sum("v", SumZero::Real).finish(&sum),
            Err(Unrepresentable::OrderSensitive)
        );
    }
}

/// Sum `SETS` random sets in SQLite, whose rows are inserted in the fold's order or shuffled,
/// and compare every answer the fold gives.
///
/// Returns:
///     Sets compared, sets the fold refused as order-sensitive, and sets SQLite overflowed on.
fn compare_with_sqlite(shuffled: bool, value: impl Fn(&mut Rng) -> Value) -> (usize, usize, usize) {
    const SETS: i64 = 3_000;
    let conn = Connection::open_in_memory().expect("open sum fixture");
    conn.execute_batch("CREATE TABLE t (g INTEGER, v)")
        .expect("create sum fixture");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut sets = Vec::new();
    {
        let mut insert = conn
            .prepare("INSERT INTO t VALUES (?1, ?2)")
            .expect("prepare insert");
        for group in 0..SETS {
            let len = 1 + rng.below(40) as usize;
            let values = (0..len).map(|_| value(&mut rng)).collect::<Vec<_>>();
            let mut stored = values.clone();
            if shuffled {
                for index in (1..stored.len()).rev() {
                    stored.swap(index, rng.below(index as u64 + 1) as usize);
                }
            }
            for value in &stored {
                insert
                    .execute(rusqlite::params![group, value])
                    .expect("insert value");
            }
            sets.push(values);
        }
    }
    let mut query = conn
        .prepare("SELECT COALESCE(SUM(v),0.0), COALESCE(SUM(v),0) FROM t WHERE g = ?1")
        .expect("prepare sum");
    let (mut compared, mut refused, mut overflowed) = (0, 0, 0);
    for (group, values) in sets.iter().enumerate() {
        let mut sum = SqliteSum::default();
        for value in values {
            sum.step(value).expect("numeric input");
        }
        let expected = query.query_row([group as i64], |row| {
            Ok((row.get::<_, Value>(0)?, row.get::<_, Value>(1)?))
        });
        let real = SumColumn::sum("v", SumZero::Real).finish(&sum);
        let integer = SumColumn::sum("v", SumZero::Integer).finish(&sum);
        match (expected, real, integer) {
            (Err(error), Err(Unrepresentable::OrderSensitive), _) => {
                assert!(
                    error.to_string().contains("integer overflow"),
                    "set {group}: {error}"
                );
                refused += 1;
                overflowed += 1;
            }
            (_, Err(Unrepresentable::OrderSensitive), _) => refused += 1,
            (Ok((expected_real, expected_integer)), Ok(real), Ok(integer)) => {
                assert_eq!(bits(&real), bits(&expected_real), "set {group}: {values:?}");
                assert_eq!(
                    bits(&integer),
                    bits(&expected_integer),
                    "set {group}: {values:?}"
                );
                compared += 1;
            }
            (expected, real, _) => {
                panic!("set {group}: SQLite {expected:?}, fold {real:?}: {values:?}")
            }
        }
    }
    (compared, refused, overflowed)
}

/// Exact identity of a value: type plus the raw bits of a real.
fn bits(value: &Value) -> (u8, u64) {
    match *value {
        Value::Null => (0, 0),
        Value::Integer(i) => (1, i as u64),
        Value::Real(r) => (2, r.to_bits()),
        Value::Text(_) | Value::Blob(_) => (3, 0),
    }
}

/// A TEXT or BLOB input is refused, never guessed at.
#[test]
fn text_and_blob_inputs_are_refused() {
    let mut sum = SqliteSum::default();
    assert_eq!(
        sum.step(&Value::Text("1".into())),
        Err(Unrepresentable::NonNumeric)
    );
    assert_eq!(
        sum.step(&Value::Blob(vec![1])),
        Err(Unrepresentable::NonNumeric)
    );
}

/// `COUNT(*)` counts every row, NULL or not, and the zero column states `0.0` whatever it saw.
#[test]
fn count_and_zero_columns() {
    let mut sum = SqliteSum::default();
    for value in [Value::Integer(1), Value::Integer(1), Value::Integer(1)] {
        sum.step(&value).expect("integer");
    }
    assert_eq!(SumColumn::Count.finish(&sum), Ok(Value::Integer(3)));
    assert_eq!(SumColumn::ZeroReal.finish(&sum), Ok(Value::Real(0.0)));
    assert_eq!(SumColumn::Count.row_sql(), "1");
    assert_eq!(SumColumn::ZeroReal.aggregate_sql(), "0.0");
    assert_eq!(
        SumColumn::sum("x", SumZero::Integer).aggregate_sql(),
        "COALESCE(SUM(x),0)"
    );
}

/// Group keys order as SQLite's `GROUP BY` emits them: NULL first, then integers ascending; a
/// real key is refused because its grouping needs SQLite's cross-type comparison.
#[test]
fn group_keys_order_like_sqlite() {
    let mut keys = [
        Value::Integer(5),
        Value::Null,
        Value::Integer(-2),
        Value::Integer(0),
    ]
    .iter()
    .map(|value| GroupKey::of(value).expect("integer or null"))
    .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys.iter().map(|key| key.value()).collect::<Vec<_>>(),
        vec![
            Value::Null,
            Value::Integer(-2),
            Value::Integer(0),
            Value::Integer(5)
        ]
    );
    assert_eq!(GroupKey::of(&Value::Real(1.0)), None);
}
