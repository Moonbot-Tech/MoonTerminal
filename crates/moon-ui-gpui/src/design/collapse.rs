//! Measured-width collapse of the header and the trading toolbar.
//!
//! Both bands shed whole element groups in a fixed order of sacrifice as the window narrows, so
//! nothing is ever clipped mid-figure. The level is chosen from MEASURED widths (the same
//! arithmetic the row renders with), never from tuned window thresholds: a longer locale or a
//! larger font collapses sooner on its own. Everything here is plain arithmetic with no GPUI
//! dependency, so the boundaries are unit-tested exactly.

/// Return the lowest level whose total width fits `available`.
///
/// Levels are ordered from widest (0) to narrowest (`max`); `width_at` must not grow with the
/// level. The comparison is inclusive, so a level is chosen at the exact pixel where its complete
/// width first fits. When even `max` does not fit, `max` is returned: the narrowest row is drawn
/// and whatever is left over is the caller's to fit (a name, never a figure).
///
/// Args:
///     available: Width the band may occupy.
///     width_at: Total width of the band at a level.
///     max: The narrowest level.
///
/// Returns:
///     The chosen level in `0..=max`.
pub fn lowest_fitting_level(available: f32, width_at: impl Fn(usize) -> f32, max: usize) -> usize {
    (0..max)
        .find(|&level| width_at(level) <= available)
        .unwrap_or(max)
}

/// Measured widths of every header element group, in rendered pixels.
///
/// Each optional field is the width the group adds when it is drawn, gaps included. `fixed` and
/// `right_fixed` are drawn at every level: `fixed` left of the ticker, `right_fixed` right of it
/// (the sleep button, clock digits, launcher icons, window controls), so the ticker popup's right
/// offset is computable from [`HeaderWidths::right_of_ticker`] alone. `separators` are the rules
/// left of the ticker, `right_separators` the ones right of it; both go at the same level.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeaderWidths {
    pub fixed: f32,
    pub right_fixed: f32,
    pub delta_1h: f32,
    pub free_sub: f32,
    pub sleep_label: f32,
    pub ticker: f32,
    pub logo: f32,
    pub launcher_labels: f32,
    pub balance_caption: f32,
    pub clock_city: f32,
    pub separators: f32,
    pub right_separators: f32,
    pub usdt_unit: f32,
    pub inactive_mode_seg: f32,
    pub core_name_full: f32,
    pub core_name_cap_l5: f32,
    pub core_name_cap_l6: f32,
    pub core_gear: f32,
}

/// One header collapse level, `0` (everything drawn) to [`HeaderLevel::MAX`].
///
/// Levels are cumulative, each hiding the groups below in addition to everything earlier:
/// L1 the 1h delta, the "free" sub-figure and the sleep label; L2 the ticker and the logo;
/// L3 the launcher labels and the balance caption; L4 the clock city, the separators, the USDT
/// unit and the inactive AUTO/MANUAL segment; L5 the core name capped; L6 the core name capped
/// tighter and the core gear hidden. Every accessor answers "is this group drawn".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeaderLevel(pub u8);

impl HeaderLevel {
    /// The narrowest header level.
    pub const MAX: u8 = 6;

    pub fn delta_1h(self) -> bool {
        self.0 < 1
    }

    pub fn free_sub(self) -> bool {
        self.0 < 1
    }

    pub fn sleep_label(self) -> bool {
        self.0 < 1
    }

    pub fn ticker(self) -> bool {
        self.0 < 2
    }

    pub fn logo(self) -> bool {
        self.0 < 2
    }

    pub fn launcher_labels(self) -> bool {
        self.0 < 3
    }

    pub fn balance_caption(self) -> bool {
        self.0 < 3
    }

    pub fn clock_city(self) -> bool {
        self.0 < 4
    }

    pub fn separators(self) -> bool {
        self.0 < 4
    }

    pub fn usdt_unit(self) -> bool {
        self.0 < 4
    }

    pub fn inactive_mode_seg(self) -> bool {
        self.0 < 4
    }

    pub fn core_gear(self) -> bool {
        self.0 < 6
    }

