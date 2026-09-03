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
    /// Whether the column absorbs free space. Only columns holding user-typed text that
    /// TRUNCATES do -- `h-name` and `h-group`. `h-key` deliberately does not: its content is
    /// masked, so extra width buys more dots and nothing readable.
    pub(super) grow: bool,
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
        width: ConnColWidth::Raw,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    ConnCol {
        id: "h-name",
        label: Some("conn.col.name"),
        tip: Some("conn.tip.name"),
        basis: 150.0,
        grow: true,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    // FIXED, not growing, at the basis it always had. The key field is MASKED and MoonUI draws
    // one bullet per character of a variable-length key, so there is no "width of the masked
    // value" to size to -- growth simply handed the column every spare pixel, ~480px of a 1791px
    // window spent on identical dots while `Имя` and `Группа` truncated real text beside them.
    // 200 keeps a usable text viewport next to the input's mask-toggle and clear affixes and the
    // sibling Paste glyph (`table.rs::paste_key_affix`); a longer key scrolls inside the field,
    // which is what it did before and what a masked field can afford.
    ConnCol {
        id: "h-key",
        label: Some("conn.col.key"),
        tip: Some("conn.tip.key"),
        basis: 200.0,
        grow: false,
        width: ConnColWidth::Raw,
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
        width: ConnColWidth::MicroTrigger,
        align: ConnColAlign::Center,
        head_pad: 0.0,
    },
    // Grows with `h-name`: both hold user-typed text that truncates, and the width the masked
    // key gave up is exactly what they were missing.
    ConnCol {
        id: "h-group",
        label: Some("conn.col.group"),
        tip: Some("conn.tip.group"),
        basis: 110.0,
        grow: true,
        width: ConnColWidth::Raw,
        align: ConnColAlign::Left,
        head_pad: 8.0,
    },
    ConnCol {
        id: "h-bundle",
        label: Some("conn.col.bundle"),
        tip: Some("conn.tip.bundle"),
        basis: 96.0,
        grow: false,
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
        match spec.width {
            ConnColWidth::Raw => spec.basis,
            // MIRRORS MoonUI's own `max(scaled(basis), minimum_width)`. Dropping the floor let
            // a trigger outgrow its column at a large `ui_scale`; `min_w_0` would then have it
            // paint across its neighbour instead of moving it -- misaligned either way.
            ConnColWidth::MicroTrigger => (spec.basis * micro.scale).max(micro.min_width),
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
