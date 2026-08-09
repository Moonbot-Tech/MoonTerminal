//! The Log panel's element tree: filter controls, the virtualized rows, and their scroll areas.
//!
//! Split from [`super`], which owns the panel's state and row collection. Everything here runs on
//! the frame thread, so what it may do is bounded: read prepared state, build elements, wire
//! listeners.

use super::*;

impl Render for LogPanel {
    /// Renders the panel and activates direct detached-window hosts on their first frame.
    ///
    /// Args:
    ///     window: Host window, which the row viewport's scrollbars keep their drag state in.
    ///     cx: Panel context used for backend reads, controls, and deferred reloads.
    ///
    /// Returns:
    ///     The complete responsive Log panel element tree.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // First line, as every sibling panel does it. Without a render rate, a cost inside this
        // element tree cannot be told apart from one on the revision path — and the whole point of
        // the counters below is telling those two apart.
        crate::diag::bump(&crate::diag::LOG_RENDER);
        if !self.refresh.is_active() {
            self.set_refresh_active(true, cx);
        } else if self.refresh.take_observed_reload() {
            let backend = self.backend.clone();
            self.pull_rows(backend.read(cx), cx);
        }
        let p = MoonPalette::active(cx);

        let (effective_source, sources) = {
            let backend = self.backend.read(cx);
            let (source, _, _) = self.effective_selection(backend);
            (source, self.sources(backend))
        };
        // Only the aggregate and exchange sources fill a row's source column with a CORE name;
        // Local fills it with a module path, which selects nothing.
        let is_agg = matches!(
            effective_source,
            LogSource::Aggregate | LogSource::Exchange(_)
        );
        let total = self.buf.total();

