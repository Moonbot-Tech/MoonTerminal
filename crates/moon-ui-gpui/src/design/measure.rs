//! Glyph-width measuring and its cache.

use super::*;

/// Cache key for one glyph advance under an exact resolved font and requested weight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MonoGlyphKey {
    font_id: FontId,
    weight_bits: u32,
    character: char,
}

/// Per-batch glyph-advance cache shared across many measured strings.
#[derive(Default)]
pub(super) struct MonoGlyphWidthCache {
    widths: HashMap<MonoGlyphKey, f32>,
}

impl MonoGlyphWidthCache {
    /// Measure text by looking up each distinct font, weight, and character tuple once.
    ///
    /// Args:
    ///     font_id: Exact font resolved for the requested weight.
    ///     weight: Numeric GPUI font weight used to resolve `font_id`.
    ///     text: Unicode text whose per-character advances are summed in order.
    ///     lookup: Uncached glyph lookup used only for missing tuples.
    ///
    /// Returns:
    ///     The same ordered sum as uncached per-character measurement.
    pub(super) fn text_width(
        &mut self,
        font_id: FontId,
        weight: FontWeight,
        text: &str,
        mut lookup: impl FnMut(FontId, FontWeight, char) -> f32,
    ) -> f32 {
        text.chars()
            .map(|character| {
                let key = MonoGlyphKey {
                    font_id,
                    weight_bits: weight.0.to_bits(),
                    character,
                };
                *self
                    .widths
                    .entry(key)
                    .or_insert_with(|| lookup(font_id, weight, character))
            })
            .sum()
    }
}

/// Exact batched measurer for monospaced Report text at the terminal body size.
///
/// Normal cells and semibold headers resolve separate fonts up front. Repeated characters across
/// all columns and rows in one natural-width batch then share exact glyph advances without merging
/// different weights or fallback-resolved font identities.
pub(crate) struct MonoBodyTextMeasurer<'a> {
    text_system: &'a TextSystem,
    size: Pixels,
    family: SharedString,
    fonts: HashMap<u32, FontId>,
    glyphs: MonoGlyphWidthCache,
}

/// Exact resolved font identity used to invalidate cached Report measurements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MonoBodyFontSignature {
    /// Resolved normal-weight font.
    pub(crate) normal: FontId,
    /// Resolved semibold font used by table headers.
    pub(crate) semibold: FontId,
    /// Rendered body size encoded for exact floating-point identity.
    pub(crate) size_bits: u32,
}

impl<'a> MonoBodyTextMeasurer<'a> {
    /// Resolve the normal and semibold mono fonts for one Report width batch.
    ///
    /// Args:
    ///     cx: Application context providing theme font tokens and the text system.
    ///
    /// Returns:
    ///     A measurer with separate resolved fonts and an empty glyph cache.
    pub(crate) fn new(cx: &'a App) -> Self {
        let tokens = MoonTheme::active_tokens(cx);
        let size = px(tokens.font(base_text(cx)));
        let family = tokens.font_family(true);
        let text_system = cx.text_system();
        let mut fonts = HashMap::with_capacity(2);
        for weight in [FontWeight::NORMAL, FontWeight::SEMIBOLD] {
            let resolved = text_system.resolve_font(&Font {
                weight,
                ..font(family.clone())
            });
            fonts.insert(weight.0.to_bits(), resolved);
        }
        Self {
            text_system,
            size,
            family,
            fonts,
            glyphs: MonoGlyphWidthCache::default(),
        }
    }

    /// Return the resolved normal/semibold font and size identity for this batch.
    ///
    /// Returns:
    ///     Stable signature that changes when rendered Report typography changes.
    pub(crate) fn signature(&self) -> MonoBodyFontSignature {
        MonoBodyFontSignature {
            normal: self.fonts[&FontWeight::NORMAL.0.to_bits()],
            semibold: self.fonts[&FontWeight::SEMIBOLD.0.to_bits()],
            size_bits: self.size.as_f32().to_bits(),
        }
    }

