//! The expert core rail: Auto-style chrome with window-local selection and disclosure.

use std::collections::HashSet;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::feed::ExchangeId;
use moon_core::session::CoreId;
use moon_ui::{
    MoonBadge, MoonBadgeSize, MoonDisclosure, MoonPalette, MoonSize, MoonSpinner, MoonSpinnerSize,
    MoonTone, MoonTooltipView, h_flex, v_flex,
};
use rust_i18n::t;

use crate::controls::core_rail::rail_row_base;
use crate::controls::core_run::{ENGINE_PAUSE_ICON, ENGINE_PLAY_ICON};
use crate::controls::row_selection::RowSelection;
use crate::design;

use super::CoreExpertView;
use super::cores::{CoreRoster, RosterRow, RosterSection};

/// Logical rail width; grows with the same UI scale as rows and text.
const SIDEBAR_W: f32 = 300.0;

/// Window-local actions: none may change the active trading or Auto-workspace core.
enum RailAction {
    Core(CoreId, Modifiers),
    Venue(Option<ExchangeId>),
    Collapse(Option<ExchangeId>, bool),
}

/// Input and callbacks shared by the production view and its layout probe.
struct RailPresentation<'a> {
    roster: &'a CoreRoster,
    selection: &'a RowSelection<CoreId>,
    anchor: Option<CoreId>,
    collapsed: &'a HashSet<Option<ExchangeId>>,
    logos_ready: bool,
    action: Rc<dyn Fn(RailAction, &mut App)>,
}

impl CoreExpertView {
    /// Render the Auto-style rail while routing all selection through the expert window.
    pub(super) fn sidebar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        rail(
            RailPresentation {
                roster: &self.roster,
                selection: &self.selection,
                anchor: self.anchor,
                collapsed: &self.collapsed_venues,
                logos_ready: self.exchange_logos_ready,
                action: Rc::new(move |action, app| {
                    let _ = view.update(app, |this, cx| match action {
                        RailAction::Core(core, modifiers) => {
                            this.select_core_row(core, modifiers, cx)
                        }
                        RailAction::Venue(venue) => this.select_core_venue(venue, cx),
                        RailAction::Collapse(venue, expanded) => {
                            if expanded {
                                this.collapsed_venues.remove(&venue);
                            } else {
                                this.collapsed_venues.insert(venue);
                            }
                            cx.notify();
                        }
                    });
                }),
            },
            p,
            cx,
        )
    }
}