    /// Return the width ceiling of the core name at this level.
    ///
    /// The full measured name below L5, so the name is never touched while anything else can
    /// still yield; the L5 and L6 caps after that, never wider than the name itself, and the L6
    /// cap never wider than the L5 one, so a deeper level can never widen the row.
    pub fn core_name_cap(self, widths: &HeaderWidths) -> f32 {
        let l5 = widths.core_name_cap_l5.min(widths.core_name_full);
        match self.0 {
            0..=4 => widths.core_name_full,
            5 => l5,
            _ => widths.core_name_cap_l6.min(l5),
        }
    }
}

/// Return `width` when `shown`, else nothing.
fn when(shown: bool, width: f32) -> f32 {
    if shown { width } else { 0.0 }
}

impl HeaderWidths {
    /// Return the total header width at `level`.
    pub fn width_at(&self, level: HeaderLevel) -> f32 {
        self.fixed
            + when(level.free_sub(), self.free_sub)
            + when(level.ticker(), self.ticker)
            + when(level.delta_1h(), self.delta_1h)
            + when(level.logo(), self.logo)
            + when(level.balance_caption(), self.balance_caption)
            + when(level.separators(), self.separators)
            + when(level.usdt_unit(), self.usdt_unit)
            + when(level.inactive_mode_seg(), self.inactive_mode_seg)
            + level.core_name_cap(self)
            + when(level.core_gear(), self.core_gear)
            + self.right_of_ticker(level)
    }

    /// Return the width of everything drawn right of the ticker at `level`.
    ///
    /// The ticker popup anchors to the ticker's right edge by this sum, so every element drawn
    /// right of the ticker belongs here and nowhere else.
    pub fn right_of_ticker(&self, level: HeaderLevel) -> f32 {
        self.right_fixed
            + when(level.sleep_label(), self.sleep_label)
            + when(level.clock_city(), self.clock_city)
            + when(level.separators(), self.right_separators)
            + when(level.launcher_labels(), self.launcher_labels)
    }

    /// Return the lowest header level that fits `available`.
    pub fn level(&self, available: f32) -> HeaderLevel {
        let max = usize::from(HeaderLevel::MAX);
        let level = lowest_fitting_level(available, |l| self.width_at(header_level(l)), max);
        header_level(level)
    }
}

/// Convert a level index known to be in `0..=HeaderLevel::MAX`.
fn header_level(level: usize) -> HeaderLevel {
    HeaderLevel(u8::try_from(level).unwrap_or(HeaderLevel::MAX))
}

/// Measured widths of every trading-toolbar element group, in rendered pixels.
///
/// `sell_strip` / `sell_drop` and `size_strip` / `size_drop` are the two forms of one control,
/// exactly one drawn at a time; a collapsed form wider than its strip counts as the strip, so a
/// deeper level never widens the row. `chart_tools` is the Profit Monitor and Screener icons;
/// `overflow` is the "More" button that takes them in once they fold. `strat_controls` is the
/// Strategy pod at its inner ladder's FULL state: counted at every level, because that pod sheds
/// only through its own ladder once the outer level is already the narrowest. `tp_pod`,
/// `sell_pod`, `size_pod` and `sl_pod` are those pods' chrome (and the Take value, the Stop switch
/// and value), each with its gap, which leave the row when the pod folds into "More"; `live` is
/// the Live chip with its gap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ToolbarWidths {
    pub fixed: f32,
    pub strat_controls: f32,
    pub sell_strip: f32,
    pub sell_drop: f32,
    pub max_readout: f32,
    pub ms_summary: f32,
    pub size_strip: f32,
    pub size_drop: f32,
    pub captions: f32,
    pub chart_tools: f32,
    pub overflow: f32,
    pub own_icon: f32,
    pub tp_pod: f32,
    pub sell_pod: f32,
    pub size_pod: f32,
    pub sl_pod: f32,
    pub live: f32,
}

/// One toolbar collapse level, `0` (everything drawn) to [`ToolbarLevel::MAX`].
///
/// Cumulative: L1 the sell strip becomes a dropdown of the selected S; L2 the MAX readout goes;
/// L3 the manual-strategy summary read-outs go; L4 the size strip becomes a dropdown of the
/// selected B; L5 the pod captions go; L6 the chart tools fold into "More"; L7 the own-set icon
/// folds into "More"; L8 the Take pod and the collapsed sell cell fold into "More" as rows; L9 the
/// collapsed size cell does too; L10 the Stop pod does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolbarLevel(pub u8);

