//! The trade-log dialog: its chrome, its virtualized line list, and its copy commands.

use super::{TradeLog, TradeLogState};
use crate::design;
use crate::design::moon;
use crate::panels::line_list::{self, ListKey, Sev};
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::applog::LogLine;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonPalette, MoonScrollbarVisibility, MoonVirtualList,
    MoonWindowExt as _, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

/// Height of the line list, in unscaled pixels.
///
/// A trade's log is a few dozen lines; this is tall enough to read a burst around the entry without
/// the dialog filling the screen on a laptop.
const LIST_H: f32 = 460.0;

/// One render-ready line: clock, message, and the severity its color comes from.
///
/// Severity is resolved once here rather than in the row renderer, which the virtual list calls for
/// every visible line on every frame — classifying costs a lowercase allocation and two dozen
/// substring scans over the whole line. The Log panel precomputes the same way into `LineView`.
pub(super) struct TradeLine {
    /// Original UTC timestamp retained so an open dialog can reformat without rescanning files.
    utc_ts: String,
    clock: String,
    msg: String,
    sev: Sev,
    /// Character budget of the rendered row: clock, one gap, and the message.
    width_chars: usize,
}

impl TradeLine {
    /// Text this line contributes to the clipboard.
    fn copy_text(&self) -> String {
        format!("{} {}", self.clock, self.msg)
    }
}

/// Turns scanned lines into render-ready ones, resolving severity and civil clocks once per line.
///
/// Args:
///     lines: Raw rows whose timestamps are stored as UTC text.
///     zone: Selected application-wide display zone.
///
/// Returns:
///     Render-ready rows whose visible and copied clocks use `zone`.
pub(super) fn build_lines(lines: Vec<LogLine>, zone: chrono_tz::Tz) -> Vec<TradeLine> {
    lines
        .into_iter()
        .map(|line| {
            let sev = line_list::classify_lower(line.level, &line.msg.to_lowercase()).sev;
            let clock = moon_core::util::display_time::format_utc_millis_clock(&line.ts, zone);
            let width_chars = clock.chars().count() + 1 + line.msg.chars().count();
            TradeLine {
                utc_ts: line.ts,
                clock,
                msg: line.msg,
                sev,
                width_chars,
            }
        })
        .collect()
}

/// Reformat cached trade-log clocks after the selected display zone changes.
///
/// Args:
///     lines: Render-ready rows retaining their original UTC timestamps.
///     zone: Newly selected application-wide display zone.
///
/// Returns:
///     Nothing; clocks and their width budgets are updated in place.
pub(super) fn rezone_lines(lines: &mut [TradeLine], zone: chrono_tz::Tz) {
    for line in lines {
        line.clock = moon_core::util::display_time::format_utc_millis_clock(&line.utc_ts, zone);
        line.width_chars = line.clock.chars().count() + 1 + line.msg.chars().count();
    }
}

/// Widest row among render-ready lines, capped so one outlier cannot size the viewport.
pub(super) fn widest_chars(lines: &[TradeLine]) -> usize {
    lines
        .iter()
        .map(|line| line.width_chars)
        .max()
        .unwrap_or(0)
        .min(line_list::WIDEST_CHARS_CAP)
}

