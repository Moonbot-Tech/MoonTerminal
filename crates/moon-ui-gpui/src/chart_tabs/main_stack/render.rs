//! Main chart stack scene composition and size probes.

use super::*;

impl Render for MainChartStack {
    /// Renders the per-chart tab row above either the active full-bleed chart or the virtualized
    /// whole-stack layout.
    ///
    /// Paint stores `host_visible` because a rendered stack is on screen. Child wake stays on the
    /// visible-range path; `set_scene_visible` is not called from here.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Paint means this host is on screen. Store the flag only: `set_scene_visible` would mark
        // every offscreen child visible.
        self.host_visible = true;
        crate::diag::bump(&crate::diag::MAIN_STACK_RENDER);
        let _render_us = crate::diag::scope(&crate::diag::MAIN_STACK_RENDER_US);
        let palette = moon_ui::MoonPalette::active(cx);
        // Before the branch, not inside it: the statistics view owns a live feed, and asking about
        // it only on the empty screen would mean a chart opening left the socket, the board poller
        // and the tick chain running for the rest of the session.
        let crowd = self.sync_crowd_stats(cx);
        if self.charts.is_empty() {
            let screen = self.empty_arrangement(cx);
            let places = EmptyPlaces::restore(&self.backend.read(cx).layout);
            // The grid's form and board compaction are separate thresholds. Repaint when either
            // changes, including a full-board/record transition within the one-column layout. The
            // form is resolved from what is ON and WHERE, so the render and its probe read one
            // answer: two copies of that comparison would repaint on every resize frame or on none.
            let needs = crate::crowd::place::ColumnNeeds::of(&screen.blocks_on(), places, cx);
            let board_thresholds = crate::crowd::place::board_thresholds(cx);
            let width_mode = move |width| {
                needs.form(width).mode()
                    | (crate::crowd::place::board_modes(width, board_thresholds) << 2)
            };
            let width = self.measured.get().width;
            let mode_now = width_mode(width);
            let frame = empty::EmptyFrame {
                screen,
                places,
                form: needs.form(width),
                width,
            };
            let empty = empty::empty_screen(self, frame, crowd, window, palette, cx);
            // Measured here too, for the reason the fullscreen branch keeps its probe: a resize
            // taken while the stack is empty must not leave a size the first divided frame uses.
            return crate::chart_tabs::stack::with_size_probe(
                empty,
                self.measured.clone(),
                cx.entity(),
                mode_now,
                move |size| width_mode(size.width),
            );
        }

        // The row is built first but composed last: it must know the active chart before the body
        // below borrows `self` for the stack layout's closures.
        let tab_row = self.render_tab_row(window, cx);
        let body = self.render_body(palette, cx);
        let Some(tab_row) = tab_row else {
            return body;
        };
        // The body flexes into whatever the row leaves, so the chart loses exactly the row's
        // height and nothing has to subtract it by hand.
        v_flex()
            .size_full()
            .child(tab_row)
            .child(div().flex_1().w_full().min_h(px(0.0)).child(body))
            .into_any_element()
    }
}

impl MainChartStack {
    /// The chart area itself: one full-bleed chart, or the whole stack tiled.
    fn render_body(&mut self, palette: MoonPalette, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active.unwrap_or(0).min(self.charts.len() - 1);
        if !self.show_stack {
            let panel = self.charts[active].panel.clone();
            let entity = cx.entity();
            // The probe rides along in fullscreen too, although nothing is divided there: a window
            // resized or maximised while one chart is full-bleed would otherwise leave the measured
            // size stale, and the first frame back in the stack would divide by the old number and
            // re-lay out — re-baking every chart's own-pass texture — a frame later.
            let measured = self.measured.clone();
            let tile = self
                .render_tile(
                    active,
                    panel,
                    None,
                    false,
                    None,
                    false,
                    rgb(palette.border),
                    entity,
                    palette,
                    true,
                    crate::design::t_body(cx),
                    &moon_ui::MoonTheme::active_tokens(cx),
                )
                .size_full()
                .into_any_element();
            // Fullscreen divides nothing, but it must still RECORD: a resize taken here is the size
            // the first divided frame back in the stack will use.
            return crate::chart_tabs::stack::with_size_probe(
                tile,
                measured,
                cx.entity(),
                1,
                |_| 1,
            );
        }

        // Stack mode uses the per-tab FIT/SCROLL/COMPRESS layout and height, or the global default.
        let (scroll, compress, cfg_h) = resolve_layout(
            self.layout_mode,
            self.layout_height_fit,
            self.layout_height_scroll,
        );
        let count = self.charts.len();
        let border = rgb(palette.border);
        let base_id = format!("main-chart-stack-{}", self.group);
        let horizontal = self
            .layout_orientation
            .unwrap_or(StackOrientation::Vertical)
            .is_horizontal();
        let entity = cx.entity();
        let p = palette;
        let title_size = crate::design::t_body(cx);
        let tokens = moon_ui::MoonTheme::active_tokens(cx);
        let on_visible_range = cx.processor(|this, range: Range<usize>, _window, cx| {
            this.sync_stack_visible_range(range, cx);
        });
        let columns = self.effective_columns(count, horizontal, cfg_h);
        let measured = self.measured.clone();
        let stack = render_chart_stack(
            &base_id,
            self,
            entity,
            count,
            scroll,
            compress,
            horizontal,
            cfg_h,
            columns,
            &self.scroll,
            border,
            |s, ix| s.charts.get(ix).map(|e| e.panel.clone()),
            move |s, ix, panel, size, flex, min_w, horizontal, border, ent| {
                s.render_tile(
                    ix, panel, size, flex, min_w, horizontal, border, ent, p, false, title_size,
                    &tokens,
                )
                .into_any_element()
            },
            |s, ix| compare_role(&s.charts, &s.compare_anchor, s.compare_orderbook_only, ix),
            Some(Box::new(on_visible_range)),
        );
        let probe_cfg = self.grid_cfg();
        crate::chart_tabs::stack::with_size_probe(
            stack,
            measured,
            cx.entity(),
            columns,
            move |size| {
                grid::columns_for(
                    probe_cfg,
                    (f32::from(size.width), f32::from(size.height)),
                    count,
                    horizontal,
                    cfg_h,
                )
            },
        )
    }
}
