//! Independent Classic/shared-Auto dock layouts, all-core navigation rail, and chart reveal.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::config::{
    AUTO_WORKSPACE_RAIL_WIDTH_MAX, AUTO_WORKSPACE_RAIL_WIDTH_MIN, WorkspaceMode,
};
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;
use moon_core::venue::CoreVenue;
use moon_ui::{
    DockTopologyByName, DockTopologyNode, MoonBackgroundPolicy, MoonBadge, MoonBadgeVariant,
    MoonPalette, MoonScrollbarVisibility, MoonTooltipView, MoonVirtualList, PanelView, h_flex,
    moon_h_resizable, moon_resizable_panel, v_flex,
};
use rust_i18n::t;

use super::Shell;
use crate::controls::core_run::{RunKey, RunScope, RunSlots, run_cell, run_cell_with_status};
use crate::workspace::{
    WorkspaceCoreStatus, WorkspaceNavigationAction, WorkspaceRailDensity, WorkspaceRosterInput,
    WorkspaceRosterRow,
};
use crate::{Backend, design};

/// Rank both status lists only when at least one list can change order.
pub(super) fn sort_status_lists<A, B>(
    down: &mut [A],
    report_sync: &mut [B],
    key_a: impl Fn(&A) -> CoreId,
    key_b: impl Fn(&B) -> CoreId,
    make_order: impl FnOnce() -> moon_core::session::core_order::CoreOrder,
) {
    if down.len() > 1 || report_sync.len() > 1 {
        let order = make_order();
        order.sort_by(down, key_a);
        order.sort_by(report_sync, key_b);
    }
}