        // Build the wrapping filter and follow controls.
        let mut controls = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1();
        controls = controls.child(self.source_combo(&sources, cx));
        if !is_agg {
            controls = controls
                .child(
                    div()
                        .text_size(crate::design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("log.file").to_string()),
                )
                .child(self.file_combo(&self.available_files, cx));
        }
        controls = controls
            .child(
                div().w(px(180.0)).child(
                    MoonInput::new("log-query")
                        .state(&self.query)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(
                MoonCheckbox::new("log-errors-only")
                    .label(t!("log.errors_only").to_string())
                    .checked(self.errors_only)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        if t.errors_only != *ch {
                            t.errors_only = *ch;
                            t.selection.clear();
                            t.apply_filter(cx);
                            cx.notify();
                        }
                    })),
            )
            .child(
                MoonCheckbox::new("log-live")
                    .label(t!("log.follow_tail").to_string())
                    .checked(self.following())
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        // A manual toggle invalidates any delayed automatic resumption.
                        t.scroll_gen = t.scroll_gen.wrapping_add(1);
                        if *ch {
                            t.resume_live(); // Reload on render and return to the selection's tail.
                        } else {
                            // Manual disable freezes following until the user enables it again.
                            t.live = false;
                            t.scroll_pause = false;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("log.count", shown = self.buf.visible(), total = total).to_string()),
            );
        // A removable chip for the coin filter, in the blue its clickable token wears in the rows.
        // The source name needs none: clicking one selects that core in the source list above,
        // which is its own indicator and its own way back.
        if let Some(coin) = self.coin_filter.clone() {
            controls = controls.child(
                div()
                    .id("log-coin-chip")
                    .flex_none()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.blue))
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.blue))
                    .child(format!("{coin} ✕"))
                    .on_click(cx.listener(|t, _, _, cx| t.set_coin_filter(None, cx))),
            );
        }

        // Build the tail-oriented virtualized list or its empty-state message.
        let weak = cx.entity().downgrade();
        let body: AnyElement = if self.buf.visible() == 0 {
            let msg = if total == 0 {
                t!("dock.log.empty").to_string()
            } else {
                t!("log.empty_filtered").to_string()
            };
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.text_soft))
                .child(msg)
                .into_any_element()
        } else {
            let scroll = self.scroll.clone();
            let query = self.query.read(cx).value().trim().to_lowercase();
            let list_el = MoonVirtualList::new(
                "log-virtual-rows",
                self.buf.visible(),
                // Scale row height with the font because MoonVirtualList accepts raw pixels; a
                // fixed 18 px row clipped text at the +6 font setting.
                crate::design::fit_h_value(cx, 18.0, 14.0, 2.0),
                move |ix, _w, app| {
                    weak.upgrade()
                        .and_then(|e| {
                            let panel = e.read(app);
                            let selected = panel.selection.contains(ix);
                            let ctx = row::RowCtx {
                                ix,
                                selected,
                                source_is_core: is_agg,
                                query: &query,
                                panel: &weak,
                            };
                            panel
                                .buf
                                .at(ix)
                                .map(|line| row::log_row(line, &ctx, p, app))
                        })
                        .unwrap_or_else(|| div().into_any_element())
                },
            )
            .track_scroll(&scroll)
            .surface(false)
            .border(false)
            .radius(0.0)
            // The list is as wide as the widest row, so its own overlay scrollbar would ride the
            // right edge of the CONTENT — off screen for a wide log. The viewport below carries the
            // vertical scrollbar instead, bound to this same handle.
            .scrollbar_visibility(MoonScrollbarVisibility::Hidden);
            // Width of the widest row, so long lines scroll sideways instead of being clipped.
            let content_w = line_list::content_width(self.buf.widest_chars(), cx);
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(line_list::hscroll_viewport(
                    "log-hscroll",
                    list_el,
                    &self.hscroll,
                    &scroll,
                    content_w,
                    window,
                    cx,
                ))
                // Any wheel event over the list pauses effective following and starts its timer.
                .on_scroll_wheel(cx.listener(|t, _e: &ScrollWheelEvent, _w, cx| {
                    t.pause_follow(cx);
                }))
                // So does grabbing a scrollbar, or the next reload would yank the list back down.
                // Capture phase, because a bar stops propagation on its own mouse-down. Left button
                // only — a right-click opens the row's copy menu and moves nothing — and an already
                // paused panel is left alone, as a row press leaves it, so a burst arms one timer.
                .capture_any_mouse_down(cx.listener(|t, ev: &MouseDownEvent, _w, cx| {
                    if ev.button != MouseButton::Left {
                        return;
                    }
                    // Held for as long as the button is, so a drag longer than the resume delay
                    // does not hand the list back to the tail mid-gesture.
                    t.press_held = true;
                    if !t.scroll_pause {
                        t.pause_follow(cx);
                    }
                }))
                .into_any_element()
        };

        v_flex()
            .id("log-panel")
            .size_full()
            // The same body colour Orders and Report paint their roots with. Without it the panel
            // showed the dock's own surface and read as a different kind of window.
            .bg(rgb(p.table_body))
            .track_focus(&self.focus)
            // Set the monospace font on this root, as Orders, Assets, and Report do. A detached
            // panel does not inherit it from the dock header; without this, it would render in Inter
            // and disagree with both the docked view and selector-width measurements in controls.rs.
            .font_family(crate::design::mono())
            // Copy, select-all, and clear reach the panel once a row press has focused it.
            .on_key_down(cx.listener(|t, ev: &KeyDownEvent, window, cx| t.on_key(ev, window, cx)))
            // Both halves are needed: `on_mouse_up` only fires over the panel's own hitbox, so a
            // drag released elsewhere would leave the gesture live and let any later left-drag
            // passing over a row rewrite the selection.
            // Releasing also ends the follow-resume hold, which then runs out its own delay from
            // here rather than from the press.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|t, _ev: &MouseUpEvent, _w, _cx| t.release_press()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|t, _ev: &MouseUpEvent, _w, _cx| t.release_press()),
            )
            .child(controls)
            .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
            .child(body)
    }
}