/// Compose the production rail; chrome and text wrap rather than clip at larger zoom.
fn rail(input: RailPresentation<'_>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let total = input.roster.rows().count();
    let ready = input
        .roster
        .rows()
        .filter(|row| row.connected && row.has_page)
        .count();
    let problem = total - ready;
    let summary = [
        (
            "expert-summary-cores",
            t!("workspace.summary_cores", n = total).to_string(),
            p.text_muted,
        ),
        (
            "expert-summary-ready",
            t!("workspace.summary_ready", n = ready).to_string(),
            design::positive_color(p),
        ),
        (
            "expert-summary-problem",
            t!("workspace.summary_problem", n = problem).to_string(),
            if problem > 0 {
                design::danger_color(p)
            } else {
                p.text_muted
            },
        ),
    ];
    let mut list = v_flex()
        .id("core-expert-core-list")
        .debug_selector(|| "expert-list".into())
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scroll()
        .py(design::ui_px(cx, 4.0));
    for section in &input.roster.sections {
        let key = section.venue.as_ref().map(|venue| venue.id);
        list = list.child(venue_heading(
            section,
            !input.collapsed.contains(&key),
            input.logos_ready,
            input.action.clone(),
            p,
            cx,
        ));
        if input.collapsed.contains(&key) {
            continue;
        }
        for row in &section.rows {
            list = list.child(core_row(
                row,
                input.selection.contains(Some(row.core)),
                input.anchor == Some(row.core),
                input.action.clone(),
                p,
                cx,
            ));
        }
    }
    if total == 0 {
        list = list.child(
            div()
                .px(design::ui_px(cx, 12.0))
                .child(t!("core_expert.cores_none").to_string()),
        );
    }
    v_flex()
        .debug_selector(|| "expert-rail".into())
        .flex_none()
        .h_full()
        .w(design::ui_px(cx, SIDEBAR_W))
        .min_h_0()
        .font_family(design::ui_font())
        .text_size(design::t_caption(cx))
        .line_height(design::ui_px(cx, 18.0))
        .text_color(rgb(p.text_muted))
        .border_r_1()
        .border_color(rgb(p.border_hover))
        .bg(rgb(p.gutter))
        .child(
            h_flex()
                .debug_selector(|| "expert-header".into())
                .flex_none()
                .w_full()
                .flex_wrap()
                .justify_between()
                .items_center()
                .px(design::ui_px(cx, 12.0))
                .pt(design::ui_px(cx, 8.0))
                .pb(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .debug_selector(|| "expert-title".into())
                        .flex_none()
                        .text_size(design::t_body(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text))
                        .child(t!("core_expert.cores_title").to_string()),
                )
                .child(
                    div()
                        .debug_selector(|| "expert-selected".into())
                        .flex_none()
                        .font_family(design::mono())
                        .child(
                            t!("core_expert.cores_selected", n = input.selection.len()).to_string(),
                        ),
                ),
        )
        .child(
            h_flex()
                .debug_selector(|| "expert-summary".into())
                .flex_none()
                .w_full()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 4.0))
                .px(design::ui_px(cx, 12.0))
                .pb(design::ui_px(cx, 8.0))
                .border_b_1()
                .border_color(rgb(p.border_soft))
                .children(
                    summary
                        .into_iter()
                        .enumerate()
                        .map(|(index, (id, text, color))| {
                            h_flex()
                                .flex_none()
                                .gap(design::ui_px(cx, 4.0))
                                .when(index > 0, |item| item.child(div().child("·")))
                                .child(
                                    div()
                                        .debug_selector(move || id.into())
                                        .flex_none()
                                        .text_color(rgb(color))
                                        .child(text),
                                )
                        }),
                ),
        )
        .child(list)
        .child(
            div()
                .debug_selector(|| "expert-footer".into())
                .flex_none()
                .w_full()
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 6.0))
                .border_t_1()
                .border_color(rgb(p.border_soft))
                .child(
                    div()
                        .debug_selector(|| "expert-footer-text".into())
                        .w_full()
                        .whitespace_normal()
                        .child(t!("core_expert.rail_hint").to_string()),
                ),
        )
}

/// Render a venue with separate collapse and select-all targets, keyed by exchange identity.
fn venue_heading(
    section: &RosterSection,
    expanded: bool,
    logos_ready: bool,
    action: Rc<dyn Fn(RailAction, &mut App)>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let key = section.venue.as_ref().map(|venue| venue.id);
    let id = match key {
        Some(id) => format!("expert-venue-{}-{}", id.code, id.dex),
        None => "expert-venue-unknown".into(),
    };
    let toggle = action.clone();
    let label = section.label.clone();
    let tooltip = label.clone();
    let logo = logos_ready
        .then(|| {
            section
                .venue
                .as_ref()
                .and_then(|venue| venue.brand())
                .and_then(crate::media::exchange_logos::exchange_logo)
        })
        .flatten();
    h_flex()
        .debug_selector({
            let id = id.clone();
            move || id.clone()
        })
        .flex_none()
        .w_full()
        .h(design::ui_px(cx, 30.0))
        .min_w_0()
        .px(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 4.0))
        .child(
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-caret")
                })
                .flex_none()
                .child(
                    MoonDisclosure::button(SharedString::from(format!("{id}-collapse")), expanded)
                        .size(10.0)
                        .box_size(18.0)
                        .color(p.text_muted)
                        .hover_color(p.text)
                        .on_toggle(move |expanded, _, app| {
                            toggle(RailAction::Collapse(key, *expanded), app)
                        }),
                ),
        )
        .child(
            h_flex()
                .id(SharedString::from(id))
                .flex_1()
                .h_full()
                .min_w_0()
                .items_center()
                .gap(design::ui_px(cx, 7.0))
                .cursor_pointer()
                .hover(move |row| {
                    row.bg(design::moon_alpha(p.accent, design::RAIL_ROW_HOVER_ALPHA))
                })
                .when_some(logo, |row, logo| {
                    row.child(
                        img(logo)
                            .flex_none()
                            .size(design::ui_px(cx, 14.0))
                            .rounded(design::ui_px(cx, 2.0)),
                    )
                })
                .child(
                    div()
                        .debug_selector({
                            let id = match key {
                                Some(id) => format!("expert-venue-{}-{}", id.code, id.dex),
                                None => "expert-venue-unknown".into(),
                            };
                            move || format!("{id}-label")
                        })
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(design::mono())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(p.text))
                        .child(label),
                )
                .child(
                    div()
                        .flex_none()
                        .font_family(design::mono())
                        .child(section.rows.len().to_string()),
                )
                .tooltip(move |_, cx| cx.new(|_| MoonTooltipView::new(tooltip.clone())).into())
                .on_click(move |_, _, app| action(RailAction::Venue(key), app)),
        )
}

