//! Regression coverage for exact batched text measurement.

use super::{FontId, FontWeight, MonoGlyphWidthCache, wrap_text};
use std::collections::HashMap;

/// Removing the glyph cache in `design::MonoGlyphWidthCache::text_width` must fail the lookup
/// count: a wide Report would otherwise repeat one text-system call per cell character.
#[test]
fn batched_glyph_cache_matches_uncached_width_with_one_lookup_per_tuple() {
    let normal = (FontId(11), FontWeight::NORMAL);
    let semibold = (FontId(29), FontWeight::SEMIBOLD);
    let samples = [
        (normal, "BTCBTC"),
        (normal, "BTC\u{0416}"),
        (semibold, "BTC"),
        (semibold, "\u{0416}BTC"),
    ];
    let width_of = |font_id: FontId, weight: FontWeight, character: char| {
        font_id.0 as f32 * 0.125 + weight.0 * 0.01 + character as u32 as f32 * 0.0001
    };
    let expected: Vec<f32> = samples
        .iter()
        .map(|((font_id, weight), text)| {
            text.chars()
                .map(|character| width_of(*font_id, *weight, character))
                .sum()
        })
        .collect();

    let mut cache = MonoGlyphWidthCache::default();
    let mut lookups = HashMap::new();
    let actual: Vec<f32> = samples
        .iter()
        .map(|((font_id, weight), text)| {
            cache.text_width(*font_id, *weight, text, |font_id, weight, character| {
                *lookups
                    .entry((font_id, weight.0.to_bits(), character))
                    .or_insert(0usize) += 1;
                width_of(font_id, weight, character)
            })
        })
        .collect();

    assert_eq!(
        actual, expected,
        "cached sums preserve exact uncached widths"
    );
    assert_eq!(
        lookups.len(),
        8,
        "the two weights retain separate Latin and Unicode glyph identities"
    );
    assert!(
        lookups.values().all(|count| *count == 1),
        "each distinct font, weight, and character tuple is looked up once"
    );
}

/// A detect line is prose: it wraps onto the next line instead of losing everything after the cut.
#[test]
fn wrapping_keeps_what_a_cut_would_have_thrown_away() {
    // One unit per character, so a budget IS a character count.
    let measure = |s: &str| s.chars().count() as f32;
    let text = "SpreadDetection: Spread: TD: 40% TD2: 4% dP: 2.3% Vol: 2.3 k Trades: 1";

    let lines = wrap_text(text, 30.0, 3, measure);
    assert!(lines.len() > 1, "{lines:?}");
    for (line, w) in &lines {
        assert!(*w <= 30.0, "line over budget: {line:?} {w}");
        assert!(!line.starts_with(' '), "{line:?}");
    }
    // Every word survives somewhere, in order — that is what wrapping buys over cutting.
    let joined = lines
        .iter()
        .map(|(l, _)| l.trim_end_matches('…'))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.starts_with("SpreadDetection:"), "{joined:?}");
    assert!(joined.contains("dP: 2.3%"), "{joined:?}");

    // What still does not fit is cut into the LAST line, with the ellipsis at the end of the block.
    let short = wrap_text(text, 20.0, 2, measure);
    assert_eq!(short.len(), 2, "{short:?}");
    assert!(short[1].0.ends_with('…'), "{short:?}");

    // Text that fits is one line and is not touched.
    let whole = wrap_text("short", 30.0, 3, measure);
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0].0, "short");
}

/// A single word longer than the line has nowhere to break: it is cut, and the block ends there.
#[test]
fn an_unbreakable_word_is_cut_rather_than_looped() {
    let measure = |s: &str| s.chars().count() as f32;
    let lines = wrap_text("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", 10.0, 3, measure);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].0.ends_with('…'));

    // A line too narrow even for the FIRST word behaves the same way: one cut line, and the block
    // ends there rather than splitting a word — or a figure — down the middle.
    let narrow = wrap_text("SpreadDetection: TD: 40%", 12.0, 3, measure);
    assert_eq!(narrow.len(), 1, "{narrow:?}");

    // And zero lines is a legitimate ask: it prints nothing rather than looping.
    assert!(wrap_text("anything", 10.0, 0, measure).is_empty());
}
