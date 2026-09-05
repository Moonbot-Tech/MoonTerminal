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

// --- Goal A: header/toolbar chrome polish -----------------------------------------------------
//
// AUTHOR MODE: the four names below (`CHROME_RULE_H`, `readout_color`, `chrome_toggle_tone`,
// `chrome_toggle_label_color`) do not exist in `design.rs` yet — the implementation is being
// typed in a different worktree while this file is authored. The three tests below use the first
// three of those names and are therefore EXPECTED NOT TO COMPILE against today's tree; that is
// the documented AUTHOR-mode state, not a defect in the test. `chrome_toggle_label_color` has no
// test of its own here because it is not one of the five named contract-level breakages in this
// packet (it is a supporting symbol `chrome/quiet.rs` calls, covered indirectly by the
// `theme_contract` toggle-tone check instead).

use super::{CHROME_RULE_H, HEADER_TOP_H, TOOLBAR_H, chrome_toggle_tone, readout_color};
use moon_core::config::{UI_FONT_DELTA_MAX, UI_FONT_DELTA_MIN};
use moon_ui::{MoonPalette, MoonThemeTokens};

/// `design::readout_color` is the ONE place a toolbar readout decides muted-vs-present.
///
/// Breakage this pins: `readout_color`'s two arms swapped, or a call site reverting to a bare
/// `p.text` regardless of presence — the whole of acceptance item A2. Either turns the bare `–`
/// shown for an unreported leverage, an unknown exchange max order or an unknown manual stop into
/// a colour indistinguishable from a live figure, so a trader reads "no value" as a real one.
#[test]
fn readout_color_is_muted_when_absent_and_full_strength_when_present() {
    let p = MoonPalette::LIGHT;
    assert_eq!(
        readout_color(p, false),
        p.text_muted,
        "an absent readout must render text_muted"
    );
    assert_eq!(
        readout_color(p, true),
        p.text,
        "a present readout must render full text"
    );
    assert_ne!(
        readout_color(p, false),
        readout_color(p, true),
        "the two arms must not collapse onto the same colour"
    );
}

/// `CHROME_RULE_H` must stay strictly under the shorter of the header and toolbar bands at EVERY
/// supported font delta (`UI_FONT_DELTA_MIN..=UI_FONT_DELTA_MAX`), or the seam
/// `design::chrome_divider` draws paints outside the chrome strip and over the dock border below
/// it — and since `chrome_divider` has ~11 consumers app-wide, that lands in every one of them at
/// once.
///
/// Breakage this pins: raising `CHROME_RULE_H` to make the seam more visible without checking the
/// narrowest band (at `UI_FONT_DELTA_MIN` both bands are 32px; the token is 20).
///
/// Computed directly against `MoonThemeTokens::fit_height` rather than through
/// `design::header_height`/`design::toolbar_height`, because those two need a live `App` this
/// unit test does not have — `fit_height` is the pure formula both delegate to, so mirroring their
/// exact base-height/base-line-height/base-pad-y triples here is genuinely equivalent, not an
/// approximation.
#[test]
fn chrome_rule_h_stays_under_both_chrome_bands_at_every_font_delta() {
    let mut tokens = MoonThemeTokens::default();
    let mut delta = UI_FONT_DELTA_MIN;
    while delta <= UI_FONT_DELTA_MAX {
        tokens.scale.font_delta = delta as f32;
        let header = tokens.fit_height(HEADER_TOP_H, 14.0, 9.0);
        let toolbar = tokens.fit_height(TOOLBAR_H, 13.0, 9.5);
        assert!(
            CHROME_RULE_H < header.min(toolbar),
            "font delta {delta}: CHROME_RULE_H {CHROME_RULE_H} must stay under header {header} \
             and toolbar {toolbar}"
        );
        delta += 1;
    }
}

/// `design::chrome_toggle_tone` is the one truth table every chrome toggle (Sleep, own-trade, SL)
/// must share, so "amber" keeps meaning "this one is a caution state" everywhere it appears — the
/// whole of acceptance item A3.
///
/// Breakage this pins: a fourth chrome toggle hand-passing `.tone(MoonTone::Warning)`, or one of
/// the three existing ones dropping the call and naming a tone itself.
#[test]
fn chrome_toggle_tone_is_warning_only_when_on_and_caution() {
    use moon_ui::MoonTone;

    assert_eq!(chrome_toggle_tone(true, true), MoonTone::Warning);
    assert_eq!(chrome_toggle_tone(true, false), MoonTone::Info);
    assert_eq!(chrome_toggle_tone(false, true), MoonTone::Info);
    assert_eq!(chrome_toggle_tone(false, false), MoonTone::Info);
}