    /// Measure one string with the exact resolved font for its weight.
    ///
    /// Args:
    ///     text: Unicode text to measure.
    ///     weight: Font weight used by the rendered Report text.
    ///
    /// Returns:
    ///     Sum of exact cached glyph advances in pixels.
    pub(crate) fn text_width(&mut self, text: &str, weight: FontWeight) -> f32 {
        let weight_bits = weight.0.to_bits();
        let font_id = if let Some(font_id) = self.fonts.get(&weight_bits).copied() {
            font_id
        } else {
            let font_id = self.text_system.resolve_font(&Font {
                weight,
                ..font(self.family.clone())
            });
            self.fonts.insert(weight_bits, font_id);
            font_id
        };
        let text_system = self.text_system;
        let size = self.size;
        self.glyphs
            .text_width(font_id, weight, text, |font_id, _, character| {
                f32::from(text_system.layout_width(font_id, size, character))
            })
    }
}

/// Resolve the exact font and rendered size one measurement will use.
///
/// The single place [`ui_text_width`] and [`text_metrics_key`] agree on how a request becomes a
/// font: a key derived from a hand-copied version of these three lines would keep validating stale
/// widths the day the resolution changes.
fn measure_font(cx: &App, base_font_size: f32, weight: f32, mono: bool) -> (FontId, Pixels) {
    let size = MoonTheme::active_tokens(cx).font(base_font_size);
    measure_font_at(cx, px(size), weight, mono)
}

/// Resolve the active theme font at an already rendered `size`.
fn measure_font_at(cx: &App, size: Pixels, weight: f32, mono: bool) -> (FontId, Pixels) {
    let tokens = MoonTheme::active_tokens(cx);
    let font = Font {
        weight: FontWeight(weight),
        ..font(tokens.font_family(mono))
    };
    (cx.text_system().resolve_font(&font), size)
}

/// Identity of the typography a text measurement was taken under.
///
/// [`ui_text_width`] has no cache of its own, so a caller retaining a measured width needs to know
/// when to throw it away. Keyed on the RESOLVED font rather than the requested family, like
/// [`MonoBodyFontSignature`]: the family is a theme token rather than the constant [`mono`], and a
/// fallback or font-availability change moves the resolution without moving the request.
///
/// Args:
///     cx: Application context providing theme tokens and the text system.
///     base_font_size: Unscaled base size the measurement was taken at.
///     weight: Numeric GPUI weight the measurement was taken at.
///     mono: Whether the measurement used the monospaced family.
///
/// Returns:
///     A digest that changes whenever a width measured under it would.
pub fn text_metrics_key(cx: &App, base_font_size: f32, weight: f32, mono: bool) -> u64 {
    let (font_id, size) = measure_font(cx, base_font_size, weight, mono);
    (font_id.0 as u64) << 32 | u64::from(size.as_f32().to_bits())
}

/// Identity of the typography a `tokens.ui`-channel text measurement was taken under.
///
/// The [`ui_text_width_zoomed`] partner of [`text_metrics_key`]: for text that follows only the UI
/// zoom, not the legacy font-delta channel — control-tier labels, and the terminal's own
/// `t_*` text after it moved onto the tier. Resolves through [`measure_font_at`] at
/// [`ui_px`]`(size)` instead of [`measure_font`].
///
/// Args:
///     cx: Application context providing theme tokens and the text system.
///     size: Design size before UI zoom.
///     weight: Numeric GPUI weight the measurement was taken at.
///     mono: Whether the measurement used the monospaced family.
///
/// Returns:
///     A digest that changes whenever a width measured under it would.
pub fn text_metrics_key_zoomed(cx: &App, size: f32, weight: f32, mono: bool) -> u64 {
    let (font_id, size) = measure_font_at(cx, ui_px(cx, size), weight, mono);
    (font_id.0 as u64) << 32 | u64::from(size.as_f32().to_bits())
}

/// Estimate text width at an unscaled base size using the active theme font.
///
/// Font scaling is applied internally, matching `MoonText`. The estimate sums per-character glyph
/// advances from `text_system::layout_width`, without kerning or ligatures. It is suitable for
/// content-driven geometry such as fitting a menu to its longest item, not pixel-exact text layout.
/// Measure with the family that renders the text: `mono = true` selects the monospaced family such
/// as Geist Mono, while `false` selects the UI family.
///
/// Args:
///     cx: Application context providing active tokens and the text system.
///     text: Text to measure.
///     base_font_size: Unscaled base size; passing an already scaled value scales twice.
///     weight: Font weight represented as the GPUI numeric value.
///     mono: Whether to use the theme's monospaced rather than UI font family.
///
/// Returns:
///     The summed glyph-advance estimate in pixels.
pub fn ui_text_width(cx: &App, text: &str, base_font_size: f32, weight: f32, mono: bool) -> f32 {
    glyph_advance_width(cx, text, measure_font(cx, base_font_size, weight, mono))
}

