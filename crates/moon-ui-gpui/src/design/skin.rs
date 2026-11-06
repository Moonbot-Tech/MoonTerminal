//! The terminal's hand-painted surfaces under MoonUI's Facet skin.
//!
//! MoonUI restyles its own components when the active theme carries `MoonSkin::Facet`. The
//! terminal also paints surfaces by hand — the two chrome rows, panel bands, cards, tile frames —
//! and this module holds what those need to follow the skin: the skin query, the lit colours
//! derived from the palette, and the fading rules that stand in for solid one-pixel borders.
//!
//! Every caller reaches these helpers behind [`is_facet`], or hands its Flat element to
//! [`panel_rule`], which gives it back untouched. A theme on the Flat skin therefore builds
//! exactly the element tree it built before skins existed.

use super::*;
use moon_ui::{MoonFacetKind, MoonSkin, facet, fading_rule_h, fading_rule_v, moon_ambient_light};

/// Strength of the highlight ink in the rule that closes a chrome band: the toolbar's bottom
/// edge and the bottom edge of a tool window's header.
pub const FACET_CHROME_RULE: f32 = 0.11;

/// Strength of the highlight ink in the rule between two bands of one panel.
pub const FACET_PANEL_RULE: f32 = 0.07;

/// Strength of the highlight ink along the top of a section heading's sheen: an exchange heading
/// in the Auto rail, in the Core Status table or in the Assets roster. The value a chart tile's
/// header is lit with, so every heading band inside a surface reads alike.
pub const FACET_SECTION_SHEEN: f32 = 0.05;

/// Strength of the highlight ink where the header row meets the toolbar row.
///
/// Both rows read this one value — the header ends on it and the toolbar starts from it — so the
/// two gradients meet without a step and the pair reads as a single band lit from above.
const FACET_CHROME_SEAM: f32 = 0.03;

/// Strength of the warm ambient light of the chrome band, at its centre.
///
/// A tool window carries the theme's ambient light in its backdrop. A group window has no
/// backdrop — its root paints nothing, so the chart can show through — and carries the same pair
/// of lights in its chrome band instead: the accent toward the left, blue toward the right. Both
/// are kept weaker than the backdrop's, because the band stands right above the chart.
const FACET_BAND_LIGHT_WARM: f32 = 0.07;

/// Strength of the cool ambient light of the chrome band, at its centre.
const FACET_BAND_LIGHT_COOL: f32 = 0.06;

/// How far from the band's left edge the warm light is centred, as a fraction of its width.
const FACET_BAND_LIGHT_WARM_AT: f32 = 0.24;

/// How far from the band's right edge the cool light is centred, as a fraction of its width.
///
/// Far enough in that the light has faded out before the right edge at any window width the
/// toolbar fits in, where a pinned overflow section repeats the plain band fill.
const FACET_BAND_LIGHT_COOL_AT: f32 = 0.28;

/// One of the two rows of a group window's chrome band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetBandRow {
    /// The window header: the upper row.
    Header,
    /// The toolbar: the lower row.
    Toolbar,
}

/// Build the ambient lights of one row of the chrome band.
///
/// Each row paints its own half: the lights are centred on the seam between the two rows, so
/// the halves meet into one pool of light with no step at the seam.
///
/// The result is a layer that fills the row, clips the lights to it and takes no pointer input.
/// The row must be `relative` and must add the layer before its other children, so everything
/// in the row paints over it. Callers reach this behind [`is_facet`].
///
/// Args:
///     row: Which row of the band the layer is for.
///     cx: Application context holding the theme global.
///
/// Returns:
///     The light layer for that row.
pub fn facet_band_lights(row: FacetBandRow, cx: &App) -> Div {
    let tokens = MoonTheme::active_tokens(cx);
    let p = tokens.palette;
    // A light is one pixel. In the header it hangs just below the row and in the toolbar it sits
    // on the row's first line: the same line of the window either way.
    let on_seam = |light: Div| match row {
        FacetBandRow::Header => light.bottom(px(-1.0)),
        FacetBandRow::Toolbar => light.top_0(),
    };
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .overflow_hidden()
        .child(
            on_seam(moon_ambient_light(p.accent, FACET_BAND_LIGHT_WARM, &tokens))
                .left(relative(FACET_BAND_LIGHT_WARM_AT)),
        )
        .child(
            on_seam(moon_ambient_light(p.blue, FACET_BAND_LIGHT_COOL, &tokens))
                .right(relative(FACET_BAND_LIGHT_COOL_AT)),
        )
}

/// Return whether the active theme draws with the Facet skin.
///
/// Args:
///     cx: Application context holding the theme global.
///
/// Returns:
///     `true` under `MoonSkin::Facet`, `false` under `MoonSkin::Flat` or with no theme installed.
pub fn is_facet(cx: &App) -> bool {
    MoonSkin::active(cx).is_facet()
}

