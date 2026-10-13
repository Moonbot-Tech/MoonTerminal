//! Updates mode: version distribution, the three fleet buttons, and the lane queue.
//!
//! Takes data the panel already computed. Button clicks downgrade a handle; this function
//! never reads `CoreStatusView` during render.

use std::collections::HashMap;
use std::net::IpAddr;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::session::core_update::{CoreUpdatePhase, CoreUpdateRecord};
use moon_ui::{
    MoonBarSegment, MoonButton, MoonButtonVariant, MoonDataTableState, MoonPalette, MoonStackedBar,
    MoonTone, h_flex, v_flex,
};
use rust_i18n::t;

use super::CoreStatusView;
use super::interactions::FleetUpdateKind;
use super::model::CoreStatusRow;
use super::presentation;
use super::updates_list::{VersionShare, updates_table};
use crate::Backend;
use crate::design;

/// Which queue blocks the Updates column draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpdatesBlocks {
    /// Nothing lags, nothing is in flight, and history is empty.
    Empty,
    /// Nothing lags, but live or history rows still render under the caught-up header.
    HeaderAndTable,
    /// At least one core lags. The table renders even when it has no rows of its own.
    Table,
}

/// Pick the queue blocks from lag, in-flight rows, and retained history.
///
/// A caught-up fleet with history must not replace that history with the empty state.
/// A lagging fleet with an empty table must not claim that every core is current.
///
/// Args:
///     lagging: Any scoped core has `version_behind`.
///     live: The in-flight list is non-empty.
///     history: The retained history list is non-empty.
///
/// Returns:
///     Which blocks to draw.
pub(super) fn updates_blocks(lagging: bool, live: bool, history: bool) -> UpdatesBlocks {
    if !lagging && !live && !history {
        UpdatesBlocks::Empty
    } else if !lagging {
        UpdatesBlocks::HeaderAndTable
    } else {
        UpdatesBlocks::Table
    }
}

/// The Updates column: bar, buttons, campaign line, then the queue or the caught-up empty state.
///
/// Args:
///     shares: Version slices, newest first. The unknown bucket is the dash build.
///     behind_n: Cores the behind button would enqueue.
///     selected: Selected cores inside the Updates scope. Zero means the wide button says "all".
///     all_empty: The all/named plan has no eligible core.
///     behind_empty: The behind plan has no eligible core.
///     summary: Joined campaign counters, or empty when none are running.
///     lags: Any scoped core has `version_behind`. History alone does not count.
///     live_rows: In-flight queue rows.
///     history: Closed records, newest first.
///     phases: Current update phase per scoped core. The history retry reads this.
///     server_names: Display name per lane address.
///     state: Persisted updates-table state.
///     zone: Display time zone.
///     now_ms: Injected clock for live durations.
///     newest_label: Label of the newest reported build, or an em dash when nobody reported one.
///     weak: Callback handle. Upgraded only inside a click.
///     backend: Shared terminal state the history retry button commands. Not read as the view.
///     cx: Panel context for palette and the table. Not read as the view entity.
///
/// Returns:
///     The Updates column.
#[allow(clippy::too_many_arguments)]
pub(super) fn updates_pane(
    shares: &[VersionShare],
    behind_n: usize,
    selected: usize,
    all_empty: bool,
    behind_empty: bool,
    summary: &str,
    lags: bool,
    live_rows: Rc<Vec<CoreStatusRow>>,
    history: Rc<Vec<CoreUpdateRecord>>,
    phases: Rc<HashMap<CoreId, CoreUpdatePhase>>,
    server_names: Rc<HashMap<IpAddr, String>>,
    state: &Entity<MoonDataTableState>,
    zone: chrono_tz::Tz,
    now_ms: i64,
    newest_label: &str,
    weak: WeakEntity<CoreStatusView>,
    backend: &Entity<Backend>,
    cx: &Context<CoreStatusView>,
) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let blocks = updates_blocks(lags, !live_rows.is_empty(), !history.is_empty());
    let queue: AnyElement = match blocks {
        UpdatesBlocks::Empty => up_to_date(newest_label, p, cx).into_any_element(),
        UpdatesBlocks::HeaderAndTable => v_flex()
            .size_full()
            .min_h_0()
            .gap(design::ui_px(cx, 8.0))
            .child(caught_up_header(newest_label, p, cx))
            .child(div().flex_1().min_h_0().child(updates_table(
                "core-status-updates",
                live_rows,
                history,
                phases,
                server_names,
                state,
                zone,
                now_ms,
                backend,
                cx,
            )))
            .into_any_element(),
        UpdatesBlocks::Table => updates_table(
            "core-status-updates",
            live_rows,
            history,
            phases,
            server_names,
            state,
            zone,
            now_ms,
            backend,
            cx,
        )
        .into_any_element(),
    };

    v_flex()
        .id("core-status-updates-pane")
        .size_full()
        .min_h_0()
        .gap(design::ui_px(cx, 10.0))
        .p(design::ui_px(cx, 12.0))
        .child(version_bar(shares, p))
        .child(legend(shares, p, cx))
        .child(command_row(
            behind_n,
            selected,
            all_empty,
            behind_empty,
            summary,
            weak,
            p,
            cx,
        ))
        .child(div().flex_1().min_h_0().child(queue))
}

