//! The ONE column specification for the Connections core table.
//!
//! Both the header row and every server row are built from [`ConnColId::ALL`], so a column's
//! width, growth and alignment are stated exactly once. Before this module the two were separate
//! literal lists that happened to carry the same numbers, and they still drew misaligned: a raw
//! `flex_basis` leaves gpui's `min_size: auto`, which clamps a flex item UP to its child's
//! min-content width, and the row's children are live controls that render WIDER than the basis
//! (`MoonDropdown::trigger_width_scaled` multiplies by the Font-slider ratio; `MoonColorPicker`
//! draws a hard 128px trigger). The header's plain text labels never inflate, so only the header
//! had slack left for its growing columns to absorb -- and every column after them drifted.
//!
//! The fix is structural rather than a nudged constant: one list; `min_w_0()` on every cell so the
//! declared basis is the whole truth (Taffy resolves an AUTO minimum from min-content, an explicit
//! zero minimum from the basis); and one [`ConnColWidth`] policy per column saying whether that
//! basis is a RENDERED width or a design reference the control's own scaler multiplies -- because
//! the two kinds genuinely coexist in one row, and scaling all of them would be as wrong as
//! scaling none.
//!
//! The three columns holding user-typed text -- name, key, group -- all GROW, and two of them
//! carry a [`ConnCol::max`]. That is the second thing stated exactly once here, and it is a
//! response to the row having TWO regimes rather than one. `table.rs::cell` gives a non-growing
//! column `flex_shrink_0` as well, so `grow: false` means rigid in both directions; the rigid
//! columns plus the inset, the scrollbar gutter and the twelve gaps come to about 661px of the
//! 824px body the DEFAULT 860px Settings window leaves, against about 487px of text-column bases.
//! So the default window is a SHRINK regime -- the caps are inert there and the BASES decide who
//! keeps what -- while a wide window is a GROW regime, where the caps are what stop the key and
//! the group from spending width they have nothing readable to put in it. Both dials are needed,
//! and neither is the other's fallback. All three carry ONE width policy, which is what makes
//! their 150 > 140 > 85 ordering a property of the literals rather than of the Font slider.
//!
//! Pure and GPUI-free on purpose -- its sibling test file can assert the header and the rows agree
//! without a window.

/// Left inset shared by the header row and by an indented core row.
///
/// The tree indent in `tab.rs` spends it as `ml(8) + pl(11) + border_l_1`; the header spends it as
/// plain padding. Both read it from here so a change to the branch guide cannot move the rows out
/// from under their own headings.
pub(super) const CONN_TABLE_INSET: f32 = 20.0;

/// Left margin of an indented core row, the first part of [`CONN_TABLE_INSET`].
pub(super) const CONN_INDENT_MARGIN: f32 = 8.0;

/// Border width of the branch guide drawn down an indented core row.
pub(super) const CONN_INDENT_BORDER: f32 = 1.0;

/// Left padding between the branch guide and an indented core row's first cell.
///
/// The three indent parts sum to [`CONN_TABLE_INSET`]; `tests` proves it.
pub(super) const CONN_INDENT_PAD: f32 = CONN_TABLE_INSET - CONN_INDENT_MARGIN - CONN_INDENT_BORDER;

/// Where a column's header label sits over the control below it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ConnColAlign {
    /// Text-left, over an input's own text: the label carries [`ConnCol::head_pad`] to match the
    /// input's internal left padding, and the row's control fills the cell.
    Left,
    /// Centred over a checkbox, a dropdown, the data popover or the colour swatch. Applied to the
    /// header cell AND to the row cell, so the two centre on the same axis.
    Center,
}

/// How a column's [`ConnCol::basis`] turns into rendered pixels.
///
/// The row's controls do NOT agree on this, which is exactly why the header could never guess it:
/// `MoonColorPicker` draws a hard `w(px(128.0))` and `MoonButton::width` takes rendered pixels,
/// while `MoonDropdown::trigger_width_scaled` multiplies its design reference by the trigger's own
/// text scale. Naming the policy per column is what lets one basis list serve both builders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ConnColWidth {
    /// The basis IS the rendered width.
    Raw,
    /// A design reference the Micro dropdown trigger scales by `tokens.font(10) / 10`.
    MicroTrigger,
    /// A design reference stated in CHARACTERS of a `MoonInput::small()`, scaled by the same
    /// `tokens.font(10) / 10` ratio. Carried by all three text columns -- `h-name`, `h-key` and
    /// `h-group`.
    ///
    /// The small input renders its text at `tokens.font(10)` (MoonUI
    /// `MoonInputMetrics::base_for_size(Size::Small)`), so a width that means "this many readable
    /// characters" is only true at the default Font setting unless it follows that ratio -- at the
    /// shipped +3 delta a raw 140px cell holds about 18 characters where the design reference
    /// promised 24. It shares [`MicroTriggerMetrics::scale`] because that IS the Font ratio, and
    /// deliberately NOT the floor beside it: `min_width` is the dropdown TRIGGER's own minimum and
    /// means nothing to an input.
    ///
    /// The three text columns share it for a second reason: their shrink order at a narrow window
    /// is decided by their RESOLVED bases, so a mixed policy would let one Font setting overtake
    /// another column. `repair_ui_font_delta` (`moon-core/src/config/schema.rs`) deliberately
    /// preserves any finite hand-edited delta, well past the slider's +6, so "no reachable setting
    /// reverses it" is only true when the comparison is scale-free.
    TextScaled,
}

