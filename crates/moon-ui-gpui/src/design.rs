//! Moonbot terminal design tokens extracted from the Moonbot Terminal design reference.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_core::util::fmt::DeltaSign;
use moon_ui::{MoonMetrics, MoonPalette, MoonTheme, MoonTone, rgba_from};
use std::collections::HashMap;
use std::sync::Arc;

const M: MoonMetrics = MoonMetrics::TERMINAL;

/// The one profit-and-loss sign-to-tone mapping: gain green, loss red, and a value that ROUNDED to
/// zero muted rather than green.
///
/// Every money cell resolves its colour here so no surface can tint the same figure differently.
/// Taking a [`DeltaSign`] rather than an `f64` is the load-bearing part: the sign is classified
/// from the value AFTER rounding, so a `-0.004` that prints as `0.00` can no longer arrive here
/// still claiming to be a loss and paint a red cell that reads as break-even.
pub fn delta_tone(sign: DeltaSign) -> MoonTone {
    sign.pick(MoonTone::Positive, MoonTone::Danger, MoonTone::Muted)
}

pub const HEADER_TOP_H: f32 = M.header_top_h;
pub const TOOLBAR_H: f32 = M.toolbar_h;
pub const STATUS_H: f32 = M.status_h;
pub const TABLE_HEAD_H: f32 = M.table_header_h;
pub const TABLE_ROW_H: f32 = M.table_row_h;
pub const HEADER_PAD_X: f32 = 12.0;

/// One spacing rule across both chrome strips: 8px inside a group, 8px + rule + 8px between
/// groups.
///
/// The header and the toolbar are one visual block on one `shell_high` background, so a gap that
/// differs between them reads as a seam. [`chrome_divider`] is this token's partner — the group
/// boundary comes from the RULE, not from extra space, which is why the same value serves both
/// positions.
pub const CHROME_GAP: f32 = 8.0;

/// Width MoonUI's `MoonVirtualList` vertical overlay scrollbar covers at its container's right
/// edge.
///
/// MIRRORS MoonUI: `moon/virtual_list.rs` builds every `MoonVirtualList`'s track via
/// `moon_scrollbar_overlay_with_palette` (`moon/scroll_area.rs`), which draws it as
/// `.absolute().right(px(0.0)).w(px(tokens.ui(8.0)))` — an OVERLAY that reserves no layout width of
/// its own. A `w_full` row inside such a list must therefore subtract this by hand on its
/// right-justified content, or that content ends up drawn under the track.
///
/// The value is in DESIGN UNITS, so apply it through [`ui_px`], never as a raw `px`. That constant
/// is private in MoonUI, so nothing checks this mirror; if it moves there, this must follow by
/// hand. This is NOT `scroll/scrollbar.rs`'s legacy `Scrollbar` (`WIDTH = 4.*2. + 8.` = 16.0) —
/// that implementation is not on the `MoonVirtualList` call path, so do not "correct" this to 16.0.
pub const MOON_SCROLLBAR_OVERLAY_W: f32 = 8.0;

/// Base (unscaled) glyph edge for an interactive disclosure caret built with
/// `MoonDisclosure::button`.
///
/// Choose among the three shared caret idioms by who owns the click: an enclosing element that owns
/// it hosts a passive `MoonDisclosure::glyph` sized by [`DISCLOSURE_GLYPH_MARKER`]; a caret that IS
/// the control is `MoonDisclosure::button` at this size; and a control that must keep button chrome
/// puts `MoonButtonIconSlot::caret` in the button's leading icon slot. `MoonButton` overrides that
/// slot's default size from its own text metrics, so none of these disclosure-size constants
/// controls it. Do not replace these shared carets with unicode glyphs.
///
/// Pass this value directly to `MoonDisclosure`: its `caret_box` applies `tokens.ui(...)`, so
/// passing [`ui_px`] would apply the UI scale twice. The caret therefore follows the UI slider,
/// while raw text sized through [`t_body`] follows the Font slider. This matches chrome such as
/// [`vline`].
pub const DISCLOSURE_GLYPH: f32 = 11.0;

/// Base (unscaled) glyph edge for a passive disclosure caret whose enclosing row owns the click.
///
/// Pass this value directly to `MoonDisclosure`; its `caret_box` applies the UI scale. This keeps
/// the marker on the UI slider rather than the Font slider used by [`t_body`].
pub const DISCLOSURE_GLYPH_MARKER: f32 = 9.0;

/// Base (unscaled) square box around either disclosure caret.
///
/// Pass this value directly to `MoonDisclosure`; its `caret_box` applies the UI scale. Sharing the
/// value keeps a caret aligned with a neighbouring `glyph_btn` cell in the same toolbar row.
pub const DISCLOSURE_BOX: f32 = 12.0;

