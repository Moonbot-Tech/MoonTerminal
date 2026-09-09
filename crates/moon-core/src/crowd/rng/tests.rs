use super::*;

#[test]
fn the_same_seed_replays_the_same_numbers() {
    let mut a = Rng::new(7);
    let mut b = Rng::new(7);
    for _ in 0..32 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}

#[test]
fn a_zero_seed_still_produces_values() {
    let mut rng = Rng::new(0);
    assert_ne!(rng.next_u64(), 0);
}

#[test]
fn random_floats_stay_inside_their_range() {
    let mut rng = Rng::new(11);
    for _ in 0..512 {
        let value = rng.next_f32();
        assert!((0.0..1.0).contains(&value), "{value} out of range");
        let ranged = rng.range(-3.0, 5.0);
        assert!((-3.0..5.0).contains(&ranged), "{ranged} out of range");
    }
}

#[test]
fn an_empty_range_collapses_to_its_low_end() {
    let mut rng = Rng::new(3);
    assert_eq!(rng.range(4.0, 4.0), 4.0);
    assert_eq!(rng.range(9.0, 2.0), 9.0);
}
