//! Compose the group-window frame (`Render for Shell`) and the content of the open trading-metric
//! popup.

use std::time::Instant;

use gpui::*;

use moon_ui::{MoonPalette, MoonWindowFrame, v_flex};

use super::Shell;
use crate::chrome::terminal_chrome;
use crate::window::input_hook::window_mouse_hook;
use crate::{controls, design};

impl Shell {
    /// Resolve one leverage address into the exchange limits for it and the quote token they are
    /// denominated in.
    ///
    /// THE reason this is a function and not two similar blocks: the toolbar row and the open
    /// popover must never state different caps for one coin, and they ask about slightly different
    /// addresses — the row asks about the target the group resolves to NOW, the popover about the
    /// target it was SEEDED from. Sharing the resolution makes that the ONLY difference between
    /// them; two hand-copied blocks would leave the guarantee resting on them staying in sync.
    ///
    /// Args:
    ///     target: The leverage address to describe, or `None` when there is none.
    ///     cx: Context used to read the backend.
    ///
    /// Returns:
    ///     A limits-and-quote tuple; its limits element is `None` and its quote token is empty when the
    ///     address is absent or incomplete.
    pub(super) fn limits_for(
        &self,
        target: Option<&controls::MetricTarget>,
        cx: &App,
    ) -> (Option<moon_core::market::MarketLimits>, String) {
        match target.map(|t| (t.core, t.market.as_deref())) {
            Some((Some(core), Some(market))) => (
                self.backend.read(cx).market_limits(core, market),
                moon_core::symbol::resolve_quote(market),
            ),
            _ => (None, String::new()),
        }
    }

    /// Build the CONTENT of the open trading-metric popup (TP/SL/leverage), or `None` when closed.
    ///
    /// Content only. The box around it — frame, background, width, and above all its POSITION —
    /// belongs to the anchored `MoonPopover` that `controls::toolbar` wraps around the matching
    /// metric button. That is the whole point of the arrangement: the popup follows its trigger by
    /// construction, so no layout term in the toolbar can desync it.
    fn open_metric_content(&self, p: MoonPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self.open_metric_popup.clone()?;
        use controls::TradeMetric;
        let metric = open.metric;
        let extended = self.active_tp_extended(cx);
        let (slider, input) = match metric {
            TradeMetric::Tp => (
                if extended {
                    &self.tp_slider_ext
                } else {
                    &self.tp_slider_normal
                },
                &self.tp_input,
            ),
            TradeMetric::Sl => (&self.sl_slider, &self.sl_input),
            TradeMetric::Lev => (&self.lev_slider, &self.lev_input),
        };
        let hedge_on = {
            let b = self.backend.read(cx);
            b.active_trade_core(&self.group)
                .and_then(|c| b.session.store().core(c))
                .and_then(|d| d.hedge_mode)
                .unwrap_or(false)
        };
        // Read against the popup's SEEDED address, not the group's current one: the Main chart can
        // move to another coin while this popup stands open, and answering with the new coin's cap
        // would print one market's limit over another market's editor.
        let (limits, quote) = self.limits_for(Some(&open.target), cx);
        Some(controls::metric_popup_content(
            &open,
            limits,
            &quote,
            slider,
            &self.tp_fine_slider,
            input,
            extended,
            hedge_on,
            &self.backend,
            &self.group,
            p,
            cx,
        ))
    }
}

/// Replace the two chrome rows with empty boxes of the same height, for one measurement.
///
/// `MOON_CHROME_STUB=1`. Not a feature and not a setting — an A/B knob, and the cheapest honest
/// answer to "what share of a frame do the header and the toolbar actually cost". The element
/// trees they build are counted (`shell_header_us`, `shell_toolbar_us`), but LAYOUT, text shaping
/// and paint happen below `render`, where nothing the terminal can install will see them — and
/// those are 83% of a frame. Two runs of the same drag, and the difference in `frame_draw_us` per
/// draw is their real share.
///
/// The boxes keep each row's HEIGHT so the dock below is laid out over the same area. Dropping
/// the rows outright would hand the dock more height, more visible table rows and a more
/// expensive frame — an A/B measuring its own distortion.
///
/// Read once: a `var_os` on the render path would join the cost it is meant to measure.
fn chrome_stubbed() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("MOON_CHROME_STUB").is_ok_and(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        })
    })
}

impl Render for Shell {
    /// Render the group window with its two chrome rows, dock, status bar, and anchored popovers.
    ///
    /// Args:
    ///     window: Current OS window used for viewport state and input hooks.
    ///     cx: Shell context used to read state, build elements, and wire callbacks.
    ///
    /// Returns:
    ///     The complete window element tree for the current frame.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::diag::bump(&crate::diag::SHELL_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::SHELL_RENDER_US);
        let prelude_us = crate::diag::timer();
        crate::hotkeys::restore_root_focus(&self.focus, window, cx);