/// Height of the toolbar strip.
///
/// The one home of its fit triple, matching [`header_height`] and the
/// `table_row_h`/`table_head_h` pair below. Centralizing the formula prevents callers that size or
/// position adjacent chrome from drifting away from the row that is actually rendered.
pub fn toolbar_height(cx: &App) -> f32 {
    fit_h_value(cx, TOOLBAR_H, 13.0, 9.5)
}

/// Height of the window header strip — the companion to [`toolbar_height`].
pub fn header_height(cx: &App) -> f32 {
    fit_h_value(cx, HEADER_TOP_H, 14.0, 9.0)
}

/// [`header_height`] as `Pixels`, for the row that draws itself.
pub fn header_height_px(cx: &App) -> Pixels {
    px(header_height(cx))
}

/// One group of controls inside a chrome strip — the header or the toolbar.
///
/// The partner of [`CHROME_GAP`] and [`chrome_divider`]: a group carries the gap INSIDE it, and the
/// boundary between two groups is drawn by a rule standing between them. One builder so the two
/// strips cannot drift into different spacing, which would read as a seam across what is one
/// visual block on one background.
pub fn chrome_section(cx: &App) -> Div {
    moon_ui::h_flex()
        .flex_none()
        .items_center()
        .gap(ui_px(cx, CHROME_GAP))
}

/// Rendered width that makes a glyph button SQUARE — the column selector (`▦`) and the report
/// export (`⇩`).
///
/// It returns the button's own drawn height, so the caller must pass it to a RENDERED width
/// (`MoonDropdown::trigger_width`, `MoonButton::width`), never to a `*_scaled` variant: MoonUI
/// scales a scaled trigger width by `font()` (which adds the Font-slider delta) while it scales the
/// height by `ui()` (a pure multiply), so the two diverge as soon as the slider leaves zero — a
/// scaled 26 renders ≈33×26 at the shipped default delta.
///
/// MIRRORS MoonUI, like [`micro_control_h_value`]: `MoonButtonMetrics::base_for_size(Size::Small)`,
/// which `MoonButtonSize::Action` resolves to, is `height 26`, `line_height 14`, so its `pad_y` is
/// `6` — exactly the arguments below. `MoonButtonMetrics` is private there, so nothing checks this
/// automatically; if MoonUI's Small metrics move, this must follow by hand.
pub fn glyph_btn_w(cx: &App) -> f32 {
    fit_h_value(cx, 26.0, 14.0, 6.0)
}

/// Ceiling for a header selector label (core, manual strategy).
///
/// Those pills size to their content and both names are arbitrary user text, so without a ceiling
/// one long name pushes the right-hand cluster — clock and window controls included — off the
/// window. Matches the other selectors' ceiling; the full name stays in the open menu.
/// This is the unscaled width passed through [`font_w`].
pub const HEADER_LABEL_MAX_W: f32 = 260.0;

/// Window widths at which the header ticker drops its per-window deltas, then goes entirely.
///
/// It collapses by priority rather than clipping: the readout is monospaced and its informative
/// part is the tail, so a character-level clip eats the deltas and then the price digits, turning
/// "61 333$" into "61 33" — a plausible WRONG price stated as fact. Usable only because
/// [`HEADER_LABEL_MAX_W`] bounds the clusters that would otherwise grow without limit.
const TICKER_DELTAS_MIN_W: f32 = 1200.0;
const TICKER_MIN_W: f32 = 1000.0;

/// Return whether the header ticker fits at `chrome_width`.
///
/// ONE predicate for the header that renders it and the popup layer that must not outlive its
/// trigger. Scaled by the UI font: everything beside the ticker is text, so a larger font claims
/// proportionally more of the same window and the ticker has to yield sooner.
pub fn ticker_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, TICKER_MIN_W)
}

/// Return whether the ticker's 1h/24h deltas fit at `chrome_width`.
///
/// Uses the same font-scaled width policy as [`ticker_visible`], with a higher threshold so the
/// price remains visible after the deltas collapse.
pub fn ticker_deltas_visible(cx: &App, chrome_width: f32) -> bool {
    chrome_width >= font_w(cx, TICKER_DELTAS_MIN_W)
}

/// Transparent macOS titlebars keep native traffic-light buttons over the client
/// area. Keep terminal chrome content and drag hitboxes out of that strip.
pub fn titlebar_leading_inset() -> f32 {
    if cfg!(target_os = "macos") {
        76.0
    } else {
        HEADER_PAD_X
    }
}

pub fn show_custom_window_controls() -> bool {
    !cfg!(target_os = "macos")
}

pub fn platform_window_decorations() -> Option<WindowDecorations> {
    if cfg!(target_os = "linux") {
        Some(WindowDecorations::Client)
    } else {
        None
    }
}

