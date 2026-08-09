//! Regression coverage for exact batched text measurement.

use super::{FontId, FontWeight, MonoGlyphWidthCache};
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
