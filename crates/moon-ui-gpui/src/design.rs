//! Moonbot terminal design tokens extracted from the Moonbot Terminal design reference.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_ui::{MoonMetrics, MoonPalette, MoonTheme, rgba_from};
use std::sync::Arc;

const M: MoonMetrics = MoonMetrics::TERMINAL;

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
/// The alpha channel is intentionally omitted. Consumers include `fig_style` and strategy hex
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
///     labels, and counters. The application's default +2 delta makes it approximately 11px.
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
///     default +2 delta makes it approximately 13px.
pub fn t_body(cx: &App) -> Pixels {
    text_px(cx, base_text(cx))
}

/// Return the title size for raw GPUI headings and large accents.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     Approximately 14px at the default theme base with zero Font-slider delta. The application's
///     default +2 delta makes it approximately 16px.
pub fn t_title(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) + 3.0)
}

pub fn fit_h_px(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> Pixels {
    px(fit_h_value(cx, base_height, base_line_height, base_pad_y))
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
    let tokens = MoonTheme::active_tokens(cx);
    let size = px(tokens.font(base_font_size));
    let font = Font {
        weight: FontWeight(weight),
        ..font(tokens.font_family(mono))
    };
    let ts = cx.text_system();
    let font_id = ts.resolve_font(&font);
    text.chars()
        .map(|ch| f32::from(ts.layout_width(font_id, size, ch)))
        .sum()
}

/// Size a compact `MoonPopupMenu` for its longest label.
///
/// The calculation mirrors MoonUI's monospaced compact-row metrics: 9.5px text at up to weight
/// 600, scaled horizontal padding and gap, plus the unscaled check column and border. `min_w` is
/// the lower bound; the scaled `MENU_MAX_W` is the upper bound unless it falls below `min_w`.
pub fn menu_fit_width<'a>(cx: &App, labels: impl IntoIterator<Item = &'a str>, min_w: f32) -> f32 {
    let max_label_w = labels
        .into_iter()
        .map(|l| ui_text_width(cx, l, 9.5, 600.0, true))
        .fold(0.0, f32::max);
    (ui_value(cx, 6.0 * 2.0 + 5.0 + 4.0 * 2.0) + 12.0 + 2.0 + max_label_w)
        .ceil()
        // Keep the upper bound at least as large as the lower bound. Some callers pass unscaled
        // `min_w` values, such as the header's raw 180 or 200, while a heavily reduced font can make
        // `font_w(MENU_MAX_W)` smaller. `.max(min_w)` prevents `f32::clamp` from panicking with
        // `lo > hi`; in that degenerate case the floor wins over the ceiling.
        .clamp(min_w, font_w(cx, MENU_MAX_W).max(min_w))
}

const CORE_MENU_LABEL_W: f32 = 256.0;
const COMPACT_CHECKED_MENU_SCALED_CHROME_W: f32 = 6.0 * 2.0 + 5.0 * 2.0 + 4.0 * 2.0;
const COMPACT_CHECKED_MENU_FIXED_CHROME_W: f32 = 12.0 + 2.0;

/// Return the fixed width shared by core-selection menus.
///
/// The text column has the same scale as the compact menu's 9.5px monospaced font. Its surrounding
/// padding and gaps follow the independent UI scale, so neither setting can consume the label
/// budget. At default scales the text column plus chrome reconstructs the 300px design width.
/// A caller-specific minimum may enlarge the menu but never makes the shared baseline narrower.
///
/// Args:
///     cx: Application context used to apply the active font and UI scales.
///     min_w: Unscaled caller-specific minimum width.
///
/// Returns:
///     The fixed core-menu width in rendered pixels.
pub fn core_menu_width(cx: &App, min_w: f32) -> f32 {
    let compact_text_scale = font_value(cx, 9.5) / 9.5;
    core_menu_width_for_scales(compact_text_scale, ui_value(cx, 1.0), font_w(cx, min_w))
}

/// Calculate core-menu width from already resolved text, UI, and minimum scales.
///
/// Args:
///     compact_text_scale: Rendered compact-menu font size divided by its 9.5px base.
///     ui_scale: Resolved UI scale after MoonUI's lower-bound handling.
///     min_rendered_w: Caller-specific minimum after its own scale has been applied.
///
/// Returns:
///     The rendered menu width whose text and chrome budgets scale independently.
pub(crate) fn core_menu_width_for_scales(
    compact_text_scale: f32,
    ui_scale: f32,
    min_rendered_w: f32,
) -> f32 {
    (CORE_MENU_LABEL_W * compact_text_scale + compact_checked_menu_chrome_width_for_scale(ui_scale))
        .max(min_rendered_w)
}

/// Return the non-text width of a compact checked `MoonPopupMenu` row.
///
/// Args:
///     cx: Application context used to apply the active UI scale.
///
/// Returns:
///     Menu and row padding, two flex gaps, the check slot, and the two menu borders.
fn compact_checked_menu_chrome_width(cx: &App) -> f32 {
    compact_checked_menu_chrome_width_for_scale(ui_value(cx, 1.0))
}

