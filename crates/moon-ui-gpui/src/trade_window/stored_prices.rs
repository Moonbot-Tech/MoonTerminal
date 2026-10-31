//! The price file's recorded Last and Mark lines for the trade on screen.
//!
//! The chart's price lines come from the provider's rings, which start empty on every run — so a
//! trade closed before this run drew neither line. The price recorder files the market list's
//! prices around every trade, and this reads them back for the window's market and span, in the
//! background, and hands them to the chart, which lays them over the rings where they speak. The
//! station's price pull may file more of them after the window read the file; it wakes the window,
//! which reads the same stretch again.

use gpui::*;
use moon_core::market::price_recorder::load_lines;
use moon_core::market::trade_replay::ReplayWindow;

use super::TradeWindowView;

impl TradeWindowView {
    /// Read the recorded lines of this window's market over `window` and hand them to the chart.
    ///
    /// Called by `fetch` for every new request, so a window that moves to another trade or period
    /// first drops the previous trade's lines; the answer is dropped too when a later request has
    /// superseded it. A failed read draws what the rings hold, as before the file existed.
    ///
    /// Args:
    ///     exchange_key: The recorder's key for the trade's exchange, from the replay address.
    ///     window: The span the replay covers.
    ///     sequence: The fetch the answer belongs to.
    ///     cx: View context.
    pub(super) fn request_stored_prices(
        &mut self,
        exchange_key: String,
        window: &ReplayWindow,
        sequence: u64,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .update(cx, |panel, pcx| panel.attach_stored_prices(None, pcx));
        self.stored_prices_at = Some((exchange_key, window.from_ms, window.to_ms));
        self.load_stored_prices(sequence, cx);
    }

    /// Read this window's stretch of the price file again, after the station's pull filed more.
    ///
    /// The lines on screen stay until the new answer replaces them, so the re-read never blanks
    /// them; an answer that comes back empty leaves them too. Nothing to do before a fetch named
    /// the stretch.
    ///
    /// Args:
    ///     cx: View context.
    pub(super) fn reload_stored_prices(&mut self, cx: &mut Context<Self>) {
        if self.stored_prices_at.is_some() {
            self.load_stored_prices(self.sequence, cx);
        }
    }

    /// Read [`Self::stored_prices_at`] in the background and hand a non-empty answer to the chart,
    /// unless a later fetch has superseded `sequence` meanwhile.
    fn load_stored_prices(&mut self, sequence: u64, cx: &mut Context<Self>) {
        let Some((exchange_key, from_ms, to_ms)) = self.stored_prices_at.clone() else {
            return;
        };
        let market = self.market.clone();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let read = executor
                .spawn(async move { load_lines(&exchange_key, &market, from_ms, to_ms) })
                .await;
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    if sequence != this.sequence {
                        return;
                    }
                    let lines = match read {
                        Ok(lines) if !lines.is_empty() => lines,
                        Ok(_) => return,
                        Err(e) => {
                            log::warn!(
                                "[trade] recorded prices for {} unreadable: {e}",
                                this.market
                            );
                            return;
                        }
                    };
                    this.panel.update(cx, |panel, pcx| {
                        panel.attach_stored_prices(Some(std::rc::Rc::new(lines)), pcx)
                    });
                })
            });
        })
        .detach();
    }
}