const LOGO_GLOW_SVG_RAW: &str = include_str!("../../../assets/brand/moonbot-logo.svg");
const LOGO_SRC_W: f32 = 199.0;
const LOGO_SRC_H: f32 = 43.0;
const LOGO_GLOW_VIEW_W: f32 = LOGO_SRC_W * 1.2;
const LOGO_GLOW_VIEW_H: f32 = LOGO_SRC_W * 1.2;

pub fn solid(hex: u32) -> Rgba {
    rgb(hex)
}

/// Convert a `0xRRGGBB` palette token to opaque `Hsla`.
///
/// This centralizes a conversion formerly duplicated in `screener/table.rs`, `panels/alerts.rs`,
/// and `strategies/mod.rs`.
///
/// Args:
///     hex: Palette color encoded as `0xRRGGBB`.
///
/// Returns:
///     The color with alpha set to one.
pub fn moon(hex: u32) -> Hsla {
    rgba_from(hex, 1.0)
}

/// Convert a `0xRRGGBB` palette token to `Hsla` with an explicit alpha value.
///
/// Args:
///     hex: Palette color encoded as `0xRRGGBB`.
///     alpha: Alpha passed through to `rgba_from`.
///
/// Returns:
///     The color with the requested alpha.
pub fn moon_alpha(hex: u32, alpha: f32) -> Hsla {
    rgba_from(hex, alpha)
}

/// Return the theme-correct positive colour.
///
/// The light theme uses its darker text token for legibility; the dark theme uses its base green.
pub fn positive_color(p: MoonPalette) -> u32 {
    if p.is_light() { p.green_text } else { p.green }
}

/// Return the theme-correct danger colour.
///
/// The light theme uses its darker text token for legibility; the dark theme uses its base red.
pub fn danger_color(p: MoonPalette) -> u32 {
    if p.is_light() { p.red_text } else { p.red }
}

/// Convert GPUI's `0xRRGGBB` representation to palette/config `[u8; 3]` RGB bytes.
pub fn u32_to_rgb(c: u32) -> [u8; 3] {
    [
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    ]
}

/// Convert palette/config `[u8; 3]` RGB bytes to GPUI's `0xRRGGBB` representation.
pub fn rgb_to_u32(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// Build the explicit palette supplied to `MoonColorPicker::colors`.
///
/// It contains five saturation/lightness variants for each of 12 hues plus five grays, for 65
/// swatches total. Each picker row contains the five variants of one hue. The forked component has
/// no free-form HSV picker and otherwise exposes only ten theme colors.
///
/// Returns:
///     The complete ordered swatch palette.
pub fn picker_palette() -> Vec<Hsla> {
    let mut out = Vec::with_capacity(65);
    // Pairs are ordered by decreasing lightness from light to dark; saturation varies independently.
    const SHADES: [(f32, f32); 5] = [
        (0.85, 0.72),
        (0.85, 0.58),
        (0.90, 0.46),
        (0.85, 0.34),
        (0.70, 0.24),
    ];
    for hue_step in 0..12 {
        let h = hue_step as f32 / 12.0;
        for (s, l) in SHADES {
            out.push(hsla(h, s, l, 1.0));
        }
    }
    for l in [0.95, 0.75, 0.50, 0.30, 0.10] {
        out.push(hsla(0.0, 0.0, l, 1.0));
    }
    out
}

/// Convert a `MoonColorPicker` `Hsla` value to 8-bit sRGB for byte-backed color fields.
///
/// The alpha channel is intentionally omitted. Consumers include `fig_styles` and strategy hex
/// fields, which retain or encode alpha separately.
///
/// Args:
///     h: Picker color to convert.
///
/// Returns:
///     Rounded red, green, and blue bytes.
pub fn hsla_to_rgb8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

pub fn mono() -> SharedString {
    SharedString::from("Geist Mono")
}

pub fn ui_font() -> SharedString {
    SharedString::from("Inter")
}

pub fn ui_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).ui(value)
}

pub fn font_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).font(value)
}

pub fn line_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).line_height(value)
}

pub fn fit_h_value(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> f32 {
    MoonTheme::active_tokens(cx).fit_height(base_height, base_line_height, base_pad_y)
}

pub fn ui_px(cx: &App, value: f32) -> Pixels {
    px(ui_value(cx, value))
}

pub fn text_px(cx: &App, value: f32) -> Pixels {
    px(font_value(cx, value))
}

pub fn line_px(cx: &App, value: f32) -> Pixels {
    px(line_value(cx, value))
}

/// Return the MoonUI theme's base terminal text size, `mono_font_size`, whose default is 11.
///
/// The three raw-GPUI text tiers below derive from this value, so a `.toml` base-size change moves
/// them together.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The unscaled base font size.
fn base_text(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).typography.mono_font_size
}