/// Opens the dialog around an already-created state entity.
pub(super) fn open_dialog(entity: Entity<TradeLog>, window: &mut Window, cx: &mut App) {
    window.open_unique_moon_dialog("report-trade-log", cx, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let body = entity.clone();
        let footer = entity.clone();
        let title = {
            let this = entity.read(cx);
            format!(
                "{} — {} · {} · #{}",
                t!("report.trade_log.title"),
                this.request.core_name,
                this.request.coin,
                this.request.task_id
            )
        };
        dialog
            .w(px(1040.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            // The list is a view of its own, so the background scan landing repaints the dialog
            // through the ordinary entity notification rather than a whole-window refresh.
            .content(move |content, _window, _cx| content.child(body.clone()))
            .footer(dialog_footer(footer, p))
    });
}

/// Footer: what the scan found, and the copy commands.
fn dialog_footer(entity: Entity<TradeLog>, p: MoonPalette) -> AnyElement {
    let copy_all = entity.clone();
    h_flex()
        .w_full()
        .items_center()
        .gap_2()
        .text_color(moon(p.text))
        // Nothing dynamic lives here: the dialog builds its footer once, while the line count and
        // the scan's outcome change when the background read lands. Those belong to the body view,
        // which repaints itself.
        .child(div().flex_1())
        .child(
            MoonButton::new("trade-log-copy-all")
                .size(MoonButtonSize::Micro)
                .outline()
                .label(t!("report.trade_log.copy_all").to_string())
                .on_click(move |_, _window, app| {
                    copy_all.update(app, |this, cx| this.copy_all(cx));
                })
                .render(),
        )
        .child(
            MoonButton::new("trade-log-close")
                .ghost()
                .size(MoonButtonSize::Micro)
                .label(t!("dialogs.close").to_string())
                .on_click(move |_, window, cx| window.close_dialog(cx))
                .render(),
        )
        .into_any_element()
}

impl TradeLog {
    /// Lines currently displayed, empty while the scan runs.
    fn lines(&self) -> &[TradeLine] {
        match &self.state {
            TradeLogState::Loading => &[],
            TradeLogState::Ready { lines, .. } => lines,
        }
    }

    /// Copies every found line.
    fn copy_all(&mut self, cx: &mut Context<Self>) {
        let all = 0..self.lines().len();
        self.copy_rows(all, cx);
    }

    /// Copies the given rows as newline-separated text.
    fn copy_rows(&self, rows: std::ops::Range<usize>, cx: &mut Context<Self>) {
        line_list::copy_rows(self.lines(), rows, TradeLine::copy_text, cx);
    }

    /// Copies the selection when row `ix` belongs to it, and that row alone otherwise.
    fn copy_row_or_selection(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.copy_rows(self.selection.range_for(ix), cx);
    }

    /// Handles copy and select-all while the list holds focus.
    ///
    /// Escape is deliberately absent: MoonUI's dialog context binds it to `CancelDialog`, so the
    /// same keystroke closes the window — clearing a selection under it would be a hidden second
    /// meaning for a key the user pressed to leave.
    fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let count = self.lines().len();
        match line_list::handle_list_key(&mut self.selection, ev, count) {
            Some(ListKey::Copy(rows)) => self.copy_rows(rows, cx),
            Some(ListKey::SelectedAll) => cx.notify(),
            None => {}
        }
    }
}

