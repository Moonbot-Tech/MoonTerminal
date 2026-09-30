use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

fn disk(free_gib: f64, total_gib: u64) -> Disk {
    Disk {
        free_bytes: (free_gib * GIB as f64) as u64,
        total_bytes: total_gib * GIB,
    }
}

/// The reserve is the larger of 2 GiB and a tenth: the test VPS's 23 GiB keeps 2.3 GiB, a
/// 10 GiB disk still keeps 2 GiB, a 200 GiB one 20 GiB.
#[test]
fn the_reserve_is_the_larger_of_two_gib_and_a_tenth() {
    assert_eq!(reserve(10 * GIB), 2 * GIB);
    assert_eq!(reserve(20 * GIB), 2 * GIB);
    assert_eq!(reserve(23 * GIB), 23 * GIB / 10);
    assert_eq!(reserve(200 * GIB), 20 * GIB);
    assert_eq!(reserve(0), 2 * GIB);
}

/// A disk above its reserve asks for nothing; one below asks for exactly what is missing, so an
/// eviction that shrinks the file by it ends the shortfall rather than starting another.
#[test]
fn the_shortfall_is_what_the_reserve_misses() {
    assert_eq!(shortfall(disk(14.7, 23)), None);
    assert_eq!(
        shortfall(Disk {
            free_bytes: 23 * GIB / 10,
            total_bytes: 23 * GIB
        }),
        None
    );
    assert_eq!(shortfall(disk(2.0, 23)), Some(23 * GIB / 10 - 2 * GIB));
    assert_eq!(shortfall(disk(0.0, 10)), Some(2 * GIB));
    // A mount that reports no size is nothing to evict for.
    assert_eq!(shortfall(disk(0.0, 0)), None);
    assert_eq!(
        shortfall(Disk {
            free_bytes: 2 * GIB,
            total_bytes: 10 * GIB
        }),
        None
    );
}
