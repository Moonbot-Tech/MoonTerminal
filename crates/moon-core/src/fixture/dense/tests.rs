use super::*;

/// Each consumer sees its own tail, while eviction asks a lagging consumer to replace its data.
#[test]
fn generated_history_retains_real_rows_and_detects_a_lapped_cursor() {
    let mut history = DenseHistory::new(1_000_000, 1, 300_000);
    let mut rows = Vec::new();
    assert!(history.copy_since(None, &mut rows));
    assert_eq!(rows.len(), CAPACITY);
    assert!(
        rows.windows(2)
            .all(|pair| pair[0].unix_millis() <= pair[1].unix_millis())
    );
    assert!(rows.iter().any(|row| row.price > 103.0));
    assert!(rows.iter().any(|row| row.is_buy()));
    assert!(rows.iter().any(|row| !row.is_buy()));
    let next = history.produced();
    history.advance(250, 100);
    assert!(!history.copy_since(Some(next), &mut rows));
    assert_eq!(rows.len(), 250);
    assert_eq!(rows.last().unwrap().unix_millis(), 1_000_100);
    assert_eq!(history.retained(), CAPACITY);
    assert!(!history.copy_since(Some(next), &mut rows));
    assert_eq!(
        rows.len(),
        250,
        "reading never consumes another chart's data"
    );
    history.advance(CAPACITY, 1_000);
    assert!(history.copy_since(Some(next), &mut rows));
    assert_eq!(rows.len(), CAPACITY);
}

/// Fixed seed and logical clock must reproduce values regardless of the machine running the test.
#[test]
fn generated_history_repeats_the_same_prices_volumes_and_timestamps() {
    let first = DenseHistory::new(1_000_000, 7, 300_000);
    let second = DenseHistory::new(1_000_000, 7, 300_000);
    let different = DenseHistory::new(1_000_000, 8, 300_000);
    assert_eq!(first.rows, second.rows);
    assert_ne!(first.rows, different.rows);
}