/// Return the Facet highlight ink: the palette's `overlay` at `alpha`.
///
/// Args:
///     p: Active palette.
///     alpha: Opacity of the ink, from 0 to 1.
///
/// Returns:
///     The translucent ink.
pub fn facet_ink(p: MoonPalette, alpha: f32) -> Hsla {
    rgba_from(p.overlay, alpha)
}

/// Return an opaque colour: `base` with the highlight ink layered over it at `alpha`.
///
/// A stand-in for MoonUI's `lift`, which `moon/skin.rs` keeps private: the terminal composites
/// the same ink over the base instead. On a dark palette `overlay` is white and the two are the
/// same colour. Once MoonUI exports `lift`, or a facet for a window's chrome band, the two chrome
/// fills below should come from there and this helper should go.
///
/// Args:
///     p: Active palette, for the ink.
///     base: The colour underneath, as `0xRRGGBB`.
///     alpha: Opacity of the ink over the base, from 0 to 1.
///
/// Returns:
///     The composited colour, fully opaque.
pub fn facet_lit(p: MoonPalette, base: u32, alpha: f32) -> Hsla {
    rgba_from(base, 1.0).blend(facet_ink(p, alpha))
}

/// Build a vertical two-stop gradient running from `top` at the top edge to `bottom` at the
/// bottom edge.
pub fn facet_band(top: Hsla, bottom: Hsla) -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(top, 0.0),
        linear_color_stop(bottom, 1.0),
    )
}

/// Build the fill of the window header row: the upper half of the chrome's top-lit band.
///
/// Opaque, because the group window's root paints no fill of its own.
pub fn facet_header_fill(p: MoonPalette) -> Background {
    facet_band(
        facet_lit(p, p.shell, 0.06),
        facet_lit(p, p.shell, FACET_CHROME_SEAM),
    )
}

/// Build the fill of the toolbar row: the lower half of the chrome's top-lit band, settling to
/// the plain window fill.
///
/// Opaque, for the same reason as [`facet_header_fill`].
pub fn facet_toolbar_fill(p: MoonPalette) -> Background {
    facet_band(facet_lit(p, p.shell, FACET_CHROME_SEAM), moon(p.shell))
}

/// Build a sheen: the highlight ink at `alpha` along the top edge, fading to nothing at the
/// bottom edge.
///
/// Translucent, so it lights whatever lies behind it rather than replacing it.
pub fn facet_sheen(p: MoonPalette, alpha: f32) -> Background {
    facet_band(facet_ink(p, alpha), facet_ink(p, 0.0))
}

/// Paint `el` as the faceted surface `kind`: its gradient fill, its one-pixel edge and its
/// shadow stack. The element keeps the corner radius it already has.
///
/// Only for an element that is meant to be opaque: a drop shadow is a blurred filled shape, not
/// a ring, so on an element with no fill it would tint everything behind the element.
///
/// Args:
///     el: The element to paint.
///     kind: Which faceted surface to resolve.
///     cx: Application context holding the theme global.
///
/// Returns:
///     `el` with the surface applied.
pub fn facet_surface<E: Styled>(el: E, kind: MoonFacetKind, cx: &App) -> E {
    let surface = facet(kind, &MoonTheme::active_tokens(cx));
    surface.paint(el.border_1())
}

/// Stand a fading rule in for the solid one-pixel bottom border of `el`.
///
/// The border keeps its width and turns transparent, so `el` lays out exactly as it does under
/// the Flat skin. The rule is then laid over the pixel row the border occupied: an absolute
/// child is placed inside its parent's border, hence the one-pixel outset.
///
/// `el` must carry a bottom border and no other, because the transparent colour reaches every
/// side.
///
/// Args:
///     el: The element whose bottom border becomes the rule.
///     color: The colour at the middle of the rule.
///
/// Returns:
///     `el` with the rule added as a child.
pub fn facet_bottom_rule<E: Styled + ParentElement>(el: E, color: Hsla) -> E {
    el.border_color(transparent_black())
        .child(edge_rule(color).bottom(px(-1.0)))
}

/// Stand a fading rule in for the solid one-pixel top border of `el`.
///
/// The top-edge twin of [`facet_bottom_rule`]; `el` must carry a top border and no other.
///
/// Args:
///     el: The element whose top border becomes the rule.
///     color: The colour at the middle of the rule.
///
/// Returns:
///     `el` with the rule added as a child.
pub fn facet_top_rule<E: Styled + ParentElement>(el: E, color: Hsla) -> E {
    el.border_color(transparent_black())
        .child(edge_rule(color).top(px(-1.0)))
}

/// [`facet_top_rule`] with a glow around the rule, for an edge that reports a state.
///
/// The glow is cast by the rule's own one-pixel box, so it stays a thin haze along the edge.
///
/// Args:
///     el: The element whose top border becomes the rule.
///     color: The colour at the middle of the rule.
///     glow: The shadow the rule casts, such as one built by `moon_ui::facet_glow`.
///
/// Returns:
///     `el` with the glowing rule added as a child.
pub fn facet_top_rule_lit<E: Styled + ParentElement>(el: E, color: Hsla, glow: BoxShadow) -> E {
    el.border_color(transparent_black())
        .child(edge_rule(color).top(px(-1.0)).shadow(vec![glow]))
}