impl Render for TradeLog {
    /// Renders the found lines, or the reason there are none to show.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The dialog focuses its OWN handle when it opens, and this list is a descendant of it, so
        // without this the copy keys would be dead until the first row press moved focus here.
        if !self.focused_once {
            self.focused_once = true;
            let focus = self.focus.clone();
            window.focus(&focus, cx);
        }
        let p = MoonPalette::active(cx);
        let count = self.lines().len();
        let body: AnyElement = if count == 0 {
            let note = match &self.state {
                TradeLogState::Loading => t!("report.trade_log.loading").to_string(),
                TradeLogState::Ready { .. } => t!("report.trade_log.empty").to_string(),
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgba_from(p.text_soft, 1.0))
                .child(note)
                .into_any_element()
        } else {
            let weak = cx.entity().downgrade();
            let list = MoonVirtualList::new(
                "trade-log-rows",
                count,
                design::fit_h_value(cx, 18.0, 14.0, 2.0),
                move |ix, _window, app| {
                    weak.upgrade()
                        .and_then(|entity| {
                            let this = entity.read(app);
                            let selected = this.selection.contains(ix);
                            this.lines()
                                .get(ix)
                                .map(|line| row(line, ix, selected, &weak, p, app))
                        })
                        .unwrap_or_else(|| div().into_any_element())
                },
            )
            .track_scroll(&self.scroll)
            .surface(false)
            .border(false)
            .radius(0.0)
            // Hidden here and carried by the viewport below, as in the Log panel: the list is as
            // wide as its widest line, so its own overlay scrollbar would sit off screen.
            .scrollbar_visibility(MoonScrollbarVisibility::Hidden);
            let content_w = line_list::content_width(self.widest_chars, cx);
            line_list::hscroll_viewport(
                "trade-log-hscroll",
                list,
                &self.hscroll,
                &self.scroll,
                content_w,
                window,
                cx,
            )
            .into_any_element()
        };
        let (status, alarming) = self.status_line(count);
        v_flex()
            .id("trade-log-body")
            .w_full()
            .h(design::ui_px(cx, LIST_H))
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| this.on_key(ev, cx)))
            // Both halves, as in the Log panel: `on_mouse_up` only fires over this list's own
            // hitbox, so a drag released elsewhere would leave the gesture live.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _w, _cx| this.selection.release()),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _ev: &MouseUpEvent, _w, _cx| this.selection.release()),
            )
            .child(div().flex_1().w_full().min_h_0().child(body))
            .child(
                div()
                    .w_full()
                    .px_1()
                    .text_color(rgba_from(if alarming { p.amber } else { p.text_soft }, 1.0))
                    .child(status),
            )
    }
}

impl TradeLog {
    /// Line shown under the list: how many lines were found, and whether the list is incomplete.
    ///
    /// Returns the text and whether it warns rather than merely counts.
    fn status_line(&self, count: usize) -> (String, bool) {
        match &self.state {
            // While loading, the centered placeholder already says so; repeating it under the list
            // would put the same sentence on screen twice.
            TradeLogState::Loading => (String::new(), false),
            TradeLogState::Ready {
                truncated: true, ..
            } => (
                t!("report.trade_log.truncated", n = count).to_string(),
                true,
            ),
            TradeLogState::Ready { .. } => {
                (t!("report.trade_log.count", n = count).to_string(), false)
            }
        }
    }
}

/// Renders one log line: muted clock, message in its severity color, with selection painting.
fn row(
    line: &TradeLine,
    ix: usize,
    selected: bool,
    weak: &WeakEntity<TradeLog>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let weak_press = weak.clone();
    let weak_drag = weak.clone();
    let weak_copy = weak.clone();
    h_flex()
        .w_full()
        .gap_1()
        .items_baseline()
        .px_1()
        .text_size(design::t_body(cx))
        .when(selected, |row| row.bg(line_list::selected_row_bg(p)))
        .child(
            div()
                .flex_none()
                .text_color(rgba_from(p.text_muted, 1.0))
                .child(line.clock.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_color(moon(line_list::sev_color(line.sev, p)))
                .child(line.msg.clone()),
        )
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _w, app| {
            if let Some(entity) = weak_press.upgrade() {
                let shift = ev.modifiers.shift;
                entity.update(app, |this, cx| {
                    if shift {
                        this.selection.shift_press(ix);
                    } else {
                        this.selection.press(ix);
                    }
                    cx.notify();
                });
            }
        })
        .on_mouse_move(move |ev: &MouseMoveEvent, _w, app| {
            if ev.pressed_button != Some(MouseButton::Left) {
                return;
            }
            if let Some(entity) = weak_drag.upgrade() {
                entity.update(app, |this, cx| {
                    if this.selection.drag_to(ix) {
                        cx.notify();
                    }
                });
            }
        })
        .on_mouse_down(MouseButton::Right, move |_ev, _w, app| {
            if let Some(entity) = weak_copy.upgrade() {
                entity.update(app, |this, cx| this.copy_row_or_selection(ix, cx));
            }
        })
        .into_any_element()
}