/// Calculate compact checked-menu chrome width from an already resolved UI scale.
///
/// Args:
///     ui_scale: Resolved UI scale after MoonUI's lower-bound handling.
///
/// Returns:
///     Scaled menu and row spacing plus the fixed check slot and menu borders.
fn compact_checked_menu_chrome_width_for_scale(ui_scale: f32) -> f32 {
    COMPACT_CHECKED_MENU_SCALED_CHROME_W * ui_scale + COMPACT_CHECKED_MENU_FIXED_CHROME_W
}

/// Return the text budget inside a compact checked `MoonPopupMenu` row.
///
/// This mirrors the same compact-row chrome used by [`menu_fit_width`]: menu and row padding plus
/// two item gaps follow UI scale, while the 12px check slot and two 1px borders remain unscaled.
///
/// Args:
///     cx: Application context used to apply the active UI scale.
///     menu_w: Rendered outer menu width.
///
/// Returns:
///     Non-negative rendered width available to the row label.
pub fn menu_item_label_width(cx: &App, menu_w: f32) -> f32 {
    (menu_w - compact_checked_menu_chrome_width(cx)).max(0.0)
}

/// Outer `MoonPopover::width` that hosts content of intrinsic width `content_w`.
///
/// MoonUI renders `.w(px(width)).p(px(tokens.ui(6.0))).border(px(1.0))`. GPUI treats that
/// width as the border box, leaving `width - 2*ui(6) - 2` for content. The padding tracks
/// the UI scale, while MoonUI's hard-coded 1px border does not.
///
/// `content_w` must already be in the scale its content actually uses — wrap it in `font_w`
/// for our own font-scaled blocks, pass it raw for a moonui widget with a fixed px width.
///
/// Interim (FORK_BUGS): delete when `MoonPopover` grows a fit-to-content mode upstream.
pub fn popover_outer_width(cx: &App, content_w: f32) -> f32 {
    content_w + 2.0 * ui_value(cx, 6.0) + 2.0
}

/// Outer width of a default-size `MoonCalendar` — the value to hand [`popover_outer_width`].
///
/// For default `Size::Medium`, MoonUI's month/year grid is 264px wide, while the day grid is
/// `7*size_9 + 6*gap_0p5` = 16.5rem. The calendar root adds `p_3` (0.75rem per side) and a
/// 1px border per side. `MoonRoot` sets the window rem from `cx.theme().font_size`, which
/// `MoonTheme` synchronizes to `base_font_size()`, so the rem-based day grid can exceed 264px.
///
/// Interim (FORK_BUGS), same removal trigger as [`popover_outer_width`].
pub fn calendar_outer_width(cx: &App) -> f32 {
    // Month/year grid (calendar.rs, Size::Medium) vs the day grid's 7*2.25rem + 6*0.125rem;
    // the root adds p_3 (0.75rem) per side plus the 1px border.
    const MONTH_GRID_W: f32 = 264.0;
    const DAY_GRID_REMS: f32 = 16.5;
    let rem = MoonTheme::active_tokens(cx).base_font_size();
    MONTH_GRID_W.max(DAY_GRID_REMS * rem) + 1.5 * rem + 2.0
}

/// Width of a `MoonPopupMenu` (Compact) row carrying a `MoonMenuItem::right_label`.
///
/// [`menu_fit_width`] reserves one item gap. A row with a right label has four children —
/// check slot, label, flex spacer, and right label — so MoonUI applies three gaps. This helper
/// conservatively measures the main label at the compact size of 9.5 and weight 600; MoonUI
/// renders the right label at size 9.0 and weight 400.
///
/// Takes the already-chosen widest pair rather than an iterator: measuring one string costs an
/// uncached `layout_line` per CHARACTER on `&App`, so the caller picks the candidate. In a mono
/// menu that is simply the longest label by character count.
pub fn menu_fit_width_2col(cx: &App, label: &str, right_label: &str, min_w: f32) -> f32 {
    let content = ui_text_width(cx, label, 9.5, 600.0, true)
        + ui_text_width(cx, right_label, 9.0, 400.0, true);
    (ui_value(cx, 6.0 * 2.0 + 5.0 * 3.0 + 4.0 * 2.0) + 12.0 + 2.0 + content)
        .ceil()
        .clamp(min_w, font_w(cx, MENU_MAX_W).max(min_w))
}

/// Visual horizontal padding around a `MoonDropdown` trigger label.
///
/// The Action and Toolbar button sizes used by these content-sized dropdowns have zero horizontal
/// padding, so this allowance supplies it. `fit_dropdown_trigger` scales the value with `ui_value`,
/// following UI scale rather than Font-slider scale.
const TRIGGER_PAD_X: f32 = 14.0;

/// Unscaled visual ceiling for a `MoonDropdown` trigger in a compact panel toolbar.
///
/// Long labels are truncated before they push adjacent controls beyond the clipped panel edge; the
/// full name remains available in the open menu. Callers scale this base through `font_w`.
const TRIGGER_MAX_W: f32 = 260.0;