/// Render one fixed-height core leaf; only its name shrinks against status and engine slots.
fn core_row(
    row: &RosterRow,
    selected: bool,
    anchor: bool,
    action: Rc<dyn Fn(RailAction, &mut App)>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let core = row.core;
    let name = row.name.clone();
    let tooltip = name.clone();
    let dot = if row.failed {
        design::danger_color(p)
    } else if !row.connected {
        p.text_muted
    } else if !row.has_page {
        p.amber
    } else {
        design::positive_color(p)
    };
    let mut content = h_flex()
        .size_full()
        .min_w_0()
        .gap(design::ui_px(cx, 7.0))
        .child(design::status_dot_sized(dot, 6.0, cx))
        .child(div().flex_none().size(design::ui_px(cx, 12.0)).when_some(
            row.run.trading.filter(|_| row.connected),
            |slot, running| {
                slot.child(
                    svg()
                        .path(if running {
                            ENGINE_PLAY_ICON
                        } else {
                            ENGINE_PAUSE_ICON
                        })
                        .size_full()
                        .text_color(rgb(if !row.run.trading_confirmed {
                            p.text_muted
                        } else if running {
                            design::positive_color(p)
                        } else {
                            design::danger_color(p)
                        })),
                )
            },
        ))
        .child(
            div()
                .debug_selector(move || format!("expert-name-{core}"))
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(design::mono())
                .text_size(design::t_caption(cx))
                .font_weight(if anchor {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(rgb(if !row.connected {
                    p.text_muted
                } else if selected {
                    p.accent
                } else {
                    p.text
                }))
                .child(name),
        );
    if !row.connected || !row.has_page {
        let waiting = row.connected;
        content = content.child(
            h_flex()
                .debug_selector(move || format!("expert-chip-{core}"))
                .flex_none()
                .items_center()
                .gap(design::ui_px(cx, 3.0))
                .when(waiting, |chip| {
                    chip.child(
                        MoonSpinner::new()
                            .size(MoonSpinnerSize::Custom(8.0))
                            .tone(MoonTone::Warning),
                    )
                })
                .child(
                    MoonBadge::new(
                        t!(if waiting {
                            "core_expert.core_requesting"
                        } else {
                            "core_expert.core_offline"
                        })
                        .to_string(),
                    )
                    .size(MoonBadgeSize::Tier(MoonSize::Xs))
                    .mono(false)
                    .weight(500.0)
                    .text_color(if waiting { p.amber } else { p.text_muted })
                    .bg_color(p.panel_high),
                ),
        );
    }
    div()
        .debug_selector(move || format!("expert-row-{core}"))
        .flex_none()
        .w_full()
        .h(design::ui_px(cx, 30.0))
        .child(
            rail_row_base(("core-expert-core", core), selected, true, 10.0, 7.0, p, cx)
                .relative()
                .pl(design::ui_px(cx, 22.0))
                .when(anchor, |row| {
                    row.child(
                        div()
                            .absolute()
                            .left_0()
                            .top(design::ui_px(cx, 4.0))
                            .bottom(design::ui_px(cx, 4.0))
                            .w(design::ui_px(cx, 3.0))
                            .bg(rgb(p.accent)),
                    )
                })
                .child(content)
                .tooltip(move |_, cx| cx.new(|_| MoonTooltipView::new(tooltip.clone())).into())
                .on_click(move |event, _, app| {
                    action(RailAction::Core(core, event.modifiers()), app)
                }),
        )
}

#[cfg(test)]
mod tests;