        // Collect frame and status diagnostics here; chart data, input, and axes stay in ChartPanel.
        // Smoothed render FPS is shown in the status bar, matching the egui host.
        let now_inst = Instant::now();
        if let Some(prev) = self.last_frame {
            let dt = now_inst.duration_since(prev).as_secs_f32().max(1e-4);
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
            // Bucket the gap since the previous repaint. The smoothed FPS above cannot
            // answer "is it jerky": it is an average, and an average hides exactly the
            // burst-and-gap pattern a stutter is made of. See `diag::SHELL_FRAME_SLOW`.
            let gap_ms = dt * 1000.0;
            crate::diag::note_frame_gap_us((dt * 1_000_000.0) as u64);
            if gap_ms > 50.0 {
                crate::diag::bump(&crate::diag::SHELL_FRAME_STALL);
            } else if gap_ms > 20.0 {
                crate::diag::bump(&crate::diag::SHELL_FRAME_SLOW);
            }
        }
        self.last_frame = Some(now_inst);
        let fps = self.fps;

        let (conn, license, snap, book_levels) = {
            let b = self.backend.read(cx);
            let mut conn = b.session.conn_summary_group(&self.group);
            // The disconnected-cores tooltip is a core list like any other — rank it the same
            // way, or it reads in a different order than the header pill right above it.
            crate::core_order::CoreOrder::new(&b.config).sort_by(&mut conn.down, |row| row.id);
            let license = b.session.license_summary_group(&self.group);
            let snap = b.snap;
            // The status bar needs only the order-book level count for the current Main chart.
            let book_levels = match b.main_chart_target(&self.group) {
                Some((core, m)) => b.session.with_orderbook_view(core, &m, |data| {
                    data.map(|(book, _)| book.len()).unwrap_or(0)
                }),
                None => 0,
            };
            (conn, license, snap, book_levels)
        };
        let chrome_width = f32::from(window.viewport_size().width);
        let p = MoonPalette::active(cx);

        // Reconcile an orphaned metric popup before building content for the remaining open one.
        // The toolbar's `MoonPopover` owns the box and trigger-relative position. Keeping the metric
        // and content paired prevents the row from attaching one metric's editor to another button.
        self.reconcile_metric_popup(cx);
        let metric_popup = self
            .open_metric_popup
            .as_ref()
            .map(|open| open.metric)
            .zip(self.open_metric_content(p, cx));

        // Exchange limits for the coin the row is trading NOW. Read once, here, through the same
        // accessor the popup uses, so the row and the popover can never state different caps for
        // one coin — an open popup still asks about the address it was SEEDED from, which is the
        // one difference between them and a deliberate one.
        //
        // Resolved through `TradeMetric::Lev.target`, NOT through `main_chart_target` directly.
        // That method is the one place that decides which (core, market) a leverage edit is
        // addressed to: it supplies a scope-gated core and the chart supplies only the MARKET.
        // In Auto mode those can be different exchanges, and in Overview the core is absent.
        // Reading the row's cap from the chart's core would let the toolbar state exchange B's
        // limit for an order that Apply then sends to core A. One identity for the readout, the
        // popup, and the command.
        let row_target = controls::TradeMetric::Lev.target(self.backend.read(cx), &self.group);
        let (row_limits, toolbar_quote) = self.limits_for(row_target.as_ref(), cx);
        let toolbar_max_order = controls::MaxOrderReadout::of(row_limits);

        // Drop the gear popup before building its content if the core it was seeded from is no
        // longer the active one — its editors hold that core's values, not this one's.
        self.reconcile_core_settings_popup(cx);

        // Build core-settings content only while the Shell-controlled `MoonPopover` is open; the
        // popover itself anchors the content to its button.
        let core_settings_content = self
            .core_settings_open
            .then(|| self.core_settings_popup_content(p, window, cx));

        // Same rule for the quiet-mode popup: build its body only while it is up, since
        // `MoonPopover` takes content eagerly and this runs on every header repaint.
        let quiet_settings_content = self
            .quiet_settings_open
            .then(|| self.quiet_settings_content(p, cx));

        // Read the persisted header-ticker selection or its cached default without mutating Backend.
        let ticker_sel = self.backend.read(cx).header_ticker();
        let (ticker_overlay, ticker_dismiss) = self.ticker_popup_layers(chrome_width, p, cx);

        // Track Main activity with a window-level listener so movement over widgets, panels, and
        // the chart still counts even when they block the root hitbox. Record activity only for an
        // active window. No notification is needed because this updates only a timestamp.
        //
        // Use the capture phase rather than bubble: the chart's element-level mouse-move handler
        // calls `stop_propagation`, which would suppress a bubble listener. Capture runs first, so
        // movement over the chart cannot be missed and accidentally trigger inactivity closure.
        //
        // Registered through a paint-phase hook rather than called here: `on_mouse_event`
        // belongs to paint and `render` runs a phase earlier (`window::input_hook`).
        let activity_hook = {
            let backend = self.backend.clone();
            let group = self.group.clone();
            window_mouse_hook(move |_e: &MouseMoveEvent, phase, window: &mut Window, cx| {
                if phase == DispatchPhase::Capture && window.is_window_active() {
                    backend.update(cx, |b, _| b.note_main_input(&group));
                }
            })
        };

