use super::*;

/// Milliseconds in a day.
const DAY_MS: i64 = 86_400_000;

fn tick(time_ms: i64, price: f32, qty: f32, side: Side) -> Tick {
    Tick {
        time_ms: time_ms as f64,
        price,
        qty,
        side,
    }
}

fn same(a: &[Tick], b: &[Tick]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.time_ms == y.time_ms
                && x.price.to_bits() == y.price.to_bits()
                && x.qty.to_bits() == y.qty.to_bits()
                && x.side == y.side
        })
}

/// Every field comes back bit for bit, the side of each print included — across a byte of the
/// side bitmap and with stamps that repeat, go back, and jump months ahead.
#[test]
fn a_blob_round_trips_bit_for_bit() {
    let ticks: Vec<Tick> = (0..19)
        .map(|i| {
            let side = if i % 3 == 0 { Side::Sell } else { Side::Buy };
            tick(
                1_700_000_000_000 + i * 7,
                0.000_123_4 + i as f32,
                i as f32 * 0.5,
                side,
            )
        })
        .chain([
            tick(1_700_000_000_000, 1.0, 1.0, Side::Sell),
            tick(1_700_000_000_000 + 90 * DAY_MS, 2.0, 2.0, Side::Buy),
            tick(1_700_000_000_000 + 90 * DAY_MS, 3.0, -0.0, Side::Sell),
        ])
        .collect();
    let blob = encode(&ticks);
    assert!(same(&decode(&blob).expect("decode"), &ticks));
}

/// A size that arrives negative stays negative: the side is its own bit, not the size's sign.
#[test]
fn a_negative_size_is_not_taken_for_the_side() {
    let ticks = [tick(1, 1.0, -3.0, Side::Buy), tick(2, 1.0, 3.0, Side::Sell)];
    let back = decode(&encode(&ticks)).expect("decode");
    assert!(same(&back, &ticks));
}

/// An empty span is one byte and decodes to nothing.
#[test]
fn an_empty_blob_is_its_header_alone() {
    let blob = encode(&[]);
    assert_eq!(blob, vec![0]);
    assert!(decode(&blob).expect("decode").is_empty());
}

/// A damaged blob is an error, never a span of garbage stamps.
#[test]
fn a_damaged_blob_is_refused() {
    let ticks: Vec<Tick> = (0..1_000)
        .map(|i| tick(i * 10, 1.0, 1.0, Side::Buy))
        .collect();
    let blob = encode(&ticks);
    assert_eq!(decode(&[]).err(), Some(DecodeError::Header));
    assert_eq!(decode(&[0x80]).err(), Some(DecodeError::Header));
    assert!(decode(&blob[..blob.len() / 2]).is_err(), "cut short");
    let mut claims_more = encode(&ticks[..999]);
    claims_more.splice(0..2, encode(&ticks)[..2].iter().copied());
    assert!(decode(&claims_more).is_err(), "count and columns disagree");
    let mut huge = Vec::new();
    put_varint(&mut huge, MAX_PRINTS + 1);
    assert_eq!(
        decode(&huge).err(),
        Some(DecodeError::TooMany(MAX_PRINTS + 1))
    );
}

/// The layout earns its place: a dense tape packs well under the legacy width.
#[test]
fn a_dense_tape_packs_under_a_quarter_of_the_legacy_width() {
    let ticks: Vec<Tick> = (0..10_000)
        .map(|i| {
            tick(
                1_700_000_000_000 + i * 37 % 1_000 + i * 50,
                100.0 + (i % 5) as f32 * 0.01,
                [0.001, 0.25, 1.5, 12.0][(i % 4) as usize],
                if i % 7 < 3 { Side::Sell } else { Side::Buy },
            )
        })
        .collect();
    let packed = encode(&ticks).len();
    let legacy = encode_legacy(&ticks).len();
    assert!(
        packed * 4 < legacy,
        "{packed} packed against {legacy} legacy bytes"
    );
}

/// The legacy reader still reads what an older build wrote, and ignores a torn tail.
#[test]
fn the_legacy_rows_read_back_and_a_torn_tail_is_ignored() {
    let rows = [
        tick(1_005, 1.5, 2.25, Side::Sell),
        tick(1_005 + 90 * DAY_MS, 1.5, 2.25, Side::Buy),
    ];
    let mut blob = encode_legacy(&rows);
    assert_eq!(blob.len(), 2 * LEGACY_ROW_BYTES);
    blob.extend_from_slice(&[1, 2, 3]);
    assert!(same(&decode_legacy(&blob), &rows));
}

#[test]
fn zigzag_round_trips_the_extremes() {
    for v in [0, 1, -1, i64::MAX, i64::MIN, 123_456_789, -987_654_321] {
        assert_eq!(unzigzag(zigzag(v)), v);
        let mut out = Vec::new();
        put_varint(&mut out, zigzag(v));
        assert_eq!(get_varint(&out), Some((zigzag(v), out.len())));
    }
}
