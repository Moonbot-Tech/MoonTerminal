//! Pointing at an auto line: the hover card, and the right-click menu with "pin as figure", "hide
//! this line" and "configure row".
//!
//! Auto lines are hit-tested for these two gestures ONLY. They live outside the figure store, so
//! the figure paths — selection, dragging, the delete gesture, the alert list — never see them;
//! the hit test reads what the pane last DREW (`ChartEngine::auto_line_at`), so a hit is always on
//! a line that is on screen.
//!
//! The card is a GPUI overlay like the trade-arrow card: theme colours and the font slider come for
//! free. It appears only after the pointer has rested on a line (`CARD_DWELL`), so a sweep across
//! the plot's full-width levels does not repaint the scene at every crossing. The hover itself is
//! handed to the engine at once, which prints an `OnHover` caption for it — a present, no rebuild.

use gpui::*;
use moon_ui::{
    MoonContextMenuWindowExt as _, MoonMenuItem, MoonPalette, MoonSurface, MoonSurfaceVariant,
    MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::config::ChartTabKind;
use moon_core::util::now_unix_ms_i64;

use super::super::ChartPanel;
use crate::chartdx::auto_figures_sync::LineKey;
use crate::chartdx::auto_figures_sync::caption::{age_text, events_text, tf_key};
use crate::design;

/// Card width in logical pixels, before the UI-scale slider: four short rows of name and value.
const CARD_W: f32 = 220.0;
/// Gap between the cursor and the card's nearest edge, on both axes.
const CARD_GAP: f32 = 12.0;
/// Gap kept between the card and the slot edge it is pushed against.
const CARD_EDGE_INSET: f32 = 6.0;
/// How long the pointer must rest on an auto line before its card appears.
///
/// The card is a GPUI element, so showing or hiding it repaints the whole scene. Auto levels run
/// across the whole plot, and a pointer sweeping over the chart crosses them constantly: shown on
/// contact, the card turned every crossing into two scene repaints — measured with FireTest's
/// mouse storm on the bench, `chart_render` 64 → 110 per second on a chart with two day-extreme
/// lines. Behind a short rest, a sweep costs nothing but the line's own highlight (a present), and
/// a reader who stops on a line still gets the card at tooltip speed.
const CARD_DWELL: std::time::Duration = std::time::Duration::from_millis(150);

/// The auto line under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::panels::chart) struct AutoHover {
    pub pane: usize,
    pub key: LineKey,
}

/// This chart's auto-line hover.
#[derive(Debug, Default)]
pub(in crate::panels::chart) struct AutoHoverState {
    hover: Option<AutoHover>,
    /// Point of the most recent hit test, for the movement threshold every chart hover uses.
    probe: Option<(f32, f32)>,
    /// Whether the card for `hover` is on screen: set once the pointer has rested [`CARD_DWELL`]
    /// on the line, cleared with every change of `hover`.
    shown: bool,
    /// Bumped with every change of `hover`, so a dwell timer armed for an earlier line does nothing.
    dwell_gen: u64,
}

impl ChartPanel {
    /// Update the hovered auto line from a pointer position. Returns whether the GPUI tree must
    /// repaint, which it must only when the card changes.
    ///
    /// `within` is false when the pointer left the chart slot, which clears the hover.
    pub(in crate::panels::chart) fn sync_auto_hover(
        &mut self,
        pos: (f32, f32),
        within: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if !within {
            return self.clear_auto_hover();
        }
        if !super::super::trade::hover_probe_due(self.auto_figures.hover.probe, pos) {
            return false;
        }
        self.auto_figures.hover.probe = Some(pos);
        let hit = self.auto_hit(pos, cx);
        let generation = self.auto_figures.hover.dwell_gen;
        let repaint = self.apply_auto_hover(hit);
        // One timer per line entered, not per pointer event: a pointer resting on (or jittering
        // along) the same line would otherwise spawn a timer task at every move.
        if self.auto_figures.hover.dwell_gen != generation {
            self.arm_auto_card(cx);
        }
        repaint
    }

    /// Drop the hover when the pointer leaves the chart slot. Returns whether the card was shown.
    pub(in crate::panels::chart) fn clear_auto_hover(&mut self) -> bool {
        self.auto_figures.hover.probe = None;
        self.apply_auto_hover(None)
    }

    /// Re-run the hit test at the last cursor position, without the movement threshold.
    ///
    /// The chart scrolls and re-selects lines between pointer events, so a line can leave from
    /// under a resting cursor; the render path calls this so the card never outlives its line. No
    /// notify: this runs inside render, and the card is built from the fresh state right after.
    pub(in crate::panels::chart) fn revalidate_auto_hover(&mut self, cx: &mut Context<Self>) {
        if self.auto_figures.hover.hover.is_none() {
            return;
        }
        let hit = self.input.cursor.and_then(|pos| self.auto_hit(pos, cx));
        let generation = self.auto_figures.hover.dwell_gen;
        self.apply_auto_hover(hit);
        // Runs in every render while a line is hovered: arm only when the line under a resting
        // cursor actually changed, never once per frame.
        if self.auto_figures.hover.dwell_gen != generation {
            self.arm_auto_card(cx);
        }
    }

