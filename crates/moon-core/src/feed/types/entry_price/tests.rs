use super::*;

/// The wire's zero is "no limit on this side": reading it as a price would draw a range down to
/// zero, or up to it, that the exchange never reported.
#[test]
fn a_zero_bound_is_no_bound() {
    let only_min = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, 0.4, 0.0).unwrap();
    assert_eq!(only_min.min_price, Some(0.4));
    assert_eq!(only_min.max_price, None);
    let only_max = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, 0.0, 0.6).unwrap();
    assert_eq!(only_max.min_price, None);
    assert_eq!(only_max.max_price, Some(0.6));
    let neither = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, 0.0, 0.0).unwrap();
    assert_eq!((neither.min_price, neither.max_price), (None, None));
}

/// A garbage bound is dropped on its own, but a requested price that cannot be drawn drops the
/// whole notice: there is nothing to put the line at.
#[test]
fn unusable_values_are_not_prices() {
    let odd = EntryPriceCorrection::from_wire("XALPHAUSDT", 0.5, -1.0, f64::NAN).unwrap();
    assert_eq!((odd.min_price, odd.max_price), (None, None));
    for requested in [0.0, -0.5, f64::NAN, f64::INFINITY] {
        assert!(EntryPriceCorrection::from_wire("XALPHAUSDT", requested, 0.4, 0.6).is_none());
    }
    assert!(EntryPriceCorrection::from_wire("", 0.5, 0.4, 0.6).is_none());
}
