//! Regression coverage for exact batched text measurement and for the brand assets.

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

/// The brand cuts: scheme name, asset source, and the wordmark colour that cut must carry.
const BRAND_CUTS: [(&str, &str, &str); 2] = [
    ("dark", super::LOGO_SVG_DARK, "#E7E7E7"),
    ("light", super::LOGO_SVG_LIGHT, "#17202A"),
];

/// Both cuts must survive [`super::logo_paths`], which splices them into the glow document.
///
/// The extraction is string surgery on an asset nobody rebuilds when they re-export it, and its
/// failure is SILENT at runtime: an empty result draws an aura with no mark, indistinguishable
/// from a decode failure. So the check belongs here rather than in a bug report.
#[test]
fn brand_cuts_survive_the_glow_extraction() {
    for (name, svg, _) in BRAND_CUTS {
        let inner = super::logo_paths(svg);
        assert!(inner.contains("<path"), "{name} cut lost its paths");
        assert!(
            !inner.contains("<svg"),
            "{name} cut kept its root tag, which cannot nest in the glow document"
        );
    }
}

/// Each cut must match the geometry constants that place it and carry its own scheme's wordmark.
///
/// `LOGO_SRC_W`/`LOGO_SRC_H` centre the mark in the glow frame and scale every lockup width, so a
/// re-export at another viewBox leaves them describing the previous drawing — which no compiler
/// and no eye catches until the mark sits off-centre. The colours are the other half of the same
/// contract: picking a FILE per scheme is what keeps a custom palette from repainting the brand,
/// and swapping the two files is invisible until someone looks at the header in one theme.
#[test]
fn brand_cuts_match_their_geometry_and_scheme() {
    for (name, svg, wordmark) in BRAND_CUTS {
        let view_box = svg
            .split_once("viewBox=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(value, _)| value)
            .unwrap_or_else(|| panic!("{name} cut has no viewBox"));
        assert_eq!(
            view_box,
            format!("0 0 {} {}", super::LOGO_SRC_W, super::LOGO_SRC_H),
            "{name} cut was re-exported at a different size than design.rs assumes"
        );
        assert!(
            svg.contains(wordmark),
            "{name} cut lost the wordmark colour its scheme is picked for"
        );
    }
}
