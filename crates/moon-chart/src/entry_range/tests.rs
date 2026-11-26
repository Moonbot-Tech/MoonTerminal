use super::*;

const MS: fn(u64) -> Duration = Duration::from_millis;

/// A notice is drawn for exactly [`SHOW`]: at the last nanosecond before it, and never at it. A
/// notice that outlived its time would keep a chart repainting on the timer that waits for it.
#[test]
fn expires_at_show() {
    assert!(plan(0.5, Some(0.4), Some(0.45), SHOW - Duration::from_nanos(1)).is_some());
    assert!(plan(0.5, Some(0.4), Some(0.45), SHOW).is_none());
    assert!(plan(0.5, Some(0.4), Some(0.45), SHOW + MS(1)).is_none());
}

/// One limit leaves that side open to the plot's edge; none at all draws no band (the requested
/// line still shows); limits that cross draw no band.
#[test]
fn bounds_map_to_the_band() {
    let at = MS(0);
    assert_eq!(
        plan(0.5, Some(0.4), Some(0.45), at).unwrap().band,
        Some((0.4, 0.45))
    );
    assert_eq!(
        plan(0.5, Some(0.4), None, at).unwrap().band,
        Some((0.4, f64::INFINITY))
    );
    assert_eq!(
        plan(0.5, None, Some(0.45), at).unwrap().band,
        Some((f64::NEG_INFINITY, 0.45))
    );
    let none = plan(0.5, None, None, at).unwrap();
    assert_eq!(none.band, None);
    assert_eq!(none.requested, 0.5);
    assert_eq!(plan(0.5, Some(0.45), Some(0.4), at).unwrap().band, None);
    assert!(plan(0.0, Some(0.4), Some(0.45), at).is_none());
    assert!(plan(f64::NAN, Some(0.4), Some(0.45), at).is_none());
}

/// The fade is a fixed ladder: every moment inside one step draws the same opacity, the next
/// change is the step's end, and the last step's end is the expiry. Two moments in one step must
/// compare equal, or every timer wake would rebuild the layer for a picture that did not change.
#[test]
fn fade_steps_down_to_expiry() {
    let step = SHOW / FADE_STEPS;
    let first = plan(0.5, Some(0.4), Some(0.45), MS(0)).unwrap();
    assert_eq!(
        (first.step, first.opacity, first.next_change),
        (0, 1.0, step)
    );
    assert_eq!(
        plan(0.5, Some(0.4), Some(0.45), step - Duration::from_nanos(1)).unwrap(),
        first
    );
    let second = plan(0.5, Some(0.4), Some(0.45), step).unwrap();
    assert_eq!(second.step, 1);
    assert!(second.opacity < first.opacity);
    let last = plan(0.5, Some(0.4), Some(0.45), SHOW - MS(1)).unwrap();
    assert_eq!(last.step, FADE_STEPS - 1);
    assert_eq!(last.next_change, SHOW);
    assert!(last.opacity > 0.0);
}

/// The band fades with the line, at its own lower ceiling, and the line is sized in logical
/// pixels.
#[test]
fn instances_carry_the_fade() {
    let p = plan(0.5, Some(0.4), Some(0.45), SHOW / 2).unwrap();
    let visible = (0.3, 0.6);
    let (band, line) = instances(&p, visible, [0, 255, 0], [255, 0, 0], 2.0);
    let band = band.unwrap();
    assert_eq!((band.price0, band.price1), (0.4, 0.45));
    assert!((band.color[3] - BAND_OPACITY * p.opacity).abs() < 1e-6);
    assert_eq!(line.price, 0.5);
    assert!((line.color[3] - p.opacity).abs() < 1e-6);
    assert_eq!(line.thickness, LINE_PX * 2.0);
    let none = plan(0.5, None, None, MS(0)).unwrap();
    let (no_band, _) = instances(&none, visible, [0; 3], [0; 3], 1.0);
    assert!(no_band.is_none());
}

/// An open side runs past the visible edge on its side — so it fills to the plot's edge — but by
/// a few windows only: a huge sentinel price there is what the zone shaders cannot carry (they
/// clamp a band's time to the plot, never its price), and its far edge would cancel to zero in
/// `f32`. The bound must stay finite and ordered on every input, a window not laid out included.
#[test]
fn an_open_side_ends_a_few_windows_past_the_edge() {
    let up = plan(0.5, Some(0.4), None, MS(0)).unwrap();
    let (band, _) = instances(&up, (0.3, 0.6), [0; 3], [0; 3], 1.0);
    let band = band.unwrap();
    assert_eq!(band.price0, 0.4);
    assert!(band.price1 > 0.6, "past the top of the plot");
    assert!(f64::from(band.price1) <= 0.6 + OPEN_SIDE_WINDOWS * 0.3 + 1e-6);
    let down = plan(0.5, None, Some(0.45), MS(0)).unwrap();
    let (band, _) = instances(&down, (0.3, 0.6), [0; 3], [0; 3], 1.0);
    let band = band.unwrap();
    assert!(band.price0 < 0.3, "past the bottom of the plot");
    assert!(f64::from(band.price0) >= 0.3 - OPEN_SIDE_WINDOWS * 0.3 - 1e-6);
    // A bound beyond the visible window still gets a band that reaches past it.
    let (band, _) = instances(&up, (0.1, 0.2), [0; 3], [0; 3], 1.0);
    assert!(band.unwrap().price1 > 0.4);
    // A view not fitted yet is born at 0 ± 0.5: on a coin priced 60000 the open side must still
    // run far past the price, not stop four units above it.
    let high = plan(60_000.0, Some(59_900.0), None, MS(0)).unwrap();
    let (band, _) = instances(&high, (-0.5, 0.5), [0; 3], [0; 3], 1.0);
    assert!(band.unwrap().price1 >= 89_000.0);
    for visible in [(0.0, 0.0), (f64::NAN, 1.0), (0.6, 0.3)] {
        let (band, _) = instances(&up, visible, [0; 3], [0; 3], 1.0);
        let band = band.unwrap();
        assert!(
            band.price1.is_finite() && band.price1 > band.price0,
            "{visible:?}"
        );
    }
}

/// The caption fades with the picture: full colour at arrival, mixed toward the background as the
/// steps pass, never all the way to it before the expiry removes it.
#[test]
fn the_caption_fades_toward_the_background() {
    let text = [200, 40, 40];
    let bg = [20, 20, 20];
    let first = plan(0.5, None, None, MS(0)).unwrap();
    assert_eq!(caption_rgb(&first, text, bg), 0x00C8_2828);
    let last = plan(0.5, None, None, SHOW - MS(1)).unwrap();
    let faded = caption_rgb(&last, text, bg);
    assert_ne!(faded, 0x00C8_2828);
    assert_ne!(faded, 0x0014_1414);
    assert!((faded >> 16) & 0xFF < 200);
}