/// Build a fading rule spanning the width of its parent, out of flow; the caller pins it to an
/// edge.
fn edge_rule(color: Hsla) -> Div {
    div()
        .absolute()
        .left_0()
        .right_0()
        .child(fading_rule_h(color))
}

/// Stand a fading rule in for the solid one-pixel right border of `el`: the edge of a sidebar or
/// a rail that stands on the left of what it bounds.
///
/// The vertical twin of [`facet_bottom_rule`], built the same way; `el` must carry a right border
/// and no other, and must not scroll its own children, or the rule would scroll with them.
///
/// Args:
///     el: The element whose right border becomes the rule.
///     color: The colour at the middle of the rule.
///
/// Returns:
///     `el` with the rule added as a child.
pub fn facet_right_rule<E: Styled + ParentElement>(el: E, color: Hsla) -> E {
    el.border_color(transparent_black())
        .child(side_rule(color).right(px(-1.0)))
}

/// Stand a fading rule in for the solid one-pixel left border of `el`: the edge of a rail that
/// stands on the right of what it bounds.
///
/// The left-edge twin of [`facet_right_rule`]; `el` must carry a left border and no other.
///
/// Args:
///     el: The element whose left border becomes the rule.
///     color: The colour at the middle of the rule.
///
/// Returns:
///     `el` with the rule added as a child.
pub fn facet_left_rule<E: Styled + ParentElement>(el: E, color: Hsla) -> E {
    el.border_color(transparent_black())
        .child(side_rule(color).left(px(-1.0)))
}

/// Build a fading rule spanning the height of its parent, out of flow; the caller pins it to a
/// side.
fn side_rule(color: Hsla) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .child(fading_rule_v(color))
}

/// Pick the one-pixel separator a panel draws between two of its bands.
///
/// The caller builds the separator it has always drawn and passes it in. Under the Flat skin
/// that element is returned as it is; under Facet a rule of the same footprint — full width, one
/// pixel tall, never shrinking — takes its place and fades toward both ends.
///
/// Args:
///     cx: Application context holding the theme global.
///     p: Active palette, for the rule's ink.
///     flat: The separator of the Flat skin.
///
/// Returns:
///     The separator to add to the panel.
pub fn panel_rule(cx: &App, p: MoonPalette, flat: impl IntoElement) -> AnyElement {
    if is_facet(cx) {
        fading_rule_h(facet_ink(p, FACET_PANEL_RULE)).into_any_element()
    } else {
        flat.into_any_element()
    }
}

/// The clear colour of a window whose root paints no fill, kept on the active theme while the
/// Facet skin is active.
///
/// Such a window shows its clear colour wherever the scene paints nothing, and that colour is
/// set once, when the window opens. Under the Flat skin every chrome surface fills itself, so a
/// theme switched while the window is open leaves the old colour only in the gaps it always
/// showed in. Under Facet the rail, the chart tab strip and the gaps between chart tiles are
/// unfilled on purpose, and the same stale colour would stand there as whole bands of the
/// previous theme. The window's host therefore calls [`Self::sync`] from its render.
///
/// Leaving Facet installs the selected Flat palette, including when the window opened in Facet.
/// Flat-to-Flat changes retain the previous window behaviour.
#[derive(Clone, Copy, Debug)]
pub struct FacetGround {
    /// Whether the previous render used the unfilled Facet chrome.
    was_facet: bool,
    /// The colour last handed to the window.
    applied: u32,
}

impl FacetGround {
    /// Start from the colour the window was opened with.
    ///
    /// Args:
    ///     opened: The clear colour the window's opener installed, as `0xRRGGBB`.
    pub fn new(opened: u32, facet: bool) -> Self {
        Self {
            was_facet: facet,
            applied: opened,
        }
    }

    /// Resolve the colour the window should clear with.
    ///
    /// Args:
    ///     facet: Whether the Facet skin is active.
    ///     ground: The active palette's entry for this window's ground, as `0xRRGGBB`.
    ///
    /// Returns:
    ///     A changed ground on Facet entry, update or exit; no change for Flat-only transitions.
    pub(super) fn update(&mut self, facet: bool, ground: u32) -> Option<u32> {
        let update = (facet || self.was_facet) && self.applied != ground;
        self.was_facet = facet;
        if update {
            self.applied = ground;
            Some(ground)
        } else {
            None
        }
    }

    /// Hand the window its clear colour when the one it holds is not the one it should hold.
    ///
    /// Costs one comparison on a frame that changes nothing, so a host may call it every render.
    ///
    /// Args:
    ///     ground: The active palette's entry for this window's ground, as `0xRRGGBB`: `shell`
    ///         for a group window, `chart_bg` for a chart window.
    ///     window: The window to update.
    ///     cx: Application context holding the theme global.
    pub fn sync(&mut self, ground: u32, window: &Window, cx: &App) {
        if let Some(wanted) = self.update(is_facet(cx), ground) {
            window.set_clear_color(Some(rgb(wanted)));
        }
    }
}
