use super::worse_pct;
use moon_core::db::tuner::ticks::search::DEFAULT_WORSE_PCT;

/// An empty or unreadable box is the default; a typed per cent, comma or point, is itself; a
/// negative one is no limit the search can hold, and falls back too.
#[test]
fn a_box_reads_its_per_cent_or_the_default() {
    assert_eq!(worse_pct(""), DEFAULT_WORSE_PCT);
    assert_eq!(worse_pct("abc"), DEFAULT_WORSE_PCT);
    assert_eq!(worse_pct("-5"), DEFAULT_WORSE_PCT);
    assert_eq!(worse_pct("0"), 0.0);
    assert_eq!(worse_pct("12,5"), 12.5);
    assert_eq!(worse_pct(" 30 "), 30.0);
}