    /// The auto line at `pos` (panel-local device pixels), if any.
    ///
    /// A user figure under the pointer wins: it is the thing that can be grabbed, and two
    /// highlights for one point would leave the reader guessing which one a click acts on.
    fn auto_hit(&self, pos: (f32, f32), _cx: &Context<Self>) -> Option<AutoHover> {
        if self.orderbook_only || self.fig_hover.is_some() {
            return None;
        }
        let pane = self.chart_gesture_pane_at(pos)?;
        let map = self.pane_map(pane)?;
        if pos.0 < map.plot.x
            || pos.0 > map.plot.x + map.plot.w
            || pos.1 < map.plot.y
            || pos.1 > map.plot.y + map.plot.h
        {
            return None;
        }
        let key = self
            .chart
            .auto_line_at(pane, pos, &map, self.fig_hit_threshold())?;
        Some(AutoHover { pane, key })
    }

    /// Apply a hit result: hand it to the engine for the `OnHover` captions and the highlight (a
    /// present, never a scene repaint). Returns whether the GPUI tree must repaint — only when a
    /// card that was ON SCREEN goes away; a new card waits for [`Self::arm_auto_card`].
    fn apply_auto_hover(&mut self, hit: Option<AutoHover>) -> bool {
        let state = &mut self.auto_figures.hover;
        if state.hover == hit {
            return false;
        }
        let was_shown = std::mem::take(&mut state.shown);
        state.hover = hit;
        state.dwell_gen = state.dwell_gen.wrapping_add(1);
        self.chart.set_auto_hover(hit.map(|h| (h.pane, h.key)));
        was_shown
    }