        // A modifier held for a MOUSE gesture — the Ctrl+Left order move, an Alt drag — is a
        // prefix too, and releasing it must not fire a lone-modifier binding. Window-level and in
        // the capture phase for the same reason as the move listener above: the chart consumes its
        // own presses, so a bubble listener on the root would never see them.
        //
        // Same hook, same reason.
        let modifier_hook = {
            let view = cx.entity();
            window_mouse_hook(
                move |_e: &MouseDownEvent, phase, _window: &mut Window, cx| {
                    if phase == DispatchPhase::Capture {
                        view.update(cx, |this, _| this.modifier_watch.interrupt());
                    }
                },
            )
        };

        // Everything above is the prelude: Backend reads whose cost scales with the number
        // of cores in the group rather than with what the frame shows. The three chrome rows
        // and the dock frame are timed one by one below.
        crate::diag::record_us(&crate::diag::SHELL_PRELUDE_US, prelude_us);

        v_flex()
            .size_full()
            .relative() // Anchor absolute popup layers over the dock.
            // A focusable root receives `on_key_down` hotkeys even when Main is empty.
            .track_focus(&self.focus)
            // Main inactivity tracking uses the window-level `on_mouse_event::<MouseMoveEvent>`
            // above because a gated root `.on_mouse_move` cannot see movement over mouse-blocking
            // widgets.
            // Do not set a root background: the central chart region remains transparent for its
            // UnderScene own-pass, while header, toolbar, panels, and status paint their own chrome.
            .font_family(design::mono())
            .text_color(rgb(p.text))
            .text_size(design::t_body(cx))
            .on_key_down(
                cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_hotkey(ev, window, cx)),
            )
            // Caps Lock and a lone modifier are bindable too, and neither arrives as a key press.
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, window, cx| {
                this.on_modifier_hotkey(ev, window, cx)
            }))
            // Capture phase, unlike the hotkey listener above: a key consumed by a focused field
            // never bubbles here, and a modifier held while it was typed must still lose its claim
            // to being a binding of its own.
            // It is also the only place that sees a press unconditionally, which is what the
            // `log.hotkeys` trace needs to tell a swallowed key from an unbound one.
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, _cx| {
                crate::hotkeys::trace_key_arrived(ev);
                this.modifier_watch.interrupt();
            }))
            // ── Header ──────────────────────────────────────────────
            .children(chrome_stubbed().then(|| div().w_full().h(px(design::header_height(cx)))))
            .children((!chrome_stubbed()).then(|| {
                let _t = crate::diag::scope(&crate::diag::SHELL_HEADER_US);
                terminal_chrome::header(
                    &self.group,
                    self.backend.clone(),
                    self.updater.clone(),
                    cx.entity(),
                    ticker_sel,
                    self.header_core_selector_open,
                    self.core_settings_open,
                    core_settings_content,
                    self.quiet_settings_open,
                    quiet_settings_content,
                    chrome_width,
                    p,
                    cx,
                )
            }))
            // Trading toolbar: fixed-height size, leverage, risk, exit, Live, and window-launch
            // sections. It is one chrome row rather than a dock panel.
            .children(chrome_stubbed().then(|| div().w_full().h(px(design::toolbar_height(cx)))))
            .children((!chrome_stubbed()).then(|| {
                let _t = crate::diag::scope(&crate::diag::SHELL_TOOLBAR_US);
                controls::toolbar(
                    &self.backend,
                    &self.group,
                    self.size_edit.clone(),
                    &self.size_input,
                    self.sell_edit.clone(),
                    &self.sell_input,
                    &cx.entity(),
                    self.settings_hint_at(),
                    metric_popup,
                    toolbar_max_order,
                    &toolbar_quote,
                    chrome_width,
                    cx,
                )
            }))
            // One DockArea: Classic keeps its local tree; Auto adds the rail around shared topology.
            .child({
                let _t = crate::diag::scope(&crate::diag::SHELL_DOCK_US);
                self.workspace_body(chrome_width, p, cx)
            })
            // ── Status bar, fully ported from egui's lower `shell::ui` panel ──
            .child({
                let _t = crate::diag::scope(&crate::diag::SHELL_STATUS_US);
                self.status_bar(conn, license, snap, book_levels, fps, chrome_width, cx)
            })
            .child(
                MoonWindowFrame::main("moon-main-window-frame", chrome_width)
                    .header_height(design::HEADER_TOP_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
            // Header price-ticker source picker and its dismiss layer.
            .children(ticker_dismiss)
            .children(ticker_overlay)
            // The window-level input hooks, installed when these are painted.
            .child(activity_hook)
            .child(modifier_hook)
    }
}