/// Unscaled minimum trigger width for the core selectors used by `dropdown_content_widths`.
///
/// The trigger grows with its content up to `TRIGGER_MAX_W`; Log panel fields provide their own
/// lower bounds.
pub const CORES_TRIGGER_MIN_W: f32 = 118.0;

/// Unscaled coarse ceiling for selector-menu width.
///
/// It prevents a pathological core name from expanding a menu across much of the screen.
/// `MoonDropdown` repositions a popover but does not shrink it. This fixed guard is not the actual
/// window width, so at a large font a menu can still extend beyond a very narrow detached window.
/// Normal core names do not approach it. Callers scale this base through `font_w`.
const MENU_MAX_W: f32 = 560.0;

/// Calculate content-driven `MoonDropdown` trigger and menu widths.
///
/// Returns the trigger label with its caret, the trigger width, and the menu width. `cur` excludes
/// the caret; `menu_labels` contains every menu label. The trigger is bounded by `min_trigger_w`
/// and `TRIGGER_MAX_W`, truncating with an ellipsis at the ceiling; the menu is bounded by
/// `min_menu_w` and `MENU_MAX_W`.
pub fn dropdown_content_widths<'a>(
    cx: &App,
    cur: &str,
    menu_labels: impl IntoIterator<Item = &'a str>,
    min_trigger_w: f32,
    min_menu_w: f32,
) -> (String, f32, f32) {
    let (label, trigger_w) = fit_dropdown_trigger(
        cx,
        cur,
        font_w(cx, min_trigger_w),
        font_w(cx, TRIGGER_MAX_W),
    );
    let menu_w = menu_fit_width(cx, menu_labels, font_w(cx, min_menu_w));
    (label, trigger_w, menu_w)
}

/// Build a dropdown label inside one fixed, font-scaled trigger width.
///
/// Unlike [`dropdown_content_widths`], this never derives geometry from the current selection.
/// Multi-select menus use it so an open popover remains anchored while its summary changes.
///
/// Args:
///     cx: Application context used for text measurement and font scaling.
///     cur: Current label without the caret.
///     width: Unscaled fixed trigger width.
///
/// Returns:
///     The fitted label with its caret and the font-scaled fixed width.
pub fn fixed_dropdown_trigger(cx: &App, cur: &str, width: f32) -> (String, f32) {
    let width = font_w(cx, width);
    fit_dropdown_trigger(cx, cur, width, width)
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

/// [`fit_text`] at the size a selector pill draws its label.
///
/// For labels that carry their own chrome (a `MoonSelectorPill` draws its own caret), where
/// [`fit_dropdown_trigger`]'s caret-and-width contract does not apply.
pub fn fit_label(cx: &App, text: &str, max_w: f32) -> String {
    fit_text(text, max_w, |s| ui_text_width(cx, s, 10.5, 400.0, true)).0
}

/// Build a `MoonDropdown` trigger label and width for the current selection.
///
/// The width grows between `min_w` and `max_w`. At the ceiling, the monospaced label is truncated
/// by measured glyph width so fallback Unicode glyphs remain within the budget; the ellipsis and
/// caret remain visible. `cur` excludes the caret.
fn fit_dropdown_trigger(cx: &App, cur: &str, min_w: f32, max_w: f32) -> (String, f32) {
    const CARET: &str = " \u{25be}"; // space plus down-pointing caret
    let tw = |s: &str| ui_text_width(cx, s, 10.5, 400.0, true);
    let full = format!("{cur}{CARET}");
    let natural = (ui_value(cx, TRIGGER_PAD_X) + tw(&full)).ceil();
    if natural <= max_w {
        return (full, natural.max(min_w));
    }
    // Above the ceiling, truncate the name while preserving the ellipsis and caret. Core names are
    // arbitrary Unicode, and fallback glyphs outside Geist Mono need not be equal-width, so build
    // the prefix from each character's measured width until `prefix + ellipsis + caret` fills the
    // budget without overflowing the button.
    let suffix = format!("\u{2026}{CARET}"); // ellipsis plus space and down-pointing caret
    let budget = max_w - ui_value(cx, TRIGGER_PAD_X) - tw(&suffix);
    let mut head = String::new();
    let mut used = 0.0f32;
    let mut buf = [0u8; 4];
    for ch in cur.chars() {
        let w = tw(ch.encode_utf8(&mut buf));
        if used + w > budget {
            break;
        }
        used += w;
        head.push(ch);
    }
    (format!("{}{}", head.trim_end(), suffix), max_w)
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
/// The result is one when the Settings Font slider has zero delta. Geometry passed through `ui()`
/// does not see that delta, so fixed-width text containers such as value fields, popups, and inputs
/// use this factor to avoid clipping or wrapping larger text.
///
/// Args:
///     cx: Application context used to read active theme tokens.
///
/// Returns:
///     The active scaled base font size divided by the unscaled base.
pub fn font_scale(cx: &App) -> f32 {
    let b = base_text(cx);
    font_value(cx, b) / b
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
    px(base * font_scale(cx))
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
    base * font_scale(cx)
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
    div()
        .w(ui_px(cx, 5.0))
        .h(ui_px(cx, 5.0))
        .rounded(ui_px(cx, 999.0))
        .bg(solid(color))
}
