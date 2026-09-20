use super::*;

#[test]
fn one_drops_an_inverted_span() {
    assert!(Coverage::one((10, 5)).is_empty());
    assert_eq!(Coverage::one((5, 5)).spans(), &[(5, 5)]);
}

#[test]
fn add_coalesces_abutting_and_overlapping_spans_in_any_order() {
    let mut c = Coverage::none();
    c.add((20, 29));
    c.add((0, 9));
    assert_eq!(c.spans(), &[(0, 9), (20, 29)]);
    // Abutting on both sides: `9 + 1 == 10` and `19 + 1 == 20` bridge the two.
    c.add((10, 19));
    assert_eq!(c.spans(), &[(0, 29)]);
    c.add((25, 40));
    assert_eq!(c.spans(), &[(0, 40)]);
    c.add((50, 60));
    c.add((45, 48));
    assert_eq!(c.spans(), &[(0, 40), (45, 48), (50, 60)]);
    c.add((49, 49));
    assert_eq!(c.spans(), &[(0, 40), (45, 60)]);
}

#[test]
fn add_keeps_a_gap_of_one_millisecond_apart() {
    let mut c = Coverage::one((0, 9));
    c.add((11, 20));
    assert_eq!(c.spans(), &[(0, 9), (11, 20)]);
    assert!(!c.contains((9, 11)));
    assert!(!c.contains_ms(10));
}

#[test]
fn contains_is_per_span_never_over_the_hull() {
    let c = Coverage::from_spans([(0, 9), (20, 29)]);
    assert_eq!(c.hull(), Some((0, 29)));
    assert!(c.contains((0, 9)));
    assert!(c.contains((22, 25)));
    assert!(!c.contains((5, 25)));
    assert!(!c.contains((9, 20)));
    assert!(!c.contains((30, 31)));
    assert!(!c.contains((5, 3)));
    assert!(Coverage::none().hull().is_none());
}

#[test]
fn covers_asks_every_span_of_the_other() {
    let wide = Coverage::from_spans([(0, 100)]);
    let split = Coverage::from_spans([(0, 9), (20, 29)]);
    assert!(wide.covers(&split));
    assert!(!split.covers(&wide));
    assert!(split.covers(&Coverage::none()));
    assert!(Coverage::none().covers(&Coverage::none()));
    assert!(!Coverage::none().covers(&split));
}

#[test]
fn clip_intersects_each_span_with_each_bound() {
    let c = Coverage::from_spans([(0, 50), (60, 100)]);
    let bounds = Coverage::from_spans([(40, 70), (90, 200)]);
    assert_eq!(c.clip(&bounds).spans(), &[(40, 50), (60, 70), (90, 100)]);
    assert!(c.clip(&Coverage::none()).is_empty());
    assert!(c.clip(&Coverage::one((51, 59))).is_empty());
}

#[test]
fn width_sums_the_spans_inclusive() {
    assert_eq!(Coverage::from_spans([(0, 9), (20, 29)]).width_ms(), 20);
    assert_eq!(Coverage::none().width_ms(), 0);
    assert!(Coverage::from_spans([(0, 9), (20, 29)]).is_split());
    assert!(!Coverage::one((0, 9)).is_split());
}

#[test]
fn display_names_every_span() {
    assert_eq!(Coverage::none().to_string(), "-");
    assert_eq!(Coverage::one((1, 2)).to_string(), "1..2");
    assert_eq!(
        Coverage::from_spans([(1, 2), (5, 9)]).to_string(),
        "1..2+5..9"
    );
}
