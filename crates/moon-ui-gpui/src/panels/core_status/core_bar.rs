//! Core Status scope selector and presentation mode strip.

use std::collections::HashSet;

use super::mode::{MODE_DIVIDER_INDEX, MODE_DIVIDER_WIDTH};
use super::{CoreStatusMode, CoreStatusView};
use crate::design;
use gpui::*;
use moon_core::session::CoreId;
use moon_core::session::core_order::OrderedCores;
use moon_ui::{MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex};
use rust_i18n::t;

impl CoreStatusView {
    /// Render the effective core scope in the top bar.
    ///
    /// Classic mode exposes the retained multi-selector and exchange batch toggles. Auto mode
    /// renders a non-interactive pinned scope chip naming the workspace label instead, without
    /// changing retained Classic state.
    ///
    /// Args:
    ///     cores: Group cores in canonical display order.
    ///     cx: View context used to read exchanges and wire selection callbacks.
    ///
    /// Returns:
    ///     The top-bar row containing an interactive Classic selector or non-interactive pinned
    ///     scope chip.
    pub(super) fn core_bar(&self, cores: &OrderedCores, cx: &Context<Self>) -> impl IntoElement {
        let scope = self.effective_scope(self.backend.read(cx));
        let workspace_owned = scope.is_workspace_owned();
        let effective_selection: HashSet<CoreId> = scope.ids().iter().copied().collect();
        let pinned_label = match scope.label() {
            crate::workspace::EffectiveScopeLabel::Overview => {
                Some(t!("workspace.overview").to_string())
            }
            crate::workspace::EffectiveScopeLabel::Core(core) => cores
                .iter()
                .find(|(id, _)| *id == core)
                .map(|(_, name)| name.clone()),
            crate::workspace::EffectiveScopeLabel::All
            | crate::workspace::EffectiveScopeLabel::Selection(_) => None,
        };
        let selection = if workspace_owned {
            &effective_selection
        } else {
            &self.sel_cores
        };
        let combo: AnyElement = if workspace_owned {
            let p = MoonPalette::active(cx);
            let label = pinned_label.unwrap_or_else(|| {
                crate::controls::core_selection_summary(
                    cores,
                    selection,
                    crate::controls::CoreAllRowMode::ImplicitOrComplete,
                    t!("core_status.all_cores").as_ref(),
                    &|n| t!("core_status.cores_n", n = n).to_string(),
                )
                .label
            });
            let width = px(crate::controls::pinned_scope_width(
                cx,
                &label,
                crate::controls::CORE_COMBO_TRIGGER_W,
                crate::controls::PINNED_SCOPE_TRIGGER_MAX_W,
            ));
            crate::panels::pinned_scope_host(
                "core-status-core-tip",
                "core-status-core",
                label,
                width,
                p,
                cx,
            )
        } else {
            let view = cx.entity();
            let exchange_view = view.clone();
            let venues = self.backend.read(cx).session.core_venues();
            let extras =
                crate::controls::core_combo_extras(!workspace_owned, &view, &self.backend, cx);
            crate::controls::core_combo(
                "core-status-core",
                cores,
                venues,
                selection,
                crate::controls::CoreAllRowMode::ImplicitOrComplete,
                t!("core_status.all_cores").to_string(),
                |n| t!("core_status.cores_n", n = n).to_string(),
                170.0,
                extras,
                cx,
                move |id, app| {
                    view.update(app, |t, c| t.toggle_core(id, c));
                },
                move |exchange_cores, app| {
                    exchange_view.update(app, |t, c| {
                        t.toggle_exchange_cores(exchange_cores, c);
                    });
                },
            )
            .into_any_element()
        };
        let weak_view = cx.entity().downgrade();
        let mode_palette = MoonPalette::active(cx);
        // The five modes are two different KINDS of surface: By-IP, Flat and Problems are live
        // views of the fleet as it stands, while Warnings and Updates are history — records of what
        // already happened. Problems sits on the LIVE side because it is present tense: it lists
        // what the cores confirm right now, and a finding leaves it when a later list omits it.
        // `MODE_DIVIDER_INDEX` is a dead cell that draws the boundary between them.
        //
        // One control rather than two, deliberately: the selection is one value, so two controls
        // would each need to render "nothing selected" while the other holds it, and any drift
        // between them shows the user two highlighted tabs. A replaced cell "preserves its
        // resolved width and selected underline but exposes no segment click, scroll, hover,
        // cursor, or tooltip behavior" (MoonUI `segment.rs::replace_item`), which is exactly a
        // separator: it can never be selected and can never be clicked, so the index below it
        // simply never arrives.
        let modes = MoonSegmentedControl::new("core-status-mode")
            .items([
                MoonSegmentItem::new("", t!("core_status.mode.by_ip").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::ByIp),
                MoonSegmentItem::new("", t!("core_status.mode.flat").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Flat),
                // The count rides in the LABEL because `MoonSegmentItem` carries no badge slot, and
                // it has to be here as well as on the dock tab: the dock asks a hidden tab only, so
                // a Core Status sitting in front on another mode would announce nothing at all.
                MoonSegmentItem::new(
                    "",
                    match self.unseen_problems {
                        0 => t!("core_status.mode.problems").to_string(),
                        n => format!(
                            "{} {}",
                            t!("core_status.mode.problems"),
                            crate::panels::common::count_text(
                                n,
                                crate::panels::common::COUNT_BADGE_MAX
                            )
                        ),
                    },
                )
                .fit_width(cx, 54.0, 104.0)
                .selected(self.mode == CoreStatusMode::Problems),
                // Narrow by explicit width, not `fit_width`: a separator sized like a label cell
                // would open a 54-88px hole in the strip, which reads as a missing button rather
                // than a boundary.
                MoonSegmentItem::new("", String::new())
                    .width(MODE_DIVIDER_WIDTH)
                    .disabled(true),
                MoonSegmentItem::new("", t!("core_status.mode.warnings").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Warnings),
                MoonSegmentItem::new("", t!("core_status.mode.updates").to_string())
                    .fit_width(cx, 54.0, 88.0)
                    .selected(self.mode == CoreStatusMode::Updates),
            ])
            .replace_item(
                MODE_DIVIDER_INDEX,
                h_flex()
                    .w_full()
                    .h_full()
                    .justify_center()
                    .items_center()
                    .child(design::chrome_divider(cx, mode_palette)),
            )
            .on_click(move |index, _, _, app| {
                let Some(view) = weak_view.upgrade() else {
                    return;
                };
                let mode = match index {
                    0 => CoreStatusMode::ByIp,
                    1 => CoreStatusMode::Flat,
                    2 => CoreStatusMode::Problems,
                    // 3 is the separator and never reports a click.
                    4 => CoreStatusMode::Warnings,
                    _ => CoreStatusMode::Updates,
                };
                view.update(app, |this, cx| this.set_mode(mode, cx));
            });
        // The core selector yields first in a narrow side dock, keeping both mode actions
        // reachable without introducing a horizontal scroll host.
        h_flex()
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(div().flex_1().min_w_0().overflow_hidden().child(combo))
            .child(modes)
            .child(self.warn_gear(cx))
    }
}
