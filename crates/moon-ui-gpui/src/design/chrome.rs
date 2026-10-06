//! Tone mapping, chrome bands and header controls of the design.

use super::*;

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
/// matching raw text sized through [`t_body`] (the body-text channel) and chrome such as
/// [`vline`].
pub const DISCLOSURE_GLYPH: f32 = 11.0;

/// The cross that drops one row: THE spelling for a new control that needs one.
///
/// A glyph rather than an icon asset, matching the tuner's filter and time grids, the Alerts table
/// and the strategy tree. Those still carry their own literals — this is where the next one belongs
/// and where a sweep would start, not a claim that the sweep has happened; the tree also holds a
/// few `×`, which is the divergence a single home exists to stop growing.
pub const GLYPH_CLOSE: &str = "✕";

/// Base (unscaled) glyph edge for a passive disclosure caret whose enclosing row owns the click.
///
/// Pass this value directly to `MoonDisclosure`; its `caret_box` applies the UI scale. This keeps
/// the marker on the UI slider, matching [`t_body`]'s body-text channel.
pub const DISCLOSURE_GLYPH_MARKER: f32 = 9.0;

/// Base (unscaled) square box around either disclosure caret.
///
/// Pass this value directly to `MoonDisclosure`; its `caret_box` applies the UI scale. Sharing the
/// value keeps a caret aligned with a neighbouring `glyph_btn` cell in the same toolbar row.
pub const DISCLOSURE_BOX: f32 = 12.0;

/// Height of the toolbar strip.
///
/// The one home of the toolbar's band, matching [`header_height`] and the
/// `table_row_h`/`table_head_h` pair below: MoonUI's `fit_band` over the design's base height,
/// so the band grows with the font delta exactly as the row it holds. Centralizing the call
/// prevents callers that size or position adjacent chrome from drifting away from the row that is
/// actually rendered.
pub fn toolbar_height(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).fit_band(TOOLBAR_H, 13.0)
}

/// Height of the window header strip — the companion to [`toolbar_height`], the same `fit_band`
/// over the header's base height. Drifting the two apart is exactly the failure this shared shape
/// exists to prevent.
pub fn header_height(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).fit_band(HEADER_TOP_H, 14.0)
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

// ---- goal A: chrome group rhythm (header + toolbar) ----

/// Height of [`chrome_divider`]'s rule, tall enough to read as a group boundary against the row's
/// own chips at 100% zoom.
///
/// A 26px-tall chip made a 16px rule read as decoration rather than a seam; 20 of that 26 draws a
/// rule that visibly interrupts the row instead of floating inside it. Goes through [`ui_px`], like
/// every other chrome-strip dimension — it follows the UI slider, matching [`vline`]'s own scaling
/// rule.
pub const CHROME_RULE_H: f32 = 20.0;

/// Colour for a readout that may be empty: full `p.text` when a value is present, `p.text_muted`
/// when it is not.
///
/// Without this, an absent Lev/MAX/SL readout renders a bare dash at the same weight as a live
/// figure, so the operator cannot tell "nothing set" from "reading zero" at a glance.
pub fn readout_color(p: MoonPalette, present: bool) -> u32 {
    if present { p.text } else { p.text_muted }
}

/// The mark a selector shows where a selection of several disagrees on its value — the strategies
/// window's own, and the expert core-settings window's, so one glyph means one thing.
pub const MIXED_MARK: &str = "≠";

/// Tone of a control whose value differs across the selection it edits, or the plain one.
pub fn mixed_tone(mixed: bool) -> MoonTone {
    if mixed {
        MoonTone::Warning
    } else {
        MoonTone::Default
    }
}

/// Trigger variant of a dropdown whose value differs across the selection it edits. Separate from
/// [`mixed_tone`] because `MoonDropdown` has a variant and no tone.
pub fn mixed_trigger_variant(mixed: bool) -> MoonButtonVariant {
    if mixed {
        MoonButtonVariant::Amber
    } else {
        MoonButtonVariant::Soft
    }
}

/// Tone for a chrome toggle whose ON state should read as a caution rather than an ordinary
/// affordance.
///
/// Mirrors `chrome/quiet.rs`'s own toggle exactly, so the sleep toggle, the own-trade toggle and
/// the SL toggle resolve their tone from one place and cannot drift apart.
///
/// `None` is the toggle's OWN fill rather than the absence of a decision, and under colour roles it
/// is the right one: MoonUI fills a default track with `bg_brand_solid` and steps it to
/// `bg_brand_solid_hover` under the pointer, while an explicit tone paints one flat colour in both
/// states. Pinning `Info` therefore cost these three toggles their hover — the gallery's toggles set
/// no tone, which is why they answer the pointer and the terminal's did not.
///
/// A legacy palette keeps the explicit `Info`. Its default fill is `accent` — amber on the terminal
/// theme — so dropping the tone there would repaint a blue toggle in the accent colour rather than
/// restore a hover it never had: `from_palette` gives that theme no distinct hover fill to step to.
///
/// Args:
///     on: Whether the toggle is checked.
///     caution: Whether this toggle's ON state reads as a caution.
///     roles: Whether the active theme installs colour roles, from [`theme_installs_roles`].
///
/// Returns:
///     The tone to pin, or `None` to take the toggle's own fill.
pub fn chrome_toggle_tone(on: bool, caution: bool, roles: bool) -> Option<MoonTone> {
    if on && caution {
        Some(MoonTone::Warning)
    } else if roles {
        None
    } else {
        Some(MoonTone::Info)
    }
}