/// Estimate text width for text that follows only the UI zoom, not the legacy font-delta channel.
///
/// For MoonUI controls whose tiers fix their text size, such as the `Sm`/`Md` `MoonCheckbox`
/// label: that text renders at `ui(base_font_size)` whatever the legacy font-delta channel adds,
/// so measuring it through [`ui_text_width`] would over-reserve by that delta. Same estimate
/// otherwise.
///
/// Args:
///     cx: Application context providing active tokens and the text system.
///     text: Text to measure.
///     base_font_size: Design size before UI zoom.
///     weight: Font weight represented as the GPUI numeric value.
///     mono: Whether to use the theme's monospaced rather than UI font family.
///
/// Returns:
///     The summed glyph-advance estimate in pixels.
pub fn ui_text_width_zoomed(
    cx: &App,
    text: &str,
    base_font_size: f32,
    weight: f32,
    mono: bool,
) -> f32 {
    let size = ui_px(cx, base_font_size);
    glyph_advance_width(cx, text, measure_font_at(cx, size, weight, mono))
}

/// Sum the cached glyph advances of `text` in one resolved font and size.
fn glyph_advance_width(cx: &App, text: &str, (font_id, size): (FontId, Pixels)) -> f32 {
    // Counted, because the callers run this every frame: see `diag::UI_TEXT_WIDTH_CALLS`.
    let measured = crate::diag::timer();
    crate::diag::bump(&crate::diag::UI_TEXT_WIDTH_CALLS);
    crate::diag::bump_by(
        &crate::diag::UI_TEXT_WIDTH_CHARS,
        text.chars().count() as u64,
    );
    let size_bits = size.as_f32().to_bits();
    let ts = cx.text_system();
    let width: f32 = GLYPH_ADVANCE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= GLYPH_ADVANCE_MAX {
            cache.clear();
        }
        text.chars()
            .map(|ch| {
                *cache.entry((font_id, size_bits, ch)).or_insert_with(|| {
                    crate::diag::bump(&crate::diag::UI_TEXT_WIDTH_MISS);
                    f32::from(ts.layout_width(font_id, size, ch))
                })
            })
            .sum()
    });
    crate::diag::record_us(&crate::diag::UI_TEXT_WIDTH_US, measured);
    width
}

/// One glyph advance, remembered.
///
/// `App::text_system()` hands back the process-wide `TextSystem`, whose `layout_width` calls the
/// platform shaper directly — the CACHED one (`line_layout_cache`) belongs to `WindowTextSystem`,
/// which this function has no handle on. So every character cost a full DirectWrite/CoreText line
/// shaping, measured at roughly 10 µs each: the toolbar's label ladder alone made 72 of those calls
/// per repaint, and the window repaints on every frame it draws.
///
/// Keyed on `(FontId, size bits, char)`, which fully determines the answer: weight and family are
/// resolved INTO the font id by [`measure_font`], and the size is the scaled pixel value. So there
/// is nothing to invalidate — a theme or font-slider change simply asks about a different key.
/// GPUI hands out font ids from a monotonic table and never reassigns one, so a remembered id
/// cannot come to name a different font later.
///
/// Deliberately per GLYPH rather than per string. Shaping each string once would be fewer platform
/// calls, but it would apply kerning and return a different number, and the per-character sum is
/// depended upon AS SUCH — see `chrome::clock`, which documents it as "glyph advances only, no
/// kerning". A glyph memo leaves every existing width bit-identical.
///
/// The cap exists only so an unbounded run of exotic text cannot grow this without limit; the real
/// working set is one alphabet per typography, a few hundred entries. Clearing wholesale rather
/// than evicting is fine at that size — it costs one cold frame.
const GLYPH_ADVANCE_MAX: usize = 8192;

thread_local! {
    static GLYPH_ADVANCE: std::cell::RefCell<HashMap<(FontId, u32, char), f32>> =
        std::cell::RefCell::new(HashMap::new());
}
