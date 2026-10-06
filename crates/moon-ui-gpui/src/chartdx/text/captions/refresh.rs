//! Synchronize pane caption inputs without cloning unchanged collections.

use super::*;

#[cfg(test)]
mod tests;

impl RenderState {
    /// Re-resolve one pane's captions from the values it currently holds.
    ///
    /// Called from the SYNC paths — a market revision or an order revision — and, for the
    /// countdown captions alone, from the frame path when their quantized clock moves
    /// (`ChartDataState::tick_countdown_captions`). This is where strings are BUILT, which is why
    /// the frame path calls it once per quantum rather than per frame: `prepare_text` runs on
    /// every presented frame and must find the strings already made.
    ///
    /// Returns whether the drawn captions changed, so the caller can repaint only when they did.
    pub(in crate::chartdx) fn refresh_pane_labels(&mut self, idx: usize) -> bool {
        let cfg = self.chart_labels.clone();
        let Some(pr) = self.panes.get(idx) else {
            return false;
        };
        // Comparison delta: this pane's own last price against the anchor's, and only while the
        // pane is a book-only broom follower. Either half missing means there is nothing to
        // compare — which prints no caption rather than a zero.
        let compare_pct = self
            .compare_ref_price
            .filter(|_| pr.orderbook_only)
            .zip(pr.cached_last_price)
            .filter(|(r, l)| *r > 0.0 && *l > 0.0)
            .map(|(r, l)| (l - r) / r * 100.0);
        // The core-name caption is substituted HERE, at the one place its inputs are assembled,
        // so nothing below — resolution, truncation, measuring, plate geometry — learns why the
        // string changed. Only which string arrives changes.
        //
        // A pane drawing more than its own core's trades names the exchange and the count. That
        // wins over a shot: the shot exists to keep an account label out of a shared picture, and
        // this string has none. Otherwise a shot in flight names the exchange alone. The core name
        // is the user's own free text, an account label such as `ACCOUNT No 38`. `venue` is never
        // empty while a shot is armed: the order sync resolves it through the shared label helper,
        // which answers with the "not identified" wording for a core that cannot be named.
        let shot = self.shot_caption_active();
        let core_name = if let Some(n) = pr.all_cores_count {
            t!(
                "chart.history.all_cores.caption",
                venue = pr.venue.as_str(),
                n = n
            )
            .to_string()
        } else if shot {
            pr.venue.clone()
        } else {
            pr.core_name.clone()
        };
        let arb_view = self.arb_view.clone();
        let pr = &mut self.panes[idx];
        let changed = pr.labels.update_with(&cfg, &arb_view, |held| {
            // Exhaustive destructuring makes a new caption input require an explicit sync path.
            let super::super::LabelInputs {
                ticker,
                core_name: held_core_name,
                venue,
                quote,
                strategy,
                detect_strategy,
                detect_msg,
                filter_lines,
                trade,
                last_price,
                scale_badge,
                time_scale_s,
                compare_pct: held_compare_pct,
                delta_1h,
                delta_24h,
                context,
                figures,
                windows,
                volumes,
                liquidations,
                cursor_ms,
                arb,
                arb_reachable,
                now_ms,
                chart_tf_ms,
                basis,
                actions,
                column_scroll,
            } = held;
            let mut changed = false;
            macro_rules! sync {
                ($held:ident, $source:expr) => {
                    let source = &$source;
                    if $held != source {
                        $held.clone_from(source);
                        changed = true;
                    }
                };
            }
            sync!(ticker, pr.ticker);
            sync!(held_core_name, core_name);
            sync!(venue, pr.venue);
            sync!(quote, pr.quote);
            sync!(strategy, pr.label_strategy);
            sync!(detect_strategy, pr.label_detect_strategy);
            sync!(detect_msg, pr.label_detect_msg);
            sync!(filter_lines, pr.filter_lines);
            // A handed trade belongs to the window, rather than any individual pane.
            sync!(trade, self.trade_labels);
            sync!(last_price, pr.cached_last_price);
            sync!(scale_badge, pr.scale_badge);
            sync!(time_scale_s, pr.time_scale_s);
            sync!(held_compare_pct, compare_pct);
            sync!(delta_1h, pr.delta_1h);
            sync!(delta_24h, pr.delta_24h);
            sync!(context, pr.label_context);
            sync!(figures, pr.label_figures);
            sync!(windows, pr.label_windows);
            sync!(volumes, pr.label_volumes);
            sync!(liquidations, pr.label_liquidations);
            sync!(cursor_ms, pr.label_cursor_ms);
            sync!(arb, pr.label_arb);
            sync!(arb_reachable, pr.label_arb_reachable);
            sync!(now_ms, pr.label_now_ms);
            sync!(chart_tf_ms, self.chart_tf_ms);
            sync!(basis, pr.label_basis);
            // Action availability is supplied by the panel, never inferred by the renderer.
            sync!(actions, pr.label_actions);
            sync!(column_scroll, pr.label_scroll);
            changed
        });
        // Recorded whether or not the texts changed: `update_with` is a cache and answers "nothing
        // moved" when the substitution happens to produce the same string, but the shot's proof
        // asks what the CURRENT labels were built from, not whether they differ from last time.
        self.panes[idx].labels_shot_substituted = shot;
        if changed {
            crate::diag::bump(&crate::diag::CHART_CAPTION_REBUILD);
        }
        changed
    }
}