/// Whether the active theme installs MoonUI's colour roles rather than deriving them from a legacy
/// palette.
///
/// The experimental Dark and Light modes install `MoonThemeConfig::moon_color_modes`, which is what
/// carries the roles; every other theme leaves `colors` unset and resolves roles from its palette.
/// Components ask this where the two want different treatment rather than naming the modes, so a
/// fourth role-carrying theme needs no change here.
pub fn theme_installs_roles(cx: &App) -> bool {
    MoonTheme::active_tokens(cx).colors.is_some()
}

/// Label colour for a chrome toggle whose ON state should read as a caution rather than an
/// ordinary affordance.
///
/// Mirrors `chrome/quiet.rs`'s own toggle label exactly; see [`chrome_toggle_tone`].
pub fn chrome_toggle_label_color(p: MoonPalette, on: bool, caution: bool) -> u32 {
    if on && caution { p.amber } else { p.text_soft }
}

/// Foreground for a SECONDARY chrome label that still has to be read at a glance: an inactive tab
/// title, a table column header, the pinned-scope chip.
///
/// One step above `p.text_muted`, which measures 3.5-3.8:1 against every chrome surface in both
/// stock themes and therefore sits under the 4.5:1 body floor; `p.text_soft` measures 6.7-7.0:1 on
/// light and 5.1-5.5:1 on dark. `p.text_dim` is NOT the step up: the dark palette defines it as the
/// same value as `p.text`, so using it here would erase the active/inactive distinction in dark
/// mode while looking correct in light mode.
///
/// This is the label tone only. The ACTIVE member of a pair keeps `p.text`, so raising the
/// inactive one narrows the gap rather than closing it.
///
/// Args:
///     p: Active palette whose secondary label tone is being resolved.
///
/// Returns:
///     The accessible secondary-label colour.
pub fn chrome_label_color(p: MoonPalette) -> u32 {
    p.text_soft
}

/// Height of a chrome tab strip, in pixels at the current UI and font scale.
///
/// One source for `MoonTabStrip`'s own rendered tab-height metric, delegated to
/// `MoonTabStrip::strip_height` rather than a hand copy. Standard renders today's number exactly;
/// a strip whose row is a different height from the tabs inside it puts the active-tab underline
/// off the row's bottom edge. Every strip in the app resolves it here — the two chart strips
/// through [`chart_tab_strip_h`](crate::chart_tabs::chart_tab_strip_h), which delegates to
/// [`tab_strip_h_value`], and the three window strips directly.
///
/// Args:
///     cx: Application context supplying the current UI and font scales.
///
/// Returns:
///     The scaled tab-strip height.
pub fn tab_strip_h(cx: &App) -> Pixels {
    px(tab_strip_h_value(cx))
}

/// [`tab_strip_h`] as a bare number, for a caller that needs the value rather than a length.
///
/// Args:
///     cx: Application context supplying the current UI and font scales.
///
/// Returns:
///     The scaled tab-strip height as a raw number.
pub fn tab_strip_h_value(cx: &App) -> f32 {
    MoonTabStrip::strip_height(&MoonTheme::active_tokens(cx))
}

/// Renders one chrome tab strip through the lifted label palette and the LIVE theme tokens.
///
/// The one place the three-line incantation lives, because getting it wrong is silent:
/// `render_with_palette` is one argument shorter, compiles, and substitutes default tokens, which
/// drops the user's font delta and UI scale — the labels shrink and the strip stops matching
/// [`tab_strip_h`], putting the active-tab underline out of line. The palette and tokens are read
/// into locals first because they cannot be read from `cx` in the same argument list that hands
/// `cx` over mutably, and the result is boxed because it would otherwise hold that `&mut cx`
/// borrow for as long as the element lives, which every caller still needs.
///
/// Returns the strip BARE, with no sizing wrapper: a caller inside an already-sized row wants it
/// that way, and one placing it as a top-level child adds its own [`tab_strip_h`] box.
///
/// Args:
///     strip: Configured tab-strip builder to render.
///     p: Active palette whose muted label tone will be lifted.
///     window: Window that owns the strip's persistent overflow state.
///     cx: Application context supplying the current theme tokens.
///
/// Returns:
///     The themed, lifted tab strip without a sizing wrapper.
pub fn chrome_tab_strip(
    strip: moon_ui::MoonTabStrip,
    p: MoonPalette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let palette = chrome_label_palette(p);
    let tokens = MoonTheme::active_tokens(cx);
    strip
        .render_with_theme(window, cx, palette, tokens)
        .into_any_element()
}

