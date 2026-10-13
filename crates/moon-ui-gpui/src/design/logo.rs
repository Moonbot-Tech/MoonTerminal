//! The MoonTerminal lockup: sizes, cuts and paths.

use super::*;

/// Factor carrying every lockup size across the brand swap, from the 199px-wide Moonbot mark the
/// sizes below were tuned against to the 283.23px-wide MoonTerminal one.
///
/// The port keeps the ICON's drawn size rather than the box it sat in: only the wordmark got
/// longer, and holding a width tuned for the shorter mark would shrink the whole lockup by a third
/// and read as a different, smaller logo. Spelling the factor ONCE is the load-bearing part —
/// these five sizes live in two files, and hand-multiplying each one is how four of them stay
/// right while the fifth quietly does not.
const LOGO_PORT_SCALE: f32 = LOGO_SRC_W / 199.0;

/// Width of the placeholder lockup a chart stack draws while it holds no charts.
pub const EMPTY_STACK_LOGO_W: f32 = 220.0 * LOGO_PORT_SCALE;

/// Share of a chart slot's width the same placeholder covers, bounded by [`CHART_LOGO_MIN_W`] and
/// [`CHART_LOGO_MAX_W`] — a floor so it stays legible in a small pane, a ceiling so it stays a
/// watermark in a large one.
///
/// The floor is ported like everything else: leaving it where it was would have kept exactly the
/// small panes — the ones that need it — drawing an icon a third smaller than before. It buys that
/// at the far end: a pane narrower than the floor itself now clips the LOCKUP, where the shorter
/// mark still fitted. That pane is under 256 logical px wide — narrower than the placeholder it
/// would be showing — and between there and 643 px, where the share takes over, the floor is what
/// keeps the icon at its designed size. The glow frame is [`LOGO_GLOW_SCALE`] times these widths
/// and overhangs sooner, which costs nothing visible: its aura reaches zero opacity at that edge.
pub const CHART_LOGO_SLOT_SHARE: f32 = 0.28 * LOGO_PORT_SCALE;
pub const CHART_LOGO_MIN_W: f32 = 180.0 * LOGO_PORT_SCALE;
pub const CHART_LOGO_MAX_W: f32 = 280.0 * LOGO_PORT_SCALE;

/// Y offset of the chart's top-left corner buttons — pin, compare lock, broom — from the plot's
/// top edge, in logical pixels.
pub const CHART_CORNER_BTN_TOP: f32 = 3.0;
/// Side of one such square corner button, in logical pixels.
pub const CHART_CORNER_BTN_SIZE: f32 = 15.0;
/// Height the top-left button strip occupies below the plot's top edge. The left-aligned
/// `ChartTop` captions start below this, so a caption is never painted under a button.
pub const CHART_CORNER_STRIP_H: f32 = CHART_CORNER_BTN_TOP + CHART_CORNER_BTN_SIZE;

/// Header lockup width: the header strip is height-constrained, so the longer wordmark has to buy
/// its room in width, or the mark shrinks inside a row whose height cannot follow it.
const HEADER_LOGO_W: f32 = 86.0 * LOGO_PORT_SCALE;

/// How much wider than the lockup the glow canvas is; the aura fills the difference.
const LOGO_GLOW_SCALE: f32 = 1.2;
/// The glow canvas is SQUARE, so the aura reads as a disc rather than a flat ellipse.
const LOGO_GLOW_VIEW: f32 = LOGO_SRC_W * LOGO_GLOW_SCALE;

/// Return the lockup cut that belongs to a colour scheme.
///
/// Each file carries its own wordmark colour, so the scheme picks a FILE; nothing recolours the
/// brand at runtime, and a custom palette therefore cannot repaint it.
fn logo_svg(light: bool) -> &'static str {
    if light { LOGO_SVG_LIGHT } else { LOGO_SVG_DARK }
}

/// Return everything the lockup's root `<svg>` wraps, for embedding in a composed document.
///
/// The cut is the root tag itself, not the first `<path>`: an export is free to wrap its paths in
/// a `<g>` or to declare a `<defs>`/`<clipPath>` ahead of them, and starting at the first path
/// would drop those opening tags while keeping their closing ones — unbalanced markup usvg rejects
/// outright, which paints NOTHING and looks exactly like a decode failure. Whatever the file
/// carries travels as-is; a `<title>` among it is inert inside a rasterized document.
///
/// Returns an empty string if the file has no root tag to cut at, which the asset test catches.
pub(super) fn logo_paths(svg: &str) -> &str {
    svg.split_once("<svg")
        .and_then(|(_, rest)| rest.split_once('>'))
        .and_then(|(_, inner)| inner.rsplit_once("</svg>"))
        .map_or("", |(inner, _)| inner)
}

/// Wrap SVG source in the GPUI image every element builder here takes.
fn svg_image(svg: impl Into<Vec<u8>>) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into()))
}

/// The brand cuts as decoded images, indexed by `is_light()`.
///
/// Built once because there are only ever two of them. GPUI keys its raster cache on a hash of the
/// bytes, so consecutive frames already shared one texture; what repeated per frame was the 6.9KB
/// copy and the hash upstream of that cache, inside the header's own render.
static LOGO_IMAGES: LazyLock<[Arc<Image>; 2]> =
    LazyLock::new(|| [svg_image(logo_svg(false)), svg_image(logo_svg(true))]);

