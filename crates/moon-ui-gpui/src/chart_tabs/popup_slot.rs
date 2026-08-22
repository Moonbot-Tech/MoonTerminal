//! The one overlay a chart host is showing.
//!
//! A chart host — the tab strip or a detached window — carries six overlays that all hang off the
//! same toolbar row: the ⚙ layout popup, candles, graphics, labels, the drawing-tool defaults panel
//! and the market-search list. They used to be six independent `bool`s, and nothing but a side
//! effect kept them apart: `MoonPopover` dismisses itself on an outside click in the CAPTURE phase,
//! which happens to fire before the neighbouring button's own press. Two of the six never took that
//! path — labels turns outside-click dismissal off because its dropdown menus paint in their own
//! deferred layers, and the tool panel and coin list are plain elements whose dismiss layer sits
//! UNDER the button row, so a press on a settings button never reaches it — and those two stayed on
//! screen under whatever opened next.
//!
//! One slot holding one value removes the question: opening anything displaces whatever was there,
//! and no combination of handlers can leave two up. What a displaced popup owes on the way out (⚙
//! commits its size fields) belongs to the host trait, not here — this type is pure state.

/// One of a chart host's mutually exclusive overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChartPopup {
    /// ⚙ layout settings for the target tab.
    Layout,
    /// Candles and trades display settings.
    Candle,
    /// The chart-graphics palette.
    Graphics,
    /// The chart-labels module list.
    Labels,
    /// Defaults for the armed drawing tool. Tab strip only — a detached window draws no tool row.
    FigStyle,
    /// The market-search match list.
    Coin,
}

/// The single overlay slot: at most one [`ChartPopup`], so two cannot be open at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PopupSlot(Option<ChartPopup>);

impl PopupSlot {
    /// Whether `popup` is the one showing.
    ///
    /// Args:
    ///     popup: The overlay to test.
    ///
    /// Returns:
    ///     `true` when the slot holds exactly that overlay.
    pub(crate) fn shows(self, popup: ChartPopup) -> bool {
        self.0 == Some(popup)
    }

    /// Put `popup` in the slot, displacing whatever was there.
    ///
    /// Args:
    ///     popup: The overlay to show.
    ///
    /// Returns:
    ///     The overlay it displaced, or `None` when the slot was empty or already held `popup`.
    ///     The caller settles that one's outstanding business (see `LayoutPopupHost`).
    pub(crate) fn show(&mut self, popup: ChartPopup) -> Option<ChartPopup> {
        self.0.replace(popup).filter(|prev| *prev != popup)
    }

    /// Empty the slot, but only if `popup` is what it holds.
    ///
    /// The ownership check is load-bearing rather than defensive. `MoonPopover` reports a close
    /// TWICE for one press on an open popup's own button — once from the outside-click handler and
    /// once as the trigger re-arms — and by the second report the slot may already hold the popup
    /// that press opened. An unconditional clear would shut that one too.
    ///
    /// Args:
    ///     popup: The overlay asking to be hidden.
    ///
    /// Returns:
    ///     Whether it was the one showing, and so was hidden.
    pub(crate) fn hide(&mut self, popup: ChartPopup) -> bool {
        if self.0 != Some(popup) {
            return false;
        }
        self.0 = None;
        true
    }
}

#[cfg(test)]
mod tests;