/// `p` with its muted text lifted to [`chrome_label_color`].
///
/// For MoonUI chrome that keys an inactive label off `text_muted` and exposes no per-item colour
/// prop — `MoonTabStrip` is the case that needs it. Exactly one field moves, so every other colour
/// the widget draws still comes from the same palette as the row around it.
///
/// Args:
///     p: Active palette to copy.
///
/// Returns:
///     A copy with only `text_muted` raised to the chrome label colour.
pub fn chrome_label_palette(p: MoonPalette) -> MoonPalette {
    MoonPalette {
        text_muted: chrome_label_color(p),
        ..p
    }
}

/// The one table style every `MoonDataTable` in this crate attaches: the palette's own fills and
/// selection, with the column-header text lifted to [`chrome_label_color`].
///
/// `MoonDataTable::render` re-themes any style it is handed, but that pass only replaces a field
/// still holding the stock dark default, and the lifted `header_text` never equals it — so the
/// lift survives. Do not call `themed` here; it would be a no-op today and a silent reset the day
/// the defaults move.
///
/// One consequence was weighed and accepted rather than overlooked: MoonUI already draws the
/// SORTED column's header at this same tone, bypassing `header_text` entirely, so raising the
/// unsorted ones makes both read alike and leaves the sort arrow as the only sorted cue. The arrow
/// is part of the header's own text run and unambiguous; the alternative was leaving nine tables'
/// headings under the contrast floor to preserve a second, weaker signal.
///
/// Args:
///     p: Active palette supplying the table's non-header colours.
///
/// Returns:
///     The table style with an accessible column-header colour.
pub fn table_style(p: MoonPalette) -> MoonTableStyle {
    MoonTableStyle {
        header_text: chrome_label_color(p),
        ..MoonTableStyle::for_palette(p)
    }
}

/// Icon standing for "which columns does this table show", on every column selector in the app.
///
/// All six pickers — Orders, Figures, Assets, the Screener, the Analytics tuner list and the
/// Report toolbar — draw THIS asset, for the reason [`CORE_COMPACT_ICON`] gives for the compact
/// core selector: the trigger carries no label, so the icon IS the name, and a second glyph for
/// the same concept would read as a different control.
///
/// It replaced `▦` (U+25A6), which is absent from the default Windows font stack and rendered as
/// two hollow squares. The embedded MoonUI set carries no `filter`, `columns` or `sliders` icon;
/// `layout-dashboard` is its nearest reading — a table's own layout is exactly what these menus
/// change — and it is the only candidate that does NOT collide with `icons/settings-2.svg`, which
/// the Orders toolbar already draws on the sort/settings dropdown standing right beside its
/// column picker. `eye.svg` was rejected: it reads as row visibility, not column layout.
///
/// The trigger is left CHILDLESS at every site so `MoonButton` takes its square icon-only layout;
/// [`glyph_btn_w`] then keeps the cell square on the UI slider while `MoonButton::render` sizes
/// the icon from the size preset's own font metrics.
pub const COLUMN_SELECTOR_ICON: &str = "icons/layout-dashboard.svg";

/// Rendered width that makes a SQUARE one-symbol button — the report export (`⇩`) and the
/// column selectors, which draw [`COLUMN_SELECTOR_ICON`] rather than a glyph.
///
/// It returns the button's own drawn height, so the caller must pass it to a RENDERED width
/// (`MoonDropdown::trigger_width`, `MoonButton::width`), never to a `*_scaled` variant: MoonUI
/// scales a scaled trigger width by `font()` (which adds the legacy font delta) while it scales
/// the height by `ui()` (a pure multiply), so the two diverge as soon as that delta leaves zero.
///
/// Reads [`action_control_h_value`]: a square control matches the ordinary button height at the
/// design's control tier.
pub fn glyph_btn_w(cx: &App) -> f32 {
    action_control_h_value(cx)
}

/// Rendered size of a control-tier button's leading glyph.
///
/// Reads `MoonButtonSize::tier_icon_size` for [`CONTROL_TIER`]. `MoonButton` and `MoonDropdown`
/// apply it themselves; this exists for the callers that draw their own trigger content and must
/// leave the component's room for it.
pub fn action_icon_px(cx: &App) -> f32 {
    ui_value(cx, MoonButtonSize::tier_icon_size(CONTROL_TIER))
}

/// Rendered width a control-tier leading glyph takes, including the gap after it.
///
/// The gap is [`CONTROL_TIER`]'s `control_metrics().gap`; see [`action_icon_px`] for the glyph.
pub fn action_icon_reservation(cx: &App) -> f32 {
    action_icon_px(cx) + ui_value(cx, CONTROL_TIER.control_metrics().gap)
}
