// `use super::*` is banned here: the parent chain re-exports `gpui::*`, whose own `test` attribute
// shadows the built-in one and makes `#[test]` expand recursively.
use crate::panels::news::unread::{BUCKETS, scan};
use moon_core::config::NewsTagSettings;
use moon_core::feed::NewsItem;

/// A clock far enough ahead of the timestamps used here that the future-skew guard never trips.
const NOW: i64 = 1_000_000;

/// Build an item published at `time_ms` carrying `tags`.
fn item(time_ms: i64, tags: &[&str]) -> NewsItem {
    NewsItem {
        time_ms,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        ..NewsItem::default()
    }
}

/// Settings with the given palette colours assigned to the given tags.
fn colored(pairs: &[(&str, &str)]) -> NewsTagSettings {
    let mut s = NewsTagSettings::default();
    for (tag, color) in pairs {
        s.set_color(tag, Some(color));
    }
    s
}

/// Catches counting a multi-coloured item into only its first colour: the card's left rail paints
/// every colour the item carries, so a tab counter that picks one would disagree with the card the
/// user opens to check it.
#[test]
fn an_item_counts_into_every_colour_it_carries() {
    let settings = colored(&[("hack", "red"), ("listing", "green")]);
    let counts = scan(&[item(10, &["hack", "listing"])], 0, NOW, &settings).counts;
    let red = counts[0];
    let green = counts[2];
    assert_eq!((red, green), (1, 1));
    assert_eq!(counts.iter().sum::<usize>(), 2);
}

/// Catches dropping the per-item dedup: two tags of the same colour on one news must raise that
/// colour once, or a heavily tagged item alone reads as a burst of unread news.
#[test]
fn two_tags_of_one_colour_count_once() {
    let settings = colored(&[("hack", "red"), ("exploit", "red")]);
    let counts = scan(&[item(10, &["hack", "exploit"])], 0, NOW, &settings).counts;
    assert_eq!(counts[0], 1);
}

/// Catches comparing against the watermark with `>=` or ignoring it: the item at the watermark was
/// the newest one already seen, so it must not be reported again on every repaint.
#[test]
fn items_at_or_below_the_watermark_are_read() {
    let settings = NewsTagSettings::default();
    let items = [item(100, &[]), item(200, &[]), item(300, &[])];
    let s = scan(&items, 200, NOW, &settings);
    assert_eq!(s.counts[BUCKETS - 1], 1);
    assert_eq!(s.total, 1);
}

/// Catches skipping the tag filter: a topic switched off is hidden in the panel and on the chart,
/// so counting it would ring the tab for news the user can never see by opening it.
#[test]
fn a_hidden_topic_does_not_count() {
    let mut settings = colored(&[("hack", "red")]);
    settings.set_hidden("hack", true);
    let counts = scan(&[item(10, &["hack"])], 0, NOW, &settings).counts;
    assert_eq!(counts.iter().sum::<usize>(), 0);
}

/// Catches counting a hidden tag's colour because a sibling tag kept the item visible: switching a
/// topic off must remove it from the counters, not relabel it under the item's other colour.
#[test]
fn a_hidden_tag_contributes_nothing_to_a_still_visible_item() {
    let mut settings = colored(&[("hack", "red"), ("listing", "green")]);
    settings.set_hidden("hack", true);
    let counts = scan(&[item(10, &["hack", "listing"])], 0, NOW, &settings).counts;
    assert_eq!((counts[0], counts[2]), (0, 1));
}

/// Catches routing untagged news into a colour bucket: with no coloured tag there is no colour to
/// paint, and the neutral bucket is what the badge row renders muted.
#[test]
fn untagged_and_neutral_items_land_in_the_neutral_bucket() {
    let settings = colored(&[("hack", "red")]);
    let counts = scan(&[item(10, &[]), item(11, &["weather"])], 0, NOW, &settings).counts;
    assert_eq!(counts[BUCKETS - 1], 2);
    assert_eq!(counts[0], 0);
}

/// Catches summing the buckets for the merged badge: the buckets deliberately double-count a
/// multi-coloured item, so a merged total taken from them would exceed the number of unread news.
#[test]
fn the_merged_total_counts_items_not_bucket_hits() {
    let settings = colored(&[("hack", "red"), ("listing", "green")]);
    let items = [item(10, &["hack", "listing"])];
    let s = scan(&items, 0, NOW, &settings);
    assert_eq!(s.counts.iter().sum::<usize>(), 2);
    assert_eq!(s.total, 1);
}

/// Catches dropping the future-skew guard the chart's marks apply: a service stamp rescaled into
/// the far future would pin a badge that reading the panel can never clear, because the watermark
/// deliberately refuses to follow it there.
#[test]
fn a_stamp_beyond_the_skew_tolerance_is_not_counted() {
    let settings = NewsTagSettings::default();
    let items = [item(NOW + 5_000_000, &[])];
    assert_eq!(scan(&items, 0, NOW, &settings).total, 0);
}

/// Catches keying the counter on `time_ms` alone: an item delivered without a publication stamp is
/// still drawn on the chart through the same fallback chain, so the tab must count it too.
#[test]
fn an_item_without_a_publication_stamp_falls_back_to_its_service_time() {
    let settings = NewsTagSettings::default();
    let stampless = NewsItem {
        time_ms: 0,
        recv_time_ms: Some(500),
        ..NewsItem::default()
    };
    assert_eq!(scan(&[stampless], 0, NOW, &settings).total, 1);
}
