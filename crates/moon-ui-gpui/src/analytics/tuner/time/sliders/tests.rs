use super::{dragged_range, excluded_bands, selected_bands, slider_bound_values};

/// Reordering or clamping against the opposite handle collapses the new wrapping suggestions.
#[test]
fn circular_handles_preserve_wraps_on_all_axes() {
    for max in [59, 1439, 10079] {
        assert_eq!(
            dragged_range((max - 4, 5), true, max - 5, max),
            (max - 5, 5)
        );
        assert_eq!(dragged_range((max - 4, 5), false, 6, max), (max - 4, 6));
        assert_eq!(dragged_range((max - 4, 5), true, 4, max), (4, 5));
        assert_eq!(
            selected_bands(max - 4, 5, max),
            vec![(max - 4, max), (0, 5)]
        );
        assert_eq!(selected_bands(4, 5, max), vec![(4, 5)]);
    }
    assert_eq!(
        slider_bound_values(0, 10074, 5),
        Some(("7.23:54".into(), "1.00:05".into()))
    );
    assert_eq!(
        slider_bound_values(1, 1380, 1045),
        Some(("23:00".into(), "17:25".into()))
    );
    assert_eq!(
        slider_bound_values(2, 55, 5),
        Some(("55".into(), "5".into()))
    );
    for (field, max) in [(0, 10079), (1, 1439), (2, 59)] {
        assert_eq!(slider_bound_values(field, 0, max), None);
    }
}

/// Wrapped finer fields exclude the middle gap, never both included ends, on every cycle.
#[test]
fn circular_cascade_shades_only_the_complement() {
    assert_eq!(excluded_bands(55, 5, 60, 120), vec![(6, 54), (66, 114)]);
    assert_eq!(
        excluded_bands(1380, 1045, 1440, 10080),
        (0..7)
            .map(|day| (day * 1440 + 1046, day * 1440 + 1379))
            .collect::<Vec<_>>()
    );
    assert_eq!(excluded_bands(10, 20, 60, 60), vec![(0, 9), (21, 59)]);
    assert!(excluded_bands(0, 59, 60, 1440).is_empty());
}