/// The composed glow documents, indexed by `is_light()`.
///
/// Two, for the reason [`LOGO_IMAGES`] gives: the colour scheme is the document's ONLY input. The
/// caller's `width` never enters it — that sizes the element afterwards — so re-composing this
/// ~7.5KB string per empty pane per frame produced the same two documents forever.
static GLOW_IMAGES: LazyLock<[Arc<Image>; 2]> =
    LazyLock::new(|| [svg_image(glow_svg(false)), svg_image(glow_svg(true))]);

/// The header lockup: the brand at the left end of the main window's titlebar.
///
/// Sized off [`HEADER_LOGO_W`] through `ui()` — the UI-scale track MoonUI's own brand used — so the
/// mark follows the interface scale but NOT the font slider, matching the separator beside it.
///
/// The asset is decoded whole, unlike in [`glow_svg`], which has to splice it into a document of
/// its own: here the file IS the document, so there is nothing to rewrite.
pub fn header_logo(cx: &App) -> impl IntoElement {
    let width = ui_value(cx, HEADER_LOGO_W);
    img(LOGO_IMAGES[usize::from(MoonPalette::active(cx).is_light())].clone())
        .w(px(width))
        .h(px(width * (LOGO_SRC_H / LOGO_SRC_W)))
}

/// Compose the glow document for one colour scheme: the aura disc with the lockup centred on it.
fn glow_svg(light: bool) -> String {
    let paths = logo_paths(logo_svg(light));
    let centre = LOGO_GLOW_VIEW * 0.5;
    let logo_x = (LOGO_GLOW_VIEW - LOGO_SRC_W) * 0.5;
    let logo_y = (LOGO_GLOW_VIEW - LOGO_SRC_H) * 0.5;
    let (aura_0_color, aura_1_color, aura_2_color, aura_0, aura_1, aura_2) = if light {
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
    format!(
        r##"<svg width="{view}" height="{view}" viewBox="0 0 {view} {view}" fill="none" xmlns="http://www.w3.org/2000/svg">
<defs>
  <radialGradient id="brand_aura" cx="50%" cy="50%" r="50%">
    <stop offset="0%" stop-color="{aura_0_color}" stop-opacity="{aura_0:.3}"/>
    <stop offset="34%" stop-color="{aura_1_color}" stop-opacity="{aura_1:.3}"/>
    <stop offset="68%" stop-color="{aura_2_color}" stop-opacity="{aura_2:.3}"/>
    <stop offset="100%" stop-color="{aura_2_color}" stop-opacity="0"/>
  </radialGradient>
</defs>
<circle cx="{centre}" cy="{centre}" r="{centre}" fill="url(#brand_aura)"/>
<g transform="translate({logo_x} {logo_y})">{paths}</g>
</svg>"##,
        view = LOGO_GLOW_VIEW,
    )
}

/// An empty chart surface's opaque cover, carrying the brand when the reader wants one.
///
/// The COVER is not decoration and is never optional. In a chart slot it hides a stale graph left
/// in the chart's own GPU pass beneath the GPUI scene; in a detached window, whose root is
/// `NoFill`, it hides the white window backing. Only the MARK follows the "show the logo" switch —
/// and that is exactly why the two are built here rather than at each call site, where one edit
/// could gate the plate along with the mark and turn "hide the logo" into "reveal a dead chart".
///
/// Args:
///     cx: Application context, for the scaled artwork.
///     background: Packed chart background colour, from the active palette.
///     logo: The lockup's width when the mark is drawn, `None` when the reader switched it off.
///
/// Returns:
///     The cover, for a caller to place — a stack fills its parent, a chart slot lays it over one.
pub fn empty_cover(cx: &App, background: u32, logo: Option<f32>) -> Div {
    div()
        .size_full()
        .bg(rgb(background))
        .flex()
        .items_center()
        .justify_center()
        .when_some(logo, |cover, width| cover.child(logo_glow_sized(cx, width)))
}

/// The glowing placeholder an empty chart surface draws; `width` is the lockup's own width.
///
/// The element is the square glow canvas around it, hence the [`LOGO_GLOW_SCALE`] factor: taking
/// the LOCKUP width keeps every caller's number comparable to the artwork rather than to a frame
/// whose size the aura decides.
pub fn logo_glow_sized(cx: &App, width: f32) -> impl IntoElement {
    let frame_w = logo_glow_frame_w(cx, width);
    img(GLOW_IMAGES[usize::from(MoonPalette::active(cx).is_light())].clone())
        .w(frame_w)
        .h(frame_w)
}

/// Width of the square glow canvas [`logo_glow_sized`] draws a lockup of `width` on: what the mark
/// actually occupies, which is what a layout reserving room for it has to ask.
///
/// Args:
///     cx: Application context, for parity with the drawing side; the frame is a lockup-width
///         multiple, not a font-scaled size.
///     width: Lockup width, as handed to [`logo_glow_sized`].
pub fn logo_glow_frame_w(_cx: &App, width: f32) -> Pixels {
    px(width * LOGO_GLOW_SCALE)
}
