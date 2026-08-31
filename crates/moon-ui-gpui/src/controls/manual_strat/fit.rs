//! Priority-ordered width clip for the header's manual-strategy quick-select button row.
//!
//! The header has no shared row-fit budget the way the toolbar does
//! (`controls::toolbar::row_fit`): this cluster is one section among several the header composes,
//! and reworking every other section's own layout to expose a live remainder is out of scope here.
//! So the caller resolves purely from `chrome_width` against this cluster's own measured content
//! plus a conservative reservation for the rest of the header — the same pattern
//! `design::ticker_visible` already uses for the header's other narrow-window collapse. Pure and
//! arg-taking, mirroring `toolbar::row_fit`'s shape, so it is unit-testable without an `App`.

/// How a button slot's label renders at the current clip level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LabelMode {
    /// The slot's caption: the trader's own, or the strategy's name when they set none.
    NameOnly,
    /// The slot's 1-based ordinal (`"1"`..`"10"`), for a row too narrow to hold captions.
    NumberOnly,
}

/// Pre-measured widths for one visible slot, at the current theme and font.
#[derive(Clone, Copy, Debug)]
pub(super) struct SlotWidths {
    /// Button width at its caption.
    pub name_only: f32,
    /// Button width with only the numeric fallback segment.
    pub number_only: f32,
}

/// Resolved visibility for the button row and the neighbouring picker pill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct StratFit {
    /// Label mode shared by every rendered button.
    pub label_mode: LabelMode,
    /// How many of the caller's slots (in show_button order) to render. `0` hides the row
    /// entirely while the toggle and picker pill remain — the last two standing.
    pub visible_count: usize,
    /// Whether the picker pill must render at the caller's reduced label cap instead of its
    /// normal one. Set once the row has already dropped to zero buttons under the normal cap:
    /// the reduced cap is then strictly the better choice for the pill regardless of whether it
    /// alone closes the remaining gap, since there is nothing left in the row to shed either way.
    pub pill_reduced: bool,
}

/// Resolve the clip level for the button row and the picker pill.
///
/// Priority, most-clipped first:
/// 1. every slot's caption, replaced by its 1-based ordinal;
/// 2. slots beyond what fits, dropped from the end of the (already visibility-filtered) list, down
///    to zero;
/// 3. the picker pill label, capped further — the toggle and the pill are the last two standing.
///
/// Args:
///     available: `chrome_width` this cluster may spend.
///     gap: Horizontal gap between two adjacent buttons.
///     slots: Pre-measured widths for each visible slot, in render order.
///     selected: Position of the slot whose strategy is currently active, if it is on screen. It
///         keeps its NAME at the number-only level (see `manual_strat`'s renderer), so it is
///         measured at `name_only` there — budgeting it as a digit would prove a row fits at a
///         width it then overflows, clipping the header's trailing readouts.
///     base: Width already spent by everything else in the cluster and a conservative
///         reservation for the rest of the header, with the picker pill at its normal label cap.
///
/// Returns:
///     The label mode, the number of leading slots to render, and whether the pill cap is reduced.
pub(super) fn resolve_strat_fit(
    available: f32,
    gap: f32,
    slots: &[SlotWidths],
    selected: Option<usize>,
    base: f32,
) -> StratFit {
    fn row_width(
        slots: &[SlotWidths],
        count: usize,
        gap: f32,
        select: impl Fn(usize, &SlotWidths) -> f32,
    ) -> f32 {
        if count == 0 {
            return 0.0;
        }
        slots[..count]
            .iter()
            .enumerate()
            .map(|(i, s)| select(i, s))
            .sum::<f32>()
            + gap * (count - 1) as f32
    }
    let n = slots.len();
    let fits = |row: f32| base + row <= available;
    // At the number-only level the selected slot still renders its caption.
    let number_or_name = move |i: usize, s: &SlotWidths| {
        if selected == Some(i) {
            s.name_only
        } else {
            s.number_only
        }
    };

    if fits(row_width(slots, n, gap, |_, s| s.name_only)) {
        return StratFit {
            label_mode: LabelMode::NameOnly,
            visible_count: n,
            pill_reduced: false,
        };
    }
    for count in (0..=n).rev() {
        if fits(row_width(slots, count, gap, number_or_name)) {
            return StratFit {
                label_mode: LabelMode::NumberOnly,
                visible_count: count,
                pill_reduced: false,
            };
        }
    }
    // Not even zero buttons fit under the normal pill cap. Nothing is left in the row to shed, so
    // this is the floor: the row stays hidden and the pill switches to its reduced cap, whether or
    // not that alone closes the remaining gap.
    StratFit {
        label_mode: LabelMode::NumberOnly,
        visible_count: 0,
        pill_reduced: true,
    }
}
