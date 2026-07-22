//! The shared scaffold of a background DB read: run a closure on the background
//! executor, then hand the result back to the view on the UI thread.
//!
//! Every Analytics recompute follows the same envelope — count the busy overlay,
//! spawn the read, and in the completion FIRST balance the overlay, THEN let the
//! caller guard its generation and store the result. The envelope is easy to get
//! subtly wrong by hand (decrementing after the stale-guard leaks the overlay on
//! every discarded reply), so every RECOMPUTE path goes through here; what varies
//! per call — which generation gates the store and where the result lands — stays
//! with the caller inside `store`. The two filter suggestion sweeps and the write
//! paths keep their envelopes inline on purpose: their completions do more than
//! store-under-one-guard — the sweeps route a failure into the KPI `LoadState`
//! under a SECOND generation (`tuner.seq`), and the writes send to the cores —
//! so folding them in would put per-site logic inside the shared scaffold.

use gpui::{AppContext as _, Context};

use super::AnalyticsView;

impl AnalyticsView {
    /// Run `db` on the background executor and `store` its result on the view.
    ///
    /// `overlay` — whether the read counts toward the blocking busy overlay
    /// (`op_started`/`op_finished`); a read that must not dim the window (a selection
    /// cue, a debounced rescan) passes `false`.
    ///
    /// `store` runs AFTER the overlay is balanced, on every completion — stale ones
    /// included: it must check its own generation before publishing the result.
    pub(super) fn spawn_db<R: Send + 'static>(
        &mut self,
        overlay: bool,
        cx: &mut Context<Self>,
        db: impl FnOnce() -> R + Send + 'static,
        store: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        if overlay {
            self.op_started();
        }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { db() }).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if overlay {
                        this.op_finished(cx);
                    }
                    store(this, result, cx);
                });
            });
        })
        .detach();
    }
}