fn version_bar(shares: &[VersionShare], p: MoonPalette) -> impl IntoElement {
    let segments = shares.iter().map(|share| segment(share, p));
    MoonStackedBar::new("core-status-version-bar")
        .height(12.0)
        .radius(3.0)
        .segments(segments)
}

fn segment(share: &VersionShare, p: MoonPalette) -> MoonBarSegment {
    let label = presentation::build_label(&share.parts);
    let legend_base = if share.newest {
        "core_status.updates.legend_newest"
    } else {
        "core_status.updates.legend_build"
    };
    let legend_key = presentation::plural_key(legend_base, share.cores as u64);
    let tip = t!(legend_key.as_str(), build = label, n = share.cores);
    let segment = MoonBarSegment::new(share.cores as f64).label(tip.to_string());
    if share.parts.number == "-" {
        segment.color(p.text_muted)
    } else if share.newest {
        segment.tone(MoonTone::Positive)
    } else {
        segment.tone(MoonTone::Warning)
    }
}

fn legend(shares: &[VersionShare], p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_wrap()
        .gap_x(design::ui_px(cx, 14.0))
        .gap_y(design::ui_px(cx, 6.0))
        .children(shares.iter().map(|share| {
            let label = presentation::build_label(&share.parts);
            let legend_base = if share.newest {
                "core_status.updates.legend_newest"
            } else {
                "core_status.updates.legend_build"
            };
            let legend_key = presentation::plural_key(legend_base, share.cores as u64);
            let text = t!(legend_key.as_str(), build = label, n = share.cores);
            let color = if share.parts.number == "-" {
                p.text_muted
            } else if share.newest {
                p.green
            } else {
                p.amber
            };
            h_flex()
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .flex_none()
                        .w(design::ui_px(cx, 8.0))
                        .h(design::ui_px(cx, 8.0))
                        .rounded(design::ui_px(cx, 2.0))
                        .bg(rgb(color)),
                )
                .child(
                    div()
                        .font_family(design::ui_font())
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_soft))
                        .child(text.to_string()),
                )
        }))
}

fn command_row(
    behind_n: usize,
    selected: usize,
    all_empty: bool,
    behind_empty: bool,
    summary: &str,
    weak: WeakEntity<CoreStatusView>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let behind_view = weak.clone();
    let all_view = weak.clone();
    let named_view = weak;
    let summary = summary.to_string();
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .when(!summary.is_empty(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.amber))
                    .child(summary),
            )
        })
        .child({
            let button = MoonButton::new("core-status-update-behind")
                .label(t!("core_status.updates.behind_n", n = behind_n).to_string())
                .variant(MoonButtonVariant::Solid)
                .disabled(behind_empty)
                .on_click(move |_, window, app| {
                    let Some(view) = behind_view.upgrade() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        this.confirm_fleet_update(FleetUpdateKind::Behind, window, cx);
                    });
                });
            if behind_empty {
                button.tooltip(t!("core_update.fleet.behind_none").to_string())
            } else {
                button
            }
            .render()
        })
        .child(
            MoonButton::new("core-status-update-all")
                .label(if selected > 0 {
                    t!("core_update.fleet.selected", n = selected).to_string()
                } else {
                    t!("core_update.fleet.all").to_string()
                })
                .variant(MoonButtonVariant::Panel)
                .disabled(all_empty)
                .on_click(move |_, window, app| {
                    let Some(view) = all_view.upgrade() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        this.confirm_fleet_update(FleetUpdateKind::All, window, cx);
                    });
                })
                .render(),
        )
        .child(
            MoonButton::new("core-status-update-named")
                .label(t!("core_update.fleet.named").to_string())
                .variant(MoonButtonVariant::Panel)
                .disabled(all_empty)
                .on_click(move |_, window, app| {
                    let Some(view) = named_view.upgrade() else {
                        return;
                    };
                    view.update(app, |this, cx| {
                        this.confirm_fleet_update(FleetUpdateKind::Named, window, cx);
                    });
                })
                .render(),
        )
        .child(
            div()
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_status.updates.hint").to_string()),
        )
}

fn caught_up_header(newest: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    v_flex()
        .flex_none()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text))
                .child(t!("core_status.updates.up_to_date_title").to_string()),
        )
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_status.updates.up_to_date_body", build = newest).to_string()),
        )
}

#[cfg(test)]
mod tests;

/// 40 px status circle drawn above an empty-state title.
fn empty_mark(glyph: &str, bg: Hsla, fg: u32, cx: &App) -> impl IntoElement {
    div()
        .flex_none()
        .w(design::ui_px(cx, 40.0))
        .h(design::ui_px(cx, 40.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bg)
        .text_color(rgb(fg))
        .font_weight(FontWeight::BOLD)
        .text_size(design::t_body_lg(cx))
        .child(glyph.to_string())
}

fn up_to_date(newest: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(design::ui_px(cx, 6.0))
        .child(empty_mark(
            "✓",
            design::moon_alpha(p.green, 0.18),
            p.green,
            cx,
        ))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(design::t_body_lg(cx))
                .text_color(rgb(p.text))
                .child(t!("core_status.updates.up_to_date_title").to_string()),
        )
        .child(
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("core_status.updates.up_to_date_body", build = newest).to_string()),
        )
}
