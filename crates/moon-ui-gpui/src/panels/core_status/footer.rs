//! Core Status counters and fleet update controls.

use super::model::ServerStatusGroup;
use super::{CoreStatusView, interactions};
use crate::design;
use crate::workspace::scope_marker::{self, ScopeMarker};
use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonButton, MoonButtonVariant, MoonPalette, h_flex};
use rust_i18n::t;

impl CoreStatusView {
    /// Render server, core, and readiness counters.
    ///
    /// Args:
    ///     groups: Current server snapshots.
    ///     total_cores: Cores in the current selector scope.
    ///     marker: This panel's scope marker, clipped onto the tail when the active preset hides
    ///         at least one core.
    ///     cx: View context used for palette and localization.
    ///
    /// Returns:
    ///     Compact footer shared by grouped and flat presentations.
    pub(super) fn footer(
        &self,
        groups: &[ServerStatusGroup],
        total_cores: usize,
        marker: &ScopeMarker,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let body = design::t_body(cx);
        let online = groups
            .iter()
            .filter(|group| group.ready_count == group.cores.len() && !group.cores.is_empty())
            .count();
        // Fleets run 3 to 200 cores, so the campaign totals must be readable WITHOUT expanding a
        // single server -- drawn only while a campaign is actually on, never a permanent "0
        // updating" fixture. `done` and `lanes_stalled` are deliberately excluded: this is the
        // fleet's ATTENTION state, and a finished or held-but-not-failed row needs none.
        let summary = self.backend.read(cx).session.core_update_summary();
        let update_parts: Vec<String> = [
            (summary.updating, "core_update.summary.updating"),
            (summary.queued, "core_update.summary.queued"),
            (summary.failed, "core_update.summary.failed"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, key)| format!("{n} {}", t!(key)))
        .collect();
        // Fleet-relative, not release-relative -- see `session::core_update`'s own doc comment.
        // Empty here means every core already agrees with the fleet's newest build, never that no
        // release exists, so the button explains that in its tooltip instead of just going gray.
        // Read off the PANEL's own plan, not off the fleet. Both of these used to ask the store
        // directly, so a panel scoped to one group lit a button that reached every core in the
        // fleet -- the state a control shows and the set it acts on have to be the same set.
        // Resolved ONCE: `visible_order` re-sorts the flat rows and re-groups the exchange
        // sections on every call, and this footer used to ask for it twice per repaint.
        let visible = self.visible_cores(cx);
        // The VISIBLE part of the selection, not the raw set: collapsing a By-IP group hides
        // its cores without rebuilding the cache, and a button that says 'update selected (5)'
        // while its own confirm would name none is worse than one that says nothing.
        let selected = self.visible_selected(&visible);
        let all_empty = self.fleet_update_plan(false, &visible, cx).cores.is_empty();
        let behind_empty = self.fleet_update_plan(true, &visible, cx).cores.is_empty();
        // TINT, not a second affordance: both buttons keep their handlers, their size and their
        // place. Amber is the tone this row ALREADY uses for "an update campaign wants attention"
        // (the counter above), so a fleet with something to update lights the control that acts on
        // it, and a fleet that is fully current leaves it as ordinary panel chrome.
        //
        // ONLY the BEHIND button is tinted, and that asymmetry is the point. These two controls
        // command LIVE cores and sit side by side: "update behind" is scoped to the cores that are
        // actually stale, while "update all" pushes to every eligible core in the fleet. Lighting
        // both under one predicate would put the attention cue on the broader, more destructive
        // action at the exact moment a user is scanning for "the lit-up button" -- so the tint
        // follows the action that RESOLVES the condition it signals, and the wider one stays
        // deliberately quiet. Tinting "update all" on its own offerability was rejected for a
        // second reason too: it is offerable on a healthy fleet at all times, so it would be a
        // permanent amber fixture, which this footer's own comment above forbids.
        let behind_variant = if behind_empty {
            MoonButtonVariant::Panel
        } else {
            MoonButtonVariant::Amber
        };
        let update_all_view = cx.entity();
        let update_behind_view = update_all_view.clone();
        let update_named_view = update_all_view.clone();
        // Frozen render idiom (`workspace/scope_marker.rs`): head and tail are DIRECT children of
        // the row, never nested in a shared box, and the tail is the ONE part of this row allowed
        // to clip.
        //
        // The tail occupies the slot this row's bare `flex_1` spacer used to hold, and exactly one
        // of the two is ever drawn -- so with nothing hidden the row keeps the child count, the
        // gaps and the shrink behaviour it had before this feature existed, which is the "nothing
        // hidden looks identical" criterion held structurally rather than by inspection.
        //
        // The wrapping campaign group further right keeps WRAPPING while this tail CLIPS. Both are
        // right for what they carry -- a clipped button is unusable, a wrapped figure row is not a
        // row -- and the height of this row already varied with that wrap before this change. Do
        // not "unify" them.
        let head_text = t!(
            "core_status.footer",
            servers = groups.len(),
            cores = total_cores,
            online = online
        )
        .to_string();
        let footer_split = scope_marker::scope_footer(head_text, Some(marker));
        let tip = scope_marker::scope_footer_tooltip(&footer_split, Some(marker));
        let has_tail = !footer_split.tail.is_empty();
        crate::panels::footer_row(cx)
            .child(
                // `flex_none` only while a tail exists to yield in its place — see the same
                // gate in `panels/orders/render.rs`.
                crate::panels::footer_value(footer_split.head, p, cx)
                    .when(has_tail, |el| el.flex_none()),
            )
            .children(scope_marker::scope_footer_tail(
                "core-status-footer-tail",
                footer_split.tail,
                tip,
                body,
                p.text_soft,
            ))
            // Exactly one of these two holds the slot: the tail when a marker is on screen, this
            // bare spacer otherwise.
            .children((!has_tail).then(|| div().flex_1()))
            // A campaign's summary plus both bulk buttons, as ONE group that may shrink and wrap
            // onto its own row rather than clip: up to three localized counters joined with the
            // fleet summary and two localized buttons on a single non-shrinking row overruns a
            // narrow dock. `min_w_0` is what makes the wrap reachable -- only a group allowed to
            // shrink below its content wraps rather than overflowing -- following the same idiom
            // `panels/report/controls.rs::selection_actions` already uses for its own count-plus-
            // commands group.
            .child(
                h_flex()
                    .min_w_0()
                    .flex_wrap()
                    .justify_end()
                    .items_center()
                    .gap_2()
                    .when(!update_parts.is_empty(), |row| {
                        row.child(
                            crate::panels::footer_text_style(
                                div(),
                                crate::panels::FooterWeight::Regular,
                                p.amber,
                                cx,
                            )
                            .flex_none()
                            .child(update_parts.join(" - ")),
                        )
                    })
                    .child(
                        MoonButton::new("core-status-update-all")
                            // ONE button that RENAMES, never a fourth beside the other three:
                            // with a selection on screen the wide action IS the selection, and
                            // saying so on the control the operator is about to press beats
                            // adding a second control that means almost the same thing.
                            .label(if selected > 0 {
                                t!("core_update.fleet.selected", n = selected).to_string()
                            } else {
                                t!("core_update.fleet.all").to_string()
                            })
                            .variant(MoonButtonVariant::Panel)
                            .disabled(all_empty)
                            .on_click(move |_, window, cx| {
                                update_all_view.update(cx, |this, cx| {
                                    this.confirm_fleet_update(
                                        interactions::FleetUpdateKind::All,
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .render(),
                    )
                    .child({
                        let behind_button = MoonButton::new("core-status-update-behind")
                            .label(t!("core_update.fleet.behind").to_string())
                            .variant(behind_variant)
                            .disabled(behind_empty)
                            .on_click(move |_, window, cx| {
                                update_behind_view.update(cx, |this, cx| {
                                    this.confirm_fleet_update(
                                        interactions::FleetUpdateKind::Behind,
                                        window,
                                        cx,
                                    );
                                });
                            });
                        if behind_empty {
                            behind_button.tooltip(t!("core_update.fleet.behind_none").to_string())
                        } else {
                            behind_button
                        }
                        .render()
                    })
                    .child(
                        // The named build at panel scope. It reuses the SAME confirm the two
                        // buttons beside it open, with the build-name field inside it -- a
                        // prompt and then a confirm would be two gates on one action.
                        MoonButton::new("core-status-update-named")
                            .label(t!("core_update.fleet.named").to_string())
                            .variant(MoonButtonVariant::Panel)
                            .disabled(all_empty)
                            .on_click(move |_, window, cx| {
                                update_named_view.update(cx, |this, cx| {
                                    this.confirm_fleet_update(
                                        interactions::FleetUpdateKind::Named,
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .render(),
                    ),
            )
    }
}
