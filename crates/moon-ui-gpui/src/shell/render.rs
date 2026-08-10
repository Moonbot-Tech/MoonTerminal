//! Compose the group-window frame (`Render for Shell`) and the content of the open trading-metric
//! popup.

use std::time::Instant;

use gpui::*;

use moon_ui::{MoonPalette, MoonWindowFrame, v_flex};

use super::Shell;
use crate::chrome::terminal_chrome;
use crate::{controls, design};

impl Shell {
    /// Build the CONTENT of the open trading-metric popup (TP/SL/leverage), or `None` when closed.
    ///
    /// Content only. The box around it — frame, background, width, and above all its POSITION —
    /// belongs to the anchored `MoonPopover` that `controls::toolbar` wraps around the matching
    /// metric button. That is the whole point of the arrangement: the popup follows its trigger by
    /// construction, so no layout term in the toolbar can desync it.
    fn open_metric_content(&self, p: MoonPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (metric, target) = self.open_metric_popup.clone()?;
        use controls::TradeMetric;
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
        Some(controls::metric_popup_content(
            metric,
            &target,
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

        // Collect frame and status diagnostics here; chart data, input, and axes stay in ChartPanel.
        // Smoothed render FPS is shown in the status bar, matching the egui host.
        let now_inst = Instant::now();
        if let Some(prev) = self.last_frame {
            let dt = now_inst.duration_since(prev).as_secs_f32().max(1e-4);
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.last_frame = Some(now_inst);
        let fps = self.fps;

        let (conn, license, snap, book_levels) = {
            let b = self.backend.read(cx);
            let mut conn = b.session.conn_summary_group(&self.group);
            // The disconnected-cores tooltip is a core list like any other — rank it the same
            // way, or it reads in a different order than the header pill right above it.
            crate::core_order::CoreOrder::new(&b.config).sort_by(&mut conn.down, |(id, _, _)| *id);
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
            .map(|(metric, _)| *metric)
            .zip(self.open_metric_content(p, cx));

        // Drop the gear popup before building its content if the core it was seeded from is no
        // longer the active one — its editors hold that core's values, not this one's.
        self.reconcile_core_settings_popup(cx);

        // Build core-settings content only while the Shell-controlled `MoonPopover` is open; the
        // popover itself anchors the content to its button.
        let core_settings_content = self
            .core_settings_open
            .then(|| self.core_settings_popup_content(p, cx));

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
        {
            let backend = self.backend.clone();
            let group = self.group.clone();
            window.on_mouse_event::<MouseMoveEvent>(move |_e, phase, window, cx| {
                if phase == DispatchPhase::Capture && window.is_window_active() {
                    backend.update(cx, |b, _| b.note_main_input(&group));
                }
            });
        }

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
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| this.on_hotkey(ev, cx)))
            // ── Header ──────────────────────────────────────────────
            .child(terminal_chrome::header(
                &self.group,
                self.backend.clone(),
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
            ))
            // Trading toolbar: fixed-height size, leverage, risk, exit, Live, and window-launch
            // sections. It is one chrome row rather than a dock panel.
            .child(controls::toolbar(
                &self.backend,
                &self.group,
                self.size_edit.clone(),
                &self.size_input,
                self.sell_edit.clone(),
                &self.sell_input,
                &cx.entity(),
                metric_popup,
                chrome_width,
                cx,
            ))
            // One DockArea: Classic keeps its local tree; Auto adds the rail around shared topology.
            .child(self.workspace_body(chrome_width, p, cx))
            // ── Status bar, fully ported from egui's lower `shell::ui` panel ──
            .child(self.status_bar(conn, license, snap, book_levels, fps, chrome_width, cx))
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
    }
}