/// Return the caption text size for raw GPUI elements such as `div().text_size(...)`.
///
/// The raw-GPUI tiers derive from MoonUI's base and pass through `font()`, so they respond to the
/// Settings Font slider. MoonUI components such as `MoonText`, `MoonButtonSegment`, and
/// `MoonDataCell` already scale their own default or supplied base size. Do not pass a `t_*` result
/// or `font_value(...)` into them, because that applies font scaling twice.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     Approximately 9px at the default theme base with zero Font-slider delta, for badges, small
///     labels, and counters. The application's default +3 delta makes it approximately 12px.
pub fn t_caption(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) - 2.0)
}

/// Return the theme-base body size for raw GPUI text, tables, and monospaced values.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     Approximately 11px at the default theme base with zero Font-slider delta. The application's
///     default +3 delta makes it approximately 14px.
pub fn t_body(cx: &App) -> Pixels {
    text_px(cx, base_text(cx))
}

/// Return the one-step-up body size for a row that must read above its neighbours in place.
///
/// Sits between [`t_body`] and [`t_title`] for the case where a row is emphasized INSIDE a list of
/// fixed-height rows: `t_title` is three steps up, and at the top of the Font slider its line box
/// outgrows a row height that does not track the font, so the text clips. One step clears the
/// neighbours while still fitting.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     Approximately 12px at the default theme base with zero Font-slider delta. The application's
///     default +3 delta makes it approximately 15px.
pub fn t_body_lg(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) + 1.0)
}

/// Return the title size for raw GPUI headings and large accents.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     Approximately 14px at the default theme base with zero Font-slider delta. The application's
///     default +3 delta makes it approximately 17px.
pub fn t_title(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) + 3.0)
}

/// Return an UNSCALED base for a MoonUI component's own size field — `MoonText::font_size`,
/// `MoonBadgeSize::Custom`'s `font_size` — never for a raw-GPUI `.text_size(...)`.
///
/// MoonUI components apply `tokens.font()` to whatever base they are given, so passing a `t_*`
/// result or [`font_value`] here would scale the Font-slider delta twice: invisible at delta 0,
/// producing roughly 30px text at the shipped range's top end. `step` is a caller-supplied LOCAL
/// unscaled addition, for a surface the user has been given its own size control over on top of
/// the global Font slider; zero means exactly the theme base, matching every other caller of
/// `base_text`. Use [`text_px`] instead when the destination is `div().text_size(...)`.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///     step: Local unscaled addition on top of the theme base; `0.0` for no local adjustment.
///
/// Returns:
///     The unscaled base size, in the same units MoonUI's own component defaults use.
pub fn moon_text_base(cx: &App, step: f32) -> f32 {
    base_text(cx) + step
}

pub fn fit_h_px(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> Pixels {
    px(fit_h_value(cx, base_height, base_line_height, base_pad_y))
}

/// Return the drawn height of a `MoonButtonSize::Micro` control, in base px.
///
/// MIRRORS MoonUI: `MoonButtonMetrics::base_for_size(Size::XSmall)` is `height 18`,
/// `line_height 12`, whose `pad_y` works out to `3` — exactly the arguments below. Only those
/// three numbers are mirrored; the scaling goes through MoonUI's own `MoonTheme::fit_height`.
///
/// Two callers need it: a plain `div` sitting BESIDE such a button (a card title) takes the same
/// box so the row's `items_center` centres two equal heights instead of centring a text line box
/// against a taller pill, and the chart's action overlay sizes its own layout from it.
///
/// Nothing checks this against MoonUI: `MoonButtonMetrics` is private there and the sibling
/// checkout is not guaranteed present in CI, so a test can neither call it nor grep it. If
/// MoonUI's XSmall metrics move, this must follow by hand.
pub fn micro_control_h_value(cx: &App) -> f32 {
    fit_h_value(cx, 18.0, 12.0, 3.0)
}

/// [`micro_control_h_value`] as `Pixels` — the `*_value`/`*_px` pair every geometry helper in
/// this file ships, because layout arithmetic needs the `f32` and styling needs the `Pixels`.
pub fn micro_control_h(cx: &App) -> Pixels {
    px(micro_control_h_value(cx))
}

/// Width of mono text drawn at the terminal's body size — [`ui_text_width`] with the theme's body
/// base filled in.
///
/// Exists so a caller measuring text it drew with `t_body()` + [`mono`] cannot reach for
/// `t_body(cx)` as the size argument: that value is ALREADY scaled, and `ui_text_width` scales its
/// input again, so the estimate would come out font-scaled twice. Keeps the base itself private.
pub fn mono_body_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width(cx, text, base_text(cx), weight, true)
}