impl ToolbarLevel {
    /// The narrowest toolbar level.
    pub const MAX: u8 = 10;
    /// The level the Take pod and the sell control fold into "More" at.
    pub const TAKE_SELL_FOLD: u8 = 8;
    /// The level the size control folds into "More" at.
    pub const SIZE_FOLD: u8 = 9;
    /// The level the Stop pod folds into "More" at.
    pub const SL_FOLD: u8 = 10;

    pub fn sell_as_dropdown(self) -> bool {
        self.0 >= 1
    }

    pub fn max_readout(self) -> bool {
        self.0 < 2
    }

    pub fn ms_summary(self) -> bool {
        self.0 < 3
    }

    pub fn size_as_dropdown(self) -> bool {
        self.0 >= 4
    }

    pub fn captions(self) -> bool {
        self.0 < 5
    }

    pub fn chart_tools_folded(self) -> bool {
        self.0 >= 6
    }

    pub fn own_icon_folded(self) -> bool {
        self.0 >= 7
    }

    pub fn take_sell_folded(self) -> bool {
        self.0 >= Self::TAKE_SELL_FOLD
    }

    pub fn size_folded(self) -> bool {
        self.0 >= Self::SIZE_FOLD
    }

    pub fn sl_folded(self) -> bool {
        self.0 >= Self::SL_FOLD
    }
}

impl ToolbarWidths {
    /// Return the total toolbar width at `level`.
    pub fn width_at(&self, level: ToolbarLevel) -> f32 {
        let sell = if level.take_sell_folded() {
            0.0
        } else if level.sell_as_dropdown() {
            self.sell_pod + self.sell_drop.min(self.sell_strip)
        } else {
            self.sell_pod + self.sell_strip
        };
        let size = if level.size_folded() {
            0.0
        } else if level.size_as_dropdown() {
            self.size_pod + self.size_drop.min(self.size_strip)
        } else {
            self.size_pod + self.size_strip
        };
        self.fixed
            + self.strat_controls
            + sell
            + size
            + self.trailing_at(level)
            + when(!level.take_sell_folded(), self.tp_pod)
            + when(!level.sl_folded(), self.sl_pod)
            + when(level.max_readout(), self.max_readout)
            + when(level.ms_summary(), self.ms_summary)
            + when(level.captions(), self.captions)
            + when(!level.own_icon_folded(), self.own_icon)
    }

    /// Return the width of the trailing cluster at `level`: the chart tools (or "More" once they
    /// fold) and the Live chip, gaps included.
    pub fn trailing_at(&self, level: ToolbarLevel) -> f32 {
        let tools = if level.chart_tools_folded() {
            self.overflow.min(self.chart_tools)
        } else {
            self.chart_tools
        };
        tools + self.live
    }

    /// Return the lowest toolbar level that fits `available`.
    pub fn level(&self, available: f32) -> ToolbarLevel {
        let max = usize::from(ToolbarLevel::MAX);
        let level = lowest_fitting_level(available, |l| self.width_at(toolbar_level(l)), max);
        toolbar_level(level)
    }

    /// Return the lowest level that fits `available`, held inside `floor..=ceil`.
    ///
    /// While a control's menu or popup is open the level must keep drawing its trigger: past the
    /// level that introduces a collapsed form, and short of the level that folds the control into
    /// "More". The held level is drawn even where it does not fit.
    pub fn level_within(
        &self,
        available: f32,
        floor: ToolbarLevel,
        ceil: ToolbarLevel,
    ) -> ToolbarLevel {
        let ceil = ceil.min(ToolbarLevel(ToolbarLevel::MAX));
        self.level(available).max(floor.min(ceil)).min(ceil)
    }
}

/// Convert a level index known to be in `0..=ToolbarLevel::MAX`.
fn toolbar_level(level: usize) -> ToolbarLevel {
    ToolbarLevel(u8::try_from(level).unwrap_or(ToolbarLevel::MAX))
}

#[cfg(test)]
mod tests;