    /// Show the card once the pointer has rested on the hovered line for [`CARD_DWELL`].
    ///
    /// Does nothing when no line is hovered or its card is already up. The timer checks the
    /// generation it was armed for, so a pointer that has moved on to another line, or off every
    /// line, by the time it fires is left alone.
    fn arm_auto_card(&mut self, cx: &mut Context<Self>) {
        let state = &self.auto_figures.hover;
        if state.hover.is_none() || state.shown {
            return;
        }
        let generation = state.dwell_gen;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(CARD_DWELL).await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.auto_figures.hover;
                if state.dwell_gen == generation && state.hover.is_some() && !state.shown {
                    state.shown = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The hover card, or `None` when no auto line is hovered: the row's name, then its
    /// timeframe, touch count, age and current event.
    pub(in crate::panels::chart) fn auto_hover_card(
        &self,
        sf: f32,
        palette: MoonPalette,
        cx: &App,
    ) -> Option<AnyElement> {
        let hover = self
            .auto_figures
            .hover
            .hover
            .filter(|_| self.auto_figures.hover.shown)?;
        let (cursor_x, cursor_y) = self.input.cursor?;
        let now_ms = now_unix_ms_i64() as f64;
        let (title, rows) = self.chart.with_auto_line(hover.pane, hover.key, |line| {
            let event = events_text(line.events);
            let event = if event.is_empty() {
                t!("auto_figures_chart.card.no_event").to_string()
            } else {
                event
            };
            (
                line.title.text(),
                [
                    (
                        t!("auto_figures_chart.card.tf"),
                        t!(tf_key(line.tf)).to_string(),
                    ),
                    (
                        t!("auto_figures_chart.card.touches"),
                        line.touches.to_string(),
                    ),
                    (
                        t!("auto_figures_chart.card.age"),
                        age_text(now_ms - line.first_ms),
                    ),
                    (t!("auto_figures_chart.card.event"), event),
                ],
            )
        })?;
        let sf = sf.max(0.1);
        let slot = self.chart.slot_dev_size();
        let slot_w = slot.0 as f32 / sf;
        let slot_h = slot.1 as f32 / sf;
        let gap = f32::from(design::ui_px(cx, CARD_GAP));
        let inset = f32::from(design::ui_px(cx, CARD_EDGE_INSET)).min(slot_w * 0.25);
        // A preferred width, never a demanded one: a pane can be dragged narrower than the card.
        let card_w = f32::from(design::ui_px(cx, CARD_W)).min((slot_w - inset * 2.0).max(1.0));
        let (cursor_x, cursor_y) = (cursor_x / sf, cursor_y / sf);
        let left = if cursor_x + gap + card_w + inset <= slot_w {
            cursor_x + gap
        } else {
            cursor_x - gap - card_w
        }
        .clamp(inset, (slot_w - card_w - inset).max(inset));
        let top = (cursor_y + gap).clamp(inset, (slot_h - inset).max(inset));
        let row = |name: std::borrow::Cow<'_, str>, value: String| {
            h_flex()
                .w_full()
                .justify_between()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(palette.text_muted))
                        .child(name.to_string()),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(palette.text))
                        .font_family(design::mono())
                        .child(value),
                )
        };
        Some(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(card_w))
                .max_h(px((slot_h - top - inset).max(1.0)))
                .overflow_hidden()
                .shadow_md()
                .child(
                    MoonSurface::new()
                        .variant(MoonSurfaceVariant::Card)
                        // Opaque over a busy chart: the Card variant's faint tint is unreadable
                        // against candles.
                        .bg_color(palette.card)
                        .bg_alpha(1.0)
                        .border_color(palette.border_card)
                        .child(
                            v_flex()
                                .w_full()
                                .gap(design::ui_px(cx, 3.0))
                                .p(design::ui_px(cx, 8.0))
                                .child(
                                    div()
                                        .text_size(design::t_body(cx))
                                        .text_color(rgb(palette.text))
                                        .child(title),
                                )
                                .children(rows.into_iter().map(|(name, value)| row(name, value))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Open the menu of a right-clicked auto line. Returns `true` when it opened, so the caller
    /// suppresses the paired release; `false` leaves the right click to whatever is under it.
    pub(in crate::panels::chart) fn try_open_auto_figure_menu(
        &mut self,
        local_pos: (f32, f32),
        menu_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(hit) = self.auto_hit(local_pos, cx) else {
            return false;
        };
        let view = cx.entity();
        let pin = MoonMenuItem::with_key(
            "auto-fig-pin",
            t!("auto_figures_chart.menu.pin").to_string(),
        )
        .on_click({
            let view = view.clone();
            move |_, window, app| {
                window.close_context_menu(app);
                view.update(app, |this: &mut Self, vcx| this.pin_auto_line(hit, vcx));
            }
        });
        let hide = MoonMenuItem::with_key(
            "auto-fig-hide",
            t!("auto_figures_chart.menu.hide").to_string(),
        )
        .on_click({
            let view = view.clone();
            move |_, window, app| {
                window.close_context_menu(app);
                view.update(app, |this: &mut Self, vcx| this.hide_auto_line(hit, vcx));
            }
        });
        let kind = self.default_kind;
        let configure = MoonMenuItem::with_key(
            "auto-fig-configure",
            t!("auto_figures_chart.menu.configure").to_string(),
        )
        .on_click(move |_, window, app| {
            window.close_context_menu(app);
            view.update(app, |this: &mut Self, vcx| {
                this.configure_auto_row(kind, hit.key.row_ix, window, vcx);
            });
        });
        window.open_fitted_moon_context_menu(
            cx,
            "chart-auto-fig-menu",
            menu_pos,
            vec![pin, hide, configure],
            200.0,
            560.0,
        );
        cx.notify();
        true
    }

    /// Copy the line into the figure store as ordinary local figures of the tools and style it is
    /// drawn with — a squeeze as its level and its sloped line — and select the first, so it can be
    /// moved, restyled and armed as an alert at once. The auto line itself stays: the next run
    /// draws it again whatever the store holds.
    fn pin_auto_line(&mut self, hit: AutoHover, cx: &mut Context<Self>) {
        let Some((core, market)) = self.fig_pane_key(hit.pane) else {
            return;
        };
        let figures = self.chart.auto_line_figures(hit.pane, hit.key);
        if figures.is_empty() {
            return;
        }
        let created_ms = now_unix_ms_i64() as f64;
        let first = {
            let b = self.backend.read(cx);
            let mut store = b.figures.borrow_mut();
            figures
                .into_iter()
                .map(|mut fig| {
                    fig.created_ms = created_ms;
                    fig.alert = false;
                    fig.shared = false;
                    store.add(core, &market, fig)
                })
                .reduce(|first, _| first)
        };
        if let Some(id) = first {
            self.backend.update(cx, |b, bcx| {
                b.fig_selected = Some((core, market.clone(), id));
                bcx.notify();
            });
        }
        self.sync_fig_visual(cx);
    }

    /// Hide the line on this pane until it changes market. Not persisted.
    fn hide_auto_line(&mut self, hit: AutoHover, cx: &mut Context<Self>) {
        let Some(target) = self.fig_pane_key(hit.pane) else {
            return;
        };
        let Some(pane) = self
            .auto_figures
            .panes
            .iter_mut()
            .find(|p| p.target == target)
        else {
            return;
        };
        if !pane.hidden.contains(&hit.key) {
            pane.hidden.push(hit.key);
        }
        if self.publish_auto_figures(&target) {
            self.resync_userdata(cx);
        }
        self.clear_auto_hover();
        cx.notify();
    }

    /// Open the row editor for row `row_ix` of `kind`'s auto-figure configuration — the same modal
    /// the "Автофигуры" popup opens from a row's name.
    ///
    /// Nothing is reopened on dismiss: the editor was entered from the chart's context menu, which
    /// has already closed, not from the popup. The editor refuses (and this logs) when the row is
    /// gone or its detector is one this build does not know — a stale menu cannot edit the wrong
    /// row.
    fn configure_auto_row(
        &mut self,
        kind: ChartTabKind,
        row_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = self.backend.clone();
        if !crate::panels::open_auto_figure_row_editor(&backend, kind, row_ix, window, cx, |_| {}) {
            log::debug!("auto figures: row {row_ix} of {kind:?} has no editor to open");
        }
    }
}