/// Width of mono text drawn at the terminal's caption size — the [`t_caption`] partner of
/// [`mono_body_text_width`], and it exists for the same reason: `t_caption(cx)` is already scaled,
/// so passing it to [`ui_text_width`] would scale it twice.
pub fn mono_caption_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width(cx, text, base_text(cx) - 2.0, weight, true)
}

/// Width of mono text drawn at the terminal's title size — the [`t_title`] partner of
/// [`mono_body_text_width`], and it exists for the same reason: `t_title(cx)` is already scaled,
/// so passing it to [`ui_text_width`] would scale it twice.
pub fn mono_title_text_width(cx: &App, text: &str, weight: f32) -> f32 {
    ui_text_width(cx, text, base_text(cx) + 3.0, weight, true)
}

/// Cache key for one glyph advance under an exact resolved font and requested weight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MonoGlyphKey {
    font_id: FontId,
    weight_bits: u32,
    character: char,
}

/// Per-batch glyph-advance cache shared across many measured strings.
#[derive(Default)]
struct MonoGlyphWidthCache {
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
    fn text_width(
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
        let family = SharedString::from(tokens.font_family(true));
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

/// Unscaled base size an Action-size button renders its label at.
///
/// MIRRORS MoonUI, like [`glyph_btn_w`]: the text size inside `MoonButtonMetrics` for the Small
/// metrics that `MoonButtonSize::Action` resolves to. Those metrics are private there, so nothing
/// checks this automatically; if they move, this must follow by hand.
pub const ACTION_LABEL_BASE: f32 = 10.5;

/// Resolve the exact font and rendered size one measurement will use.
///
/// The single place [`ui_text_width`] and [`text_metrics_key`] agree on how a request becomes a
/// font: a key derived from a hand-copied version of these three lines would keep validating stale
/// widths the day the resolution changes.
fn measure_font(cx: &App, base_font_size: f32, weight: f32, mono: bool) -> (FontId, Pixels) {
    let tokens = MoonTheme::active_tokens(cx);
    let font = Font {
        weight: FontWeight(weight),
        ..font(tokens.font_family(mono))
    };
    (
        cx.text_system().resolve_font(&font),
        px(tokens.font(base_font_size)),
    )
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
    // Counted, because the callers run this every frame: see `diag::UI_TEXT_WIDTH_CALLS`.
    let measured = crate::diag::timer();
    crate::diag::bump(&crate::diag::UI_TEXT_WIDTH_CALLS);
    crate::diag::bump_by(
        &crate::diag::UI_TEXT_WIDTH_CHARS,
        text.chars().count() as u64,
    );
    let (font_id, size) = measure_font(cx, base_font_size, weight, mono);
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

/// Truncate `text` with an ellipsis to the available prefix budget, returning the result and width.
///
/// Text is arbitrary Unicode and equal glyph width is not guaranteed outside Geist Mono, so the
/// prefix is accumulated character by character against the real budget rather than by counting
/// characters. Returns `text` unchanged when it already fits. A budget narrower than the ellipsis
/// returns the ellipsis alone even though no non-empty marker can fit that budget.
///
/// The width comes back because every caller needs it and measuring costs an uncached glyph layout
/// PER CHARACTER (see [`ui_text_width`]) — a caller that re-measured the result would pay for the
/// same string twice, and these run per frame.
///
/// `measure` is passed in rather than taken from the theme: the caller draws its text at its own
/// size and weight, and truncating against a narrower font underestimates the width and overflows
/// anyway. Pure, so it is unit-testable without an `App`.
pub fn fit_text(text: &str, max_w: f32, measure: impl Fn(&str) -> f32) -> (String, f32) {
    const ELLIPSIS: &str = "\u{2026}";
    let full = measure(text);
    if full <= max_w {
        return (text.to_string(), full);
    }
    let budget = max_w - measure(ELLIPSIS);
    let mut head = String::new();
    let mut used = 0.0f32;
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let w = measure(ch.encode_utf8(&mut buf));
        if used + w > budget {
            break;
        }
        used += w;
        head.push(ch);
    }
    let out = format!("{}{ELLIPSIS}", head.trim_end());
    let width = measure(&out);
    (out, width)
}

/// Break text into at most `max_lines` lines of `max_w`, cutting the last one if it still spills.
///
/// For the one thing the chart prints that is a SENTENCE rather than a figure: the core's own
/// detect line, which states half a dozen numbers and does not fit the plot's width. Cutting it —
/// what every caption does — throws away most of what it said; wrapping it keeps the rest and puts
/// the ellipsis at the end of the block instead of the end of the first line.
///
/// Lines break on SPACES. A word longer than the whole line has nowhere to break, so it is cut by
/// [`fit_text`] and ends the block: continuing would either loop or split a number in half.
///
/// The split point is arithmetic rather than a search: `measure` is called once for the whole
/// remainder, the advance per character comes from that, and the candidate is walked back to its
/// last space. Measuring candidate after candidate would shape the same sentence a dozen times per
/// frame. The advance is an AVERAGE, so on a face that is not monospaced — or on text whose glyphs
/// are not one code point each — the candidate can come out over budget; it is therefore measured
/// and walked back space by space until it fits, which costs nothing on the monospaced face the
/// chart actually draws and stays correct on any other.
///
/// `measure` is passed in for the same reason [`fit_text`] takes it, and this is pure for the same
/// reason: it is unit-testable without an `App`.
pub fn wrap_text(
    text: &str,
    max_w: f32,
    max_lines: usize,
    measure: impl Fn(&str) -> f32,
) -> Vec<(String, f32)> {
    let mut out: Vec<(String, f32)> = Vec::new();
    let mut rest = text.trim();
    if max_lines == 0 {
        return out;
    }
    while !rest.is_empty() && out.len() < max_lines {
        let full = measure(rest);
        if full <= max_w {
            out.push((rest.to_string(), full));
            return out;
        }
        // The last line takes what fits and says, with an ellipsis, that there was more.
        if out.len() + 1 == max_lines {
            let (line, w) = fit_text(rest, max_w, &measure);
            if !line.is_empty() {
                out.push((line, w));
            }
            return out;
        }
        let count = rest.chars().count().max(1) as f32;
        let advance = full / count;
        let fits = match advance > 0.0 {
            true => (max_w / advance).floor().max(0.0) as usize,
            false => 0,
        };
        // Byte index just past the last character that fits, then back off to the last space
        // before it.
        let end = rest
            .char_indices()
            .nth(fits)
            .map_or(rest.len(), |(at, _)| at);
        // `rest` is trimmed on every path, so a space at index 0 cannot happen; a line with no
        // space at all is the unbreakable-word case below.
        let Some(cut) = rest[..end].rfind(' ') else {
            let (line, w) = fit_text(rest, max_w, &measure);
            if !line.is_empty() {
                out.push((line, w));
            }
            return out;
        };
        // The candidate, walked back a word at a time until it MEASURES within the budget.
        let mut cut = cut;
        let (line, width) = loop {
            let line = rest[..cut].trim_end();
            let w = measure(line);
            if w <= max_w {
                break (line, w);
            }
            match line.rfind(' ') {
                Some(at) if at > 0 => cut = at,
                // One word, and it does not fit: cut it and end the block rather than split a
                // figure across two lines, where each half reads as a number of its own.
                _ => {
                    let (line, w) = fit_text(rest, max_w, &measure);
                    if !line.is_empty() {
                        out.push((line, w));
                    }
                    return out;
                }
            }
        };
        out.push((line.to_string(), width));
        rest = rest[cut..].trim_start();
    }
    out
}

/// [`fit_text`] at the size a selector pill draws its label.
///
/// The literal is deliberately NOT [`ACTION_LABEL_BASE`]: a pill is not an Action-size button, and
/// tying its truncation budget to that constant would move this text the day MoonUI moves the
/// button metric.
pub fn fit_label(cx: &App, text: &str, max_w: f32) -> String {
    fit_text(text, max_w, |s| ui_text_width(cx, s, 10.5, 400.0, true)).0
}

/// Return the effective font-scaled `MoonDataTable` row height.
///
/// This mirrors the component's `fit_height(row_h, 14.0, 5.5)` call so wrappers computing natural
/// table height do not clip rows at large font settings.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The rendered row height in pixels.
pub fn table_row_h(cx: &App) -> f32 {
    fit_h_value(cx, TABLE_ROW_H, 14.0, 5.5)
}

/// Return the effective font-scaled `MoonDataTable` header height.
///
/// This mirrors the component's `fit_height(header_h, 11.0, 7.5)` call.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The rendered header height in pixels.
pub fn table_head_h(cx: &App) -> f32 {
    fit_h_value(cx, TABLE_HEAD_H, 11.0, 7.5)
}

/// Return the current font-size scale relative to the theme base.
///
/// Delegates to MoonUI so the Settings Font slider and fixed-width text containers share one width
/// scale definition.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The active scaled base font size divided by the unscaled base.
pub fn font_scale(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).font_width_scale()
}

/// Scale a fixed text-container width with the font and return `Pixels`.
///
/// Args:
///     cx: Application context used to calculate [`font_scale`].
///     base: Width at zero Font-slider delta.
///
/// Returns:
///     The font-scaled width as `Pixels`.
pub fn font_w_px(cx: &App, base: f32) -> Pixels {
    px(font_w(cx, base))
}

/// Scale a fixed text-container width with the font and return raw pixels as `f32`.
///
/// Use this for MoonUI builders that apply raw `px(...)` without their own scaling, including
/// `menu_width`, `trigger_width`, and `MoonButton::width`.
///
/// Args:
///     cx: Application context used to calculate [`font_scale`].
///     base: Width at zero Font-slider delta.
///
/// Returns:
///     The font-scaled raw pixel width.
pub fn font_w(cx: &App, base: f32) -> f32 {
    MoonTheme::active_tokens(cx).font_width(base)
}

// Radius tokens come from `MoonMetrics::TERMINAL`, rather than local numeric values. Avoid raw
// `px(N)` in `.rounded()`. Pill values such as `SEL_H / 2.0` or `999.0` describe shape, not a radius
// tier, and are outside this rule. `*_BASE` values are unscaled inputs for MoonUI builders that scale
// internally, such as `MoonButtonSize::Custom { radius }`; `r_*` functions return ready-to-use
// `Pixels` for raw GPUI `.rounded()`. Mixing them applies scaling twice.
//
// `MoonMetrics` exposes these two shared radius tokens and no shared small-radius token for chips or
// swatches; a third `radius_sm` metric has been requested upstream.
/// Unscaled shared `button_radius` token for MoonUI builders that apply UI scaling internally.
pub const R_BUTTON_BASE: f32 = M.button_radius;
/// Unscaled shared `container_radius` token for MoonUI builders that apply UI scaling internally.
pub const R_CONTAINER_BASE: f32 = M.container_radius;

/// Return the scaled MoonUI `button_radius`, default 4, for buttons, cards, popups, and panels.
///
/// Args:
///     cx: Application context used to apply UI scaling.
///
/// Returns:
///     The ready-to-use raw-GPUI radius.
pub fn r_button(cx: &App) -> Pixels {
    ui_px(cx, R_BUTTON_BASE)
}

/// Return the scaled MoonUI `container_radius`, default 8, for dialogs, modals, and containers.
///
/// Args:
///     cx: Application context used to apply UI scaling.
///
/// Returns:
///     The ready-to-use raw-GPUI radius.
pub fn r_container(cx: &App) -> Pixels {
    ui_px(cx, R_CONTAINER_BASE)
}

/// Brand-book navy for the Moonbot wordmark in the light theme.
///
/// Navy lacks contrast in the dark theme, where the wordmark uses `p.text` instead.
const LOGO_WORDMARK_NAVY: u32 = 0x0C2C4A;

pub fn logo_glow_sized(cx: &App, width: f32) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let text = if p.is_light() {
        LOGO_WORDMARK_NAVY
    } else {
        p.text
    };
    let text_fill = format!("#{text:06X}");
    let logo =
        LOGO_GLOW_SVG_RAW.replace(r##"fill="#E7E7E7""##, &format!(r##"fill="{text_fill}""##));
    let paths = logo
        .split_once(r#"<g clip-path="url(#clip0_3800_3393)">"#)
        .and_then(|(_, rest)| rest.split_once("</g>"))
        .map(|(paths, _)| paths)
        .unwrap_or("");
    let cx = LOGO_GLOW_VIEW_W * 0.5;
    let cy = LOGO_GLOW_VIEW_H * 0.5;
    let r = LOGO_GLOW_VIEW_W * 0.5;
    let logo_x = (LOGO_GLOW_VIEW_W - LOGO_SRC_W) * 0.5;
    let logo_y = (LOGO_GLOW_VIEW_H - LOGO_SRC_H) * 0.5;
    let (aura_0_color, aura_1_color, aura_2_color, aura_0, aura_1, aura_2) = if p.is_light() {
        (
            "#BFF5C9",
            "#AEEFC1",
            "#98E8B2",
            0.30 * 0.5 / 3.0,
            0.19 * 0.5 / 3.0,
            0.07 * 0.5 / 3.0,
        )
    } else {
        ("#00BCFF", "#1A76FF", "#0A5CFF", 0.30, 0.19, 0.07)
    };
    let svg = format!(
        r##"<svg width="{view_w}" height="{view_h}" viewBox="0 0 {view_w} {view_h}" fill="none" xmlns="http://www.w3.org/2000/svg">
<defs>
  <radialGradient id="moonbot_aura" cx="50%" cy="50%" r="50%">
    <stop offset="0%" stop-color="{aura_0_color}" stop-opacity="{aura_0:.3}"/>
    <stop offset="34%" stop-color="{aura_1_color}" stop-opacity="{aura_1:.3}"/>
    <stop offset="68%" stop-color="{aura_2_color}" stop-opacity="{aura_2:.3}"/>
    <stop offset="100%" stop-color="{aura_2_color}" stop-opacity="0"/>
  </radialGradient>
</defs>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#moonbot_aura)"/>
<g transform="translate({logo_x} {logo_y})">{paths}</g>
</svg>"##,
        view_w = LOGO_GLOW_VIEW_W,
        view_h = LOGO_GLOW_VIEW_H,
        cx = cx,
        cy = cy,
        r = r,
        logo_x = logo_x,
        logo_y = logo_y,
        aura_0_color = aura_0_color,
        aura_1_color = aura_1_color,
        aura_2_color = aura_2_color,
        aura_0 = aura_0,
        aura_1 = aura_1,
        aura_2 = aura_2,
        paths = paths,
    );
    let frame_w = width * (LOGO_GLOW_VIEW_W / 199.0);
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(frame_w))
    .h(px(frame_w * (LOGO_GLOW_VIEW_H / LOGO_GLOW_VIEW_W)))
}

/// Vertical 1px group separator.
///
/// The height goes through `ui()`, which tracks the UI SCALE but NOT the Font slider — matching
/// MoonUI, which draws its own separators the same way (the brand cluster in `MoonWindowFrame` is
/// one). So the rule keeps its height while a larger font grows the row around it; standing beside
/// a MoonUI separator that did move is the worse of the two mismatches. The 1px width stays raw,
/// also matching MoonUI, since a hairline must not thicken with the font.
pub fn vline(cx: &App, height: f32, color: u32) -> impl IntoElement {
    // flex_none: a 1px rule inside a shrinking row would otherwise be the first thing squeezed
    // to nothing, silently dropping the group boundary it draws.
    div()
        .flex_none()
        .w(px(1.0))
        .h(ui_px(cx, height))
        .bg(rgb(color))
}

/// Group separator for the horizontal chrome strips — the toolbar and the window header.
///
/// One definition of the chrome-height rule, so those two strips cannot drift apart, and so the
/// height matches the separator MoonUI's own brand cluster draws.
///
/// Drawn in `border_hover` rather than `border`: against `shell_high`, `border` measures about
/// 1.2:1 in the dark palette and 1.3:1 in the light palette. These rules are the only thing marking
/// where one group ends and the next begins because spacing inside and between groups is identical.
/// `border_hover` is one step stronger in both palettes (about 1.5:1 dark and 1.7:1 light), which
/// reads as a boundary without reading as a frame.
pub fn chrome_divider(cx: &App, p: MoonPalette) -> impl IntoElement {
    vline(cx, 16.0, p.border_hover)
}

pub fn status_dot(color: u32, cx: &App) -> impl IntoElement {
    status_dot_sized(color, 5.0, cx)
}

/// Draw a status dot at a caller-selected logical size.
///
/// Args:
///     color: Theme-resolved RGB color.
///     size: Unscaled logical diameter.
///     cx: Application context used to apply the UI scale.
///
/// Returns:
///     A circular status marker whose diameter tracks the active UI scale.
pub fn status_dot_sized(color: u32, size: f32, cx: &App) -> impl IntoElement {
    dot(size, solid(color), cx)
}

/// Opacity a status colour carries when its source has not confirmed it on the current connection.
///
/// One value for every surface that draws such a state, so the same "second-hand" language cannot
/// mean two different things in two windows.
pub const STALE_ALPHA: f32 = 0.45;

/// Draw a status dot whose colour is FADED, for a value the source no longer confirms.
///
/// The same dot at the same size in the same colour, at reduced opacity: a reader must still see
/// WHICH state is being reported — a stale "running" is not the same fact as "unknown" — while the
/// fade says the claim is second-hand. Anything that changed the hue instead would collide with the
/// palette's own green/amber/red meanings.
///
/// Args:
///     color: Theme-resolved RGB colour of the confirmed state.
///     cx: Application context used to apply the UI scale.
///
/// Returns:
///     A circular status marker at [`STALE_ALPHA`].
pub fn status_dot_stale(color: u32, cx: &App) -> impl IntoElement {
    dot(5.0, moon_alpha(color, STALE_ALPHA), cx)
}

/// The one geometry both status dots draw, so a faded dot cannot drift from the solid one it
/// stands in for.
fn dot(size: f32, fill: impl Into<gpui::Fill>, cx: &App) -> impl IntoElement {
    div()
        .w(ui_px(cx, size))
        .h(ui_px(cx, size))
        .rounded(ui_px(cx, 999.0))
        .bg(fill)
}

/// Drawn width of a [`status_dot`], for a layout that must reserve its column.
///
/// One source with the dot itself: a row that leaves a gap for it and the dot that fills the gap
/// cannot drift apart when the UI scale changes.
pub fn status_dot_w(cx: &App) -> f32 {
    ui_value(cx, 5.0)
}

#[cfg(test)]
mod tests;