/// One column of the core table, as both the header and the row read it.
#[derive(Clone, Copy, Debug)]
pub(super) struct ConnCol {
    /// Element id of this column's header cell.
    pub(super) id: &'static str,
    /// Locale key of the header label, or `None` for a column that carries no heading.
    pub(super) label: Option<&'static str>,
    /// Locale key of the header tooltip. Always present when `label` is.
    pub(super) tip: Option<&'static str>,
    /// Flex basis in rendered pixels. Authoritative: every cell also carries `min_w_0()`, so a
    /// wider child paints over its neighbour instead of pushing it.
    pub(super) basis: f32,
    /// Whether the column absorbs free space. The three columns carrying user-typed text do --
    /// `h-name`, `h-key` and `h-group` -- and only `h-name` does so without a [`ConnCol::max`].
    ///
    /// It is also what decides whether the column can SHRINK: `table.rs::cell` gives a
    /// non-growing column `flex_shrink_0`, so `grow: false` means "this width is rigid in BOTH
    /// directions". The default 860px Settings window does not fit this row, so every column that
    /// can afford to give way there has to be ABLE to -- see [`ConnCol::max`] for the arithmetic.
    pub(super) grow: bool,
    /// Upper bound on a GROWING column, in the same units as [`ConnCol::basis`] and resolved by
    /// the same [`ConnCol::width`] policy. `None` means "grow without limit"; only `h-name` has it.
    ///
    /// GROW-WITH-A-CAP rather than `grow: false` is the load-bearing choice here, and the 860px
    /// window is why. `settings/render.rs` leaves an 824px body inside its 18px padding there; the
    /// rigid columns -- two checkboxes, the bundle field, the colour picker, the three Micro
    /// dropdowns, the delete, reconnect and status glyphs -- plus the table inset, the scrollbar
    /// gutter and twelve gaps take about 661px of it at the SHIPPED Font delta of +3, leaving
    /// roughly 163px for three text columns whose bases resolve to about 487px. The default window
    /// is therefore a SHRINK regime, and anything pinned `flex_shrink_0` in it is width the text
    /// columns can never get back: a fixed 260px key would simply BE the widest column on a
    /// default install while the name column collapsed to nothing. Growable, all three give way in
    /// proportion to their RESOLVED bases instead -- which is why those bases are ordered, and why
    /// all three carry the same [`ConnColWidth::TextScaled`] policy so that the ordering cannot
    /// depend on the Font setting.
    ///
    /// The cap governs the other regime. Widen the window and Taffy freezes each capped item at
    /// its cap, then hands the remaining free space to the only uncapped one -- `h-name`.
    pub(super) max: Option<f32>,
    /// How [`ConnCol::basis`] becomes a rendered width.
    pub(super) width: ConnColWidth,
    /// Where the header label and the row control sit inside the cell.
    pub(super) align: ConnColAlign,
    /// Left padding of the header label, matching the input's internal text inset. Meaningful for
    /// [`ConnColAlign::Left`] only.
    pub(super) head_pad: f32,
}