/// Build and filter canonically ordered rail facts from explicit snapshot accessors.
fn rail_inputs<'a, 'b>(
    fleet: &'a [moon_core::config::ServerConfig],
    order: &moon_core::session::core_order::CoreOrder,
    live: impl IntoIterator<Item = (CoreId, &'a str)>,
    get_core: impl Fn(CoreId) -> Option<&'b moon_core::session::store::CoreData>,
    get_venue: impl Fn(&moon_core::config::ServerConfig) -> Option<CoreVenue>,
    availability: impl Fn(
        &str,
        Option<&moon_core::config::ServerConfig>,
        bool,
    ) -> crate::workspace::WorkspaceCoreAvailability,
) -> Vec<WorkspaceRosterInput> {
    let mut servers: Vec<&moon_core::config::ServerConfig> = fleet.iter().collect();
    order.sort_by(&mut servers, |server| server.id);
    let live = crate::backend::WorkspaceLiveIndex::new(fleet, live);
    let is_ready = |id| get_core(id).is_some_and(|core| core.status == ConnStatus::Ready);
    // Fed the WHOLE fleet: hidden cores still contribute transport evidence.
    let modes = crate::conn_diag::FleetModeIndex::new(fleet, is_ready);
    let mut inputs: Vec<_> = servers
        .iter()
        .map(|server| {
            let core = get_core(server.id);
            let resolved = live.server(&server.group, server.id);
            WorkspaceRosterInput {
                core: server.id,
                name: server.name.clone(),
                group: server.group.clone(),
                venue: get_venue(server),
                availability: availability(
                    &server.group,
                    resolved,
                    live.live(&server.group, server.id),
                ),
                ready: is_ready(server.id),
                connection: core.map(|core| core.status.clone()),
                startup: core.map(|core| core.startup).unwrap_or_default(),
                fault: core.and_then(|core| core.fault.clone()),
                mode_suggestion: modes.suggestion(server.id),
            }
        })
        .collect();
    inputs.retain(|input| live.displayed(Some(WorkspaceMode::AutoTrading), input.core));
    inputs
}

mod rail_item;
mod rail_model;
mod status;
mod topology;
mod window_sync;

use {rail_item::*, rail_model::*, status::*, topology::*};

pub(super) use topology::{
    auto_workspace_tab_to_persist, auto_workspace_topology_is_persistable, rehome_auto_panel,
    resolved_auto_workspace_tab,
};

impl Shell {
    /// Apply the global persisted Auto rail width to this Shell's live resize state.
    ///
    /// Args:
    ///     window: Owning window required by MoonUI's programmatic resize API.
    ///     cx: Shell context used to read Backend and update the resize entity.
    ///
    /// Returns:
    ///     Nothing; an equal live width, an in-flight drag, and an as-yet-unmeasured first render
    ///     are all no-ops.
    pub(super) fn sync_auto_rail_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A drag owns the width until the user lets go. This runs from the Backend observer, which
        // fires on every 10 Hz coordination tick, while `on_resize` writes the new width only on
        // mouse-up — so mid-drag the stored preference is still the OLD width and reconciling to it
        // would yank the rail back. Worse, `resize_panel_silently` clears the in-flight drag as it
        // applies, so the pointer would be left dragging nothing and mouse-up would never persist
        // the width. The reconcile is not skipped, only deferred: mouse-up publishes a revision
        // that brings this straight back with the width the user actually chose.
        if self.workspace_resize_state.read(cx).is_resizing() {
            return;
        }
        let preferred = self.backend.read(cx).auto_workspace_rail_width();
        self.applied_auto_rail_width = preferred;
        let scale = design::ui_value(cx, 1.0).max(0.01);
        let fitted =
            fitted_auto_rail_width(preferred, f32::from(window.viewport_size().width), scale);
        let Some(current) = self
            .workspace_resize_state
            .read(cx)
            .sizes()
            .first()
            .map(|width| width.as_f32() / scale)
        else {
            return;
        };
        if (current - fitted).abs() < 0.5 {
            return;
        }
        self.workspace_resize_state.update(cx, |state, state_cx| {
            state.resize_panel_silently(0, design::ui_px(state_cx, fitted), window, state_cx);
        });
    }

    /// Transform the single live DockArea between independent Classic and shared Auto layouts.
    ///
    /// Args:
    ///     mode: Persisted desired workspace mode for this group.
    ///     window: Owning window required for panel activation synchronization.
    ///     cx: Shell context used for the dock update.
    ///
    /// Returns:
    ///     Nothing; local panel identities survive both name-based transformations.
    pub(super) fn apply_workspace_mode(
        &mut self,
        mode: WorkspaceMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mode == self.applied_workspace_mode {
            return;
        }
        match mode {
            WorkspaceMode::AutoTrading => {
                self.start_exchange_logo_prewarm(cx);
                let classic = self.dock.read(cx).named_layout(cx);
                let classic_panel_names = classic.panel_names();
                let detached_names = auto_only_detached_panel_names(
                    &self.group,
                    &classic_panel_names,
                    &self.backend.read(cx).detached,
                );
                let mut auto_only_panels = detached_names
                    .iter()
                    .filter_map(|name| {
                        crate::window::detached::build_panel(
                            name,
                            &self.group,
                            &self.backend,
                            window,
                            cx,
                        )
                    })
                    .collect::<Vec<_>>();
                let topology = self
                    .backend
                    .read(cx)
                    .auto_dock_topology()
                    .cloned()
                    .unwrap_or_else(default_auto_workspace_topology);
                self.push_missing_auto_topology_panels(
                    &topology,
                    &mut auto_only_panels,
                    window,
                    cx,
                );
                let active_panel = {
                    let backend = self.backend.read(cx);
                    resolved_auto_workspace_tab(backend.auto_workspace_tab(&self.group)).to_string()
                };
                let guard_generation = self.begin_auto_topology_application();
                let classic_only_panels = self.dock.update(cx, |dock, dock_cx| {
                    let classic_only_panels = auto_classic_only_panel_names()
                        .iter()
                        .filter_map(|panel_name| {
                            dock.take_panel_by_name(panel_name, window, dock_cx)
                        })
                        .collect::<Vec<_>>();
                    dock.set_layout_editable(true, dock_cx);
                    dock.set_detach_allowed(false, dock_cx);
                    dock.set_close_allowed(true, dock_cx);
                    dock.apply_topology_by_name(
                        &topology,
                        auto_only_panels.clone(),
                        window,
                        dock_cx,
                    );
                    dock.set_pinned_leading_panels(vec!["ChartTabs".into()], dock_cx);
                    let activated = dock.activate_panel_by_name(&active_panel, window, dock_cx);
                    if let Some(fallback) = auto_workspace_activation_fallback(activated) {
                        dock.activate_panel_by_name(fallback, window, dock_cx);
                    }
                    classic_only_panels
                });
                let actual = self.dock.read(cx).topology_by_name(cx);
                self.backend.update(cx, |backend, backend_cx| {
                    backend.reconcile_auto_dock_topology(actual, backend_cx);
                });
                let group = self.group.clone();
                let suspended = self.backend.update(cx, |backend, _| {
                    crate::window::detached::take_windows(backend, |owner| owner == group.as_str())
                });
                crate::window::windowing::close_all(suspended, cx);
                self.classic_dock_layout = Some(classic);
                self.classic_only_panels = classic_only_panels;
                self.auto_only_panels = auto_only_panels;
                self.header_core_selector_open = false;
                self.applied_workspace_mode = WorkspaceMode::AutoTrading;
                self.finish_auto_topology_application(guard_generation, cx);
            }
            WorkspaceMode::Classic => {
                let classic = self.classic_dock_layout.take();
                self.dock.update(cx, |dock, dock_cx| {
                    dock.set_pinned_leading_panels(Vec::new(), dock_cx);
                    dock.set_detach_allowed(true, dock_cx);
                    dock.set_close_allowed(true, dock_cx);
                    dock.set_layout_editable(true, dock_cx);
                    if let Some(classic) = classic.as_ref() {
                        dock.apply_named_layout(
                            classic,
                            self.classic_only_panels.clone(),
                            window,
                            dock_cx,
                        );
                    }
                });
                self.classic_only_panels.clear();
                self.auto_only_panels.clear();
                self.applied_workspace_mode = WorkspaceMode::Classic;
                crate::window::detached::respawn_all(&self.backend, cx);
            }
        }
        cx.notify();
    }

    /// Re-home a user-closed Auto panel through the user-driven topology setter.
    ///
    /// `set_auto_dock_topology` unlocks an authority locked by an unreadable `auto_dock.json`
    /// as well as marking it dirty; a close click is a user edit, so it must not go through
    /// `reconcile_auto_dock_topology`.
    ///
    /// Args:
    ///     panel_name: Stable panel name carried by `DockEvent::PanelCloseRequested`.
    ///     cx: Shell context used to read and write the shared Auto topology authority.
    ///
    /// Returns:
    ///     Nothing; Classic mode, ineligible names, and an already-home panel are no-ops.
    pub(super) fn rehome_auto_panel_from_user(&mut self, panel_name: &str, cx: &mut Context<Self>) {
        if self.applied_workspace_mode != WorkspaceMode::AutoTrading {
            return;
        }
        let topology = self
            .backend
            .read(cx)
            .auto_dock_topology()
            .cloned()
            .unwrap_or_else(default_auto_workspace_topology);
        let next = rehome_auto_panel(topology.clone(), panel_name);
        if next != topology {
            self.backend.update(cx, |backend, backend_cx| {
                backend.set_auto_dock_topology(next, backend_cx);
            });
        }
    }

    /// Apply the latest shared Auto topology to this Shell's local panel instances.
    ///
    /// Args:
    ///     reveal: Unseen Auto surface to make present before the caller activates it.
    ///     window: Owning group window required by DockArea synchronization.
    ///     cx: Shell context used to read the authority and update the dock.
    ///
    /// Returns:
    ///     Nothing; Classic Shells ignore Auto-layout broadcasts. A reveal that names a panel
    ///     missing from the saved topology inserts that name once and builds a live instance when
    ///     the dock does not already own one, so activation is not a discarded no-op.
    fn sync_auto_dock_topology(
        &mut self,
        reveal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.applied_workspace_mode != WorkspaceMode::AutoTrading {
            return;
        }
        let Some(original) = self.backend.read(cx).auto_dock_topology().cloned() else {
            return;
        };
        let mut topology = original.clone();
        let mut extra = self.auto_only_panels.clone();
        if let Some(panel_name) = reveal {
            ensure_auto_topology_contains_panel(&mut topology, panel_name);
        }
        self.push_missing_auto_topology_panels(&topology, &mut extra, window, cx);
        let guard_generation = self.begin_auto_topology_application();
        self.dock.update(cx, |dock, dock_cx| {
            dock.apply_topology_by_name(&topology, extra, window, dock_cx);
        });
        let repaired = self.dock.read(cx).topology_by_name(cx);
        if repaired != original {
            self.backend.update(cx, |backend, backend_cx| {
                backend.reconcile_auto_dock_topology(repaired, backend_cx);
            });
        }
        self.finish_auto_topology_application(guard_generation, cx);
    }

    /// Build Auto-eligible panels named by `topology` but missing from this dock.
    ///
    /// MoonUI discards unknown topology names, so a saved or just-injected Assets surface cannot
    /// activate until a live identity exists. Only [`AUTO_PANEL_ORDER`] names are created; Classic-only
    /// surfaces stay suspended.
    fn push_missing_auto_topology_panels(
        &self,
        topology: &DockTopologyByName,
        extra: &mut Vec<Rc<dyn PanelView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let live = self.dock.read(cx).topology_by_name(cx).panel_names();
        for name in topology.panel_names() {
            if !AUTO_PANEL_ORDER.contains(&name.as_str()) {
                continue;
            }
            if live.iter().any(|existing| existing == &name) {
                continue;
            }
            if extra
                .iter()
                .any(|panel| panel.panel_name(cx).as_ref() == name)
            {
                continue;
            }
            if let Some(panel) =
                crate::window::detached::build_panel(&name, &self.group, &self.backend, window, cx)
            {
                extra.push(panel);
            }
        }
    }

    /// Open or close the rail's ⚙ popup.
    ///
    /// The already-closed guard mirrors the quiet popup's: `Popover` reports `false` twice when the
    /// trigger is clicked while open, and the second report would cost a repaint for nothing.
    ///
    /// Args:
    ///     open: Whether the popup should be showing.
    ///     cx: Shell context used to repaint.
    pub(super) fn set_rail_settings_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.rail_settings_open == open {
            return;
        }
        self.rail_settings_open = open;
        cx.notify();
    }

    /// Render the current workspace body: unchanged Classic dock or Auto rail plus the same dock.
    ///
    /// Args:
    ///     chrome_width: Current rendered window width used by the responsive rail policy.
    ///     p: Active Moon palette.
    ///     cx: Shell context used to derive the live all-config roster.
    ///
    /// Returns:
    ///     Complete body element between toolbar and status bar.
    pub(super) fn workspace_body(
        &self,
        chrome_width: f32,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.applied_workspace_mode == WorkspaceMode::Classic {
            return dock_host(self.dock.clone()).into_any_element();
        }

        let scale = design::ui_value(cx, 1.0).max(0.01);
        let state_id = ElementId::from(SharedString::from(format!(
            "workspace-resizable-{}",
            self.group
        )));
        let resize_state = self.workspace_resize_state.clone();
        let rail_width = fitted_auto_rail_width(self.applied_auto_rail_width, chrome_width, scale);
        let density = crate::workspace::workspace_rail_density(rail_width);
        let max_rail_width =
            fitted_auto_rail_width(AUTO_WORKSPACE_RAIL_WIDTH_MAX, chrome_width, scale);
        let backend = self.backend.clone();
        moon_h_resizable(state_id)
            .with_state(&resize_state)
            .on_resize(move |state, _, cx| {
                let scale = design::ui_value(cx, 1.0).max(0.01);
                let Some(width) = state
                    .read(cx)
                    .sizes()
                    .first()
                    .map(|width| width.as_f32() / scale)
                else {
                    return;
                };
                backend.update(cx, |backend, backend_cx| {
                    backend.set_auto_workspace_rail_width(width, backend_cx);
                });
            })
            .child(
                moon_resizable_panel()
                    .size(design::ui_px(cx, rail_width))
                    .size_range(
                        design::ui_px(cx, AUTO_WORKSPACE_RAIL_WIDTH_MIN)
                            ..design::ui_px(cx, max_rail_width),
                    )
                    .flex_none()
                    .child(self.workspace_rail(density, p, cx)),
            )
            .child(moon_resizable_panel().child(dock_host(self.dock.clone())))
            .into_any_element()
    }

    /// Render the application-wide virtualized core rail for one Auto group window.
    ///
    /// Args:
    ///     density: Full, compact, or icon rung chosen from the body width.
    ///     p: Active Moon palette.
    ///     cx: Shell context used to read configured and live core state.
    ///
    /// Returns:
    ///     Full-width rail panel with summary, current-group Overview, and exchange-grouped cores.
    fn workspace_rail(
        &self,
        density: WorkspaceRailDensity,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (roster, prefs) = {
            let backend = self.backend.read(cx);
            let order = moon_core::session::core_order::CoreOrder::new(&backend.config);
            let venues = backend.session.core_venues();
            let store = backend.session.store();
            let configured_total = backend.config.servers.len();
            let inputs = rail_inputs(
                &backend.config.servers,
                &order,
                backend
                    .session
                    .sessions()
                    .iter()
                    .map(|session| (session.id, session.group.as_str())),
                |id| store.core(id),
                |server| venues.get(&server.id).cloned(),
                |group, server, live| {
                    backend.workspace_core_availability_resolved(group, server, live)
                },
            );
            let roster = crate::workspace::derive_workspace_roster(
                &inputs,
                &self.group,
                backend.valid_auto_workspace_core(&self.group),
                configured_total,
            );
            let prefs = rail_settings::RailPrefs::restore(&backend.layout);
            (roster, prefs)
        };
        let slots = prefs.slots(density);
        let run = RailRun {
            slots,
            exchange_controls: prefs.exchange_controls,
        };

        let mut items = vec![RailItem::Overview {
            selected: roster.overview_selected,
        }];
        for (section_index, section) in roster.sections.into_iter().enumerate() {
            let logo = if self.exchange_logos_ready {
                section
                    .venue
                    .as_ref()
                    .and_then(|venue| venue.brand())
                    .and_then(crate::media::exchange_logos::exchange_logo)
            } else {
                None
            };
            let cores: Rc<[CoreId]> = section.rows.iter().map(|row| row.core).collect();
            items.push(RailItem::Exchange {
                venue: section.venue,
                logo,
                section: section_index,
                cores,
            });
            append_core_section_items(&mut items, section.rows);
        }
        let item_count = items.len();
        let row_height = design::ui_value(cx, 30.0);
        let backend = self.backend.clone();
        let current_group = self.group.clone();
        let rail = MoonVirtualList::new(
            format!("workspace-rail-{}", self.group),
            item_count,
            row_height,
            move |index, _, app| {
                items
                    .get(index)
                    .cloned()
                    .map(|item| {
                        render_rail_item(
                            item,
                            density,
                            run,
                            p,
                            backend.clone(),
                            current_group.clone(),
                            app,
                        )
                    })
                    .unwrap_or_else(|| div().into_any_element())
            },
        )
        .surface(false)
        .background_policy(MoonBackgroundPolicy::NoFill)
        .border(false)
        .radius(0.0)
        .scrollbar_visibility(MoonScrollbarVisibility::Hover);

        let mut summary = t!(
            "workspace.summary",
            configured = roster.summary.configured,
            ready = roster.summary.ready,
            problem = roster.summary.problem
        )
        .to_string();
        // The rail's own scope marker: its viewing preset is always Auto, since it only ever
        // renders inside an Auto window (frozen contract §5, the H3 rule's concrete instance).
        let marker = crate::workspace::scope_marker::ScopeMarker::new(
            Some(WorkspaceMode::AutoTrading),
            roster.summary.configured,
            roster.summary.configured_total,
        );
        let marker_facts = marker.facts();
        for fact in &marker_facts {
            summary.push(' ');
            summary.push_str(fact);
        }
        let has_problem = roster.summary.problem > 0;
        let problem_color = if has_problem {
            design::danger_color(p)
        } else {
            p.text_muted
        };
        let marker_tail =
            (!marker_facts.is_empty()).then(|| format!(" {}", marker_facts.join(" ")));
        // Rendered as a LIST of segments that the summary's centered wrapper below takes as its
        // direct children, never as a nested flex row of their own: a nested row measured its
        // truncating children at zero and collapsed the whole line to "...problems..." at full
        // rail width (seen live 2026-09-05). The wrapper's row lays the segments out side by side
        // at their content size, and the shrink priority applies only when it is out of room.
        let summary_content: Vec<AnyElement> = match density {
            WorkspaceRailDensity::Icon => {
                let icon_color = if has_problem {
                    design::danger_color(p)
                } else {
                    p.text_soft
                };
                vec![
                    div()
                        .min_w_0()
                        .truncate()
                        .text_center()
                        .text_color(rgb(icon_color))
                        .child(icon_workspace_summary(roster.summary.configured))
                        .into_any_element(),
                ]
            }
            WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => {
                let cores_ready = format!(
                    "{}{SUMMARY_SEP}{}{SUMMARY_SEP}",
                    t!("workspace.summary_cores", n = roster.summary.configured),
                    t!("workspace.summary_ready", n = roster.summary.ready)
                );
                let problem_seg =
                    t!("workspace.summary_problem", n = roster.summary.problem).to_string();
                let mut segments = vec![
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(p.text_muted))
                        .child(cores_ready)
                        .into_any_element(),
                    // `RAIL_ALARM_SHRINK` keeps this segment's flex-shrink small against
                    // `cores_ready`'s default 1.0, so `cores_ready` absorbs essentially all of
                    // the shrink first and the alarm stays fully visible until the row is
                    // genuinely out of room. It still shrinks a little rather than 0, so the
                    // bar's `overflow_hidden()` never hard-clips it without an ellipsis in that
                    // last resort. The full text is always in the bar's own tooltip.
                    div()
                        .min_w_0()
                        .truncate()
                        .flex_shrink(design::RAIL_ALARM_SHRINK)
                        .text_color(rgb(problem_color))
                        .child(problem_seg)
                        .into_any_element(),
                ];
                if let Some(tail) = marker_tail.clone() {
                    segments.push(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(rgb(p.text_muted))
                            .child(tail)
                            .into_any_element(),
                    );
                }
                segments
            }
        };
        // The closing hint line lives in the tooltip only — Icon density's bare count never gets
        // it either, since both densities share this one tooltip.
        let summary = if marker_facts.is_empty() {
            summary
        } else {
            marker.tooltip(std::slice::from_ref(&summary))
        };
        let gear = match density {
            // 52 px holds the count and nothing else; the preferences only govern the dot there anyway.
            WorkspaceRailDensity::Icon => None,
            WorkspaceRailDensity::Full | WorkspaceRailDensity::Compact => {
                Some(rail_settings::rail_settings_popover(
                    &cx.entity(),
                    &self.backend,
                    self.rail_settings_open,
                    prefs,
                    p,
                    cx,
                ))
            }
        };
        v_flex()
            .size_full()
            .h_full()
            .w_full()
            .min_h_0()
            // The rail reads as its own recessed surface rather than a strip of the window, the way
            // a desktop navigation pane does. `gutter` is the recessed side-strip token and is the
            // one that steps AWAY from the chrome and toolbar above (both `shell_high`) in the same
            // direction in either theme, so the separation survives a runtime theme switch. The
            // divider uses the stronger `border_hover` token, matching the existing chrome-boundary
            // convention without turning the rail into a framed card.
            .border_r_1()
            .border_color(rgb(p.border_hover))
            .bg(rgb(p.gutter))
            .child(
                div()
                    .flex_none()
                    .h(design::fit_h_px(cx, 38.0, 11.0, 8.0))
                    .overflow_hidden()
                    .px(design::ui_px(cx, 8.0))
                    .flex()
                    .items_center()
                    .gap(design::ui_px(cx, 4.0))
                    .min_w_0()
                    .text_size(design::t_caption(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .border_b_1()
                    .border_color(rgb(p.border_soft))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "workspace-summary-{}",
                                self.group
                            )))
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .justify_center()
                                    .children(summary_content),
                            )
                            .tooltip(move |_window, cx| {
                                cx.new(|_| MoonTooltipView::new(summary.clone())).into()
                            }),
                    )
                    .children(gear),
            )
            .child(div().flex_1().min_h_0().child(rail))
            .into_any_element()
    }
}

mod rail_settings;

#[cfg(test)]
mod tests;