/// Every column of the core table, left to right.
///
/// Indexed by [`ConnColId`]; `tests` proves the two stay in step.
const CONN_COLS: [ConnCol; 13] = [
    // 34, matching `h-win`: both hold a three-letter label, and at the supported +6 Font
    // setting the English/Spanish "Act" measures about 30.6px in Geist Mono -- at 28 the
    // HEADING itself ellipsised, which is the complaint this change exists to answer.
    ConnCol {
        id: "h-act",
        label: Some("conn.col.act"),
        tip: Some("conn.tip.act"),
        basis: 34.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-win",
        label: Some("conn.col.win"),
        tip: Some("conn.tip.win"),
        basis: 34.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    // The ONLY uncapped column, which is what makes it the widest of the three on any window with
    // free space to give: `h-key` and `h-group` freeze at their caps and Taffy hands what is left
    // to whatever is still growable. Its 150 is also the largest of the three bases, and because
    // all three share one width policy that ordering cannot be reversed by any Font setting -- so
    // it stays the widest of them on a window too narrow for the table as well, which the default
    // 860px one is.
    ConnCol {
        id: "h-name",
        label: Some("conn.col.name"),
        tip: Some("conn.tip.name"),
        basis: 150.0,
        grow: true,
        max: None,
        width: ConnColWidth::TextScaled,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    // Capped rather than uncapped, because the key field is MASKED: MoonUI draws one bullet per
    // character of a variable-length key, so there is no "width of the masked value" to size to,
    // and uncapped growth simply handed the column every spare pixel -- ~480px of a 1791px window
    // spent on identical dots while the name and group columns truncated real text beside them.
    //
    // 260 is what the field is actually asking for. At 200 the text viewport left over after the
    // input's mask-toggle and clear affixes, its own padding and the sibling Paste glyph
    // (`table.rs::paste_key_affix`) was still cramped, and that -- the VIEWPORT, not the key's
    // length -- is what the user reported; a longer key scrolls inside the field and always did.
    //
    // The 140 basis is deliberately far BELOW the cap and below `h-name`'s. Shrinkage is
    // proportional to the resolved basis, so on a window too narrow to hold the table -- and
    // 860px, the default, is exactly such a window -- the bases alone decide the order the three
    // text columns end up in, and 140 puts the key second. Pinning this column at a rigid 260
    // instead would have made the masked key the widest thing on screen there while the name
    // column collapsed.
    ConnCol {
        id: "h-key",
        label: Some("conn.col.key"),
        tip: Some("conn.tip.key"),
        basis: 140.0,
        grow: true,
        max: Some(260.0),
        width: ConnColWidth::TextScaled,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    // The three Micro dropdowns keep their design reference and are scaled with it; a raw basis
    // here is what let the trigger render ~1.3x wider than its own column at the shipped font
    // delta of +3 (`moon-core/src/config/schema.rs`).
    ConnCol {
        id: "h-proto",
        label: Some("conn.col.proto"),
        tip: Some("conn.tip.proto"),
        basis: 52.0,
        grow: false,
        max: None,
        width: ConnColWidth::MicroTrigger,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-preset",
        label: Some("conn.col.preset"),
        tip: Some("conn.tip.preset"),
        basis: 72.0,
        grow: false,
        max: None,
        width: ConnColWidth::MicroTrigger,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    // The lowest basis AND the lowest cap of the three, because a group name is one word. The 85
    // basis is what puts it last when the window is too narrow for the row; it is only ever
    // compared against the other two, which share its policy, so the ordering is a property of the
    // three literals and holds at every Font setting rather than only inside the slider's range.
    //
    // The cap: 140 is 126px of text viewport once the small input's 7px paddings are removed,
    // about 24 lowercase glyphs at the design-reference font size -- comfortable for a name like
    // "default" and for anything a user would actually type, and nothing like the ~420px this
    // column was taking of a 1791px window while the name column truncated beside it. Uncapped
    // growth here is what made the two growing columns split the free width evenly when only one
    // of them holds a name long enough to need it.
    ConnCol {
        id: "h-group",
        label: Some("conn.col.group"),
        tip: Some("conn.tip.group"),
        basis: 85.0,
        grow: true,
        max: Some(140.0),
        width: ConnColWidth::TextScaled,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    ConnCol {
        id: "h-bundle",
        label: Some("conn.col.bundle"),
        tip: Some("conn.tip.bundle"),
        basis: 96.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    ConnCol {
        id: "h-data",
        label: Some("conn.col.data"),
        tip: Some("conn.tip.flags"),
        basis: 52.0,
        grow: false,
        max: None,
        width: ConnColWidth::MicroTrigger,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    // 128 RAW, because that is what `MoonColorPicker` draws, unconditionally and unscaled. The old
    // 110 was 18px of guaranteed overflow.
    ConnCol {
        id: "h-color",
        label: Some("conn.col.color"),
        tip: Some("conn.tip.color"),
        basis: 128.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-del",
        label: None,
        tip: None,
        basis: 24.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-rec",
        label: None,
        tip: None,
        basis: 24.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-status",
        label: None,
        tip: None,
        basis: 16.0,
        grow: false,
        max: None,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
];

/// What a Micro dropdown trigger's rendered width depends on, measured once per render.
///
/// TWO numbers, because MoonUI resolves a scaled trigger against TWO independent sliders:
/// `scale` follows the Font delta, while the floor below it follows the UI-geometry scale. A
/// mirror carrying only the first was right at every shipped Font setting and wrong for a
/// hand-edited `ui_scale`, where the trigger's own padding alone can exceed the scaled basis --
/// and a trigger wider than its column is exactly the drift this module exists to prevent.
#[derive(Clone, Copy, Debug)]
pub(super) struct MicroTriggerMetrics {
    /// Multiplier from a design-reference trigger width to its rendered width.
    ///
    /// It is `tokens.font(10) / 10`, the Font-slider ratio itself, which is why
    /// [`ConnColWidth::TextScaled`] reads it too: a `MoonInput::small()` sizes its text at the
    /// same `tokens.font(10)`.
    pub(super) scale: f32,
    /// Floor MoonUI clamps a scaled trigger up to.
    pub(super) min_width: f32,
}

/// Name of one core-table column.
///
/// The discriminant IS the index into [`CONN_COLS`], so a row and a header naming the same variant
/// cannot reach different geometry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ConnColId {
    Act,
    Win,
    Name,
    Key,
    Proto,
    Preset,
    Group,
    Bundle,
    Data,
    Color,
    Delete,
    Reconnect,
    Status,
}

impl ConnColId {
    /// Every column, left to right, in the order both builders emit them.
    pub(super) const ALL: [ConnColId; 13] = [
        ConnColId::Act,
        ConnColId::Win,
        ConnColId::Name,
        ConnColId::Key,
        ConnColId::Proto,
        ConnColId::Preset,
        ConnColId::Group,
        ConnColId::Bundle,
        ConnColId::Data,
        ConnColId::Color,
        ConnColId::Delete,
        ConnColId::Reconnect,
        ConnColId::Status,
    ];

    /// Rendered width of this column, in pixels.
    ///
    /// Pure on purpose: the caller measures the scale once and passes it in, so this module needs
    /// no `App` and its tests need no window.
    ///
    /// Args:
    ///     micro: The Micro dropdown trigger's rendered-width inputs, from
    ///         `table::micro_trigger_metrics`.
    ///
    /// Returns:
    ///     The width both the header cell and the row cell are laid out at.
    pub(super) fn width(self, micro: MicroTriggerMetrics) -> f32 {
        let spec = self.spec();
        Self::resolve(spec.width, spec.basis, micro)
    }

    /// Upper bound this column may grow to, in pixels, or `None` for an uncapped one.
    ///
    /// Read by `table::SettingsView::cell` as `max_w`, which is what stops a growing column from
    /// taking free width it has no readable content to spend.
    ///
    /// Args:
    ///     micro: The Micro dropdown trigger's rendered-width inputs, from
    ///         `table::micro_trigger_metrics`.
    ///
    /// Returns:
    ///     The cap both the header cell and the row cell are laid out against, if there is one.
    pub(super) fn max_width(self, micro: MicroTriggerMetrics) -> Option<f32> {
        let spec = self.spec();
        // Same policy as the basis on purpose: a cap in different units from the width it bounds
        // is a cap that stops meaning what it says the moment a slider moves.
        spec.max.map(|max| Self::resolve(spec.width, max, micro))
    }

    /// Turn one design reference into rendered pixels under a column's width policy.
    ///
    /// Args:
    ///     policy: The column's [`ConnColWidth`].
    ///     reference: A basis or a cap, in that policy's units.
    ///     micro: The Micro dropdown trigger's rendered-width inputs.
    ///
    /// Returns:
    ///     The rendered pixel value.
    fn resolve(policy: ConnColWidth, reference: f32, micro: MicroTriggerMetrics) -> f32 {
        match policy {
            ConnColWidth::Raw => reference,
            // MIRRORS MoonUI's own `max(scaled(basis), minimum_width)`. Dropping the floor let
            // a trigger outgrow its column at a large `ui_scale`; `min_w_0` would then have it
            // paint across its neighbour instead of moving it -- misaligned either way.
            ConnColWidth::MicroTrigger => (reference * micro.scale).max(micro.min_width),
            // No floor: the floor beside `scale` is the dropdown trigger's own minimum, and an
            // input has nothing to do with it.
            ConnColWidth::TextScaled => reference * micro.scale,
        }
    }

    /// Look up this column's shared specification.
    ///
    /// Returns:
    ///     The one [`ConnCol`] both the header cell and the row cell are built from.
    pub(super) fn spec(self) -> &'static ConnCol {
        &CONN_COLS[self as usize]
    }
}

#[cfg(test)]
mod tests;
