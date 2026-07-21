//! Flat drag-and-drop editor for `CoreSortMode::Manual`.
//!
//! The grouped connection tree cannot express a global order across groups, so this editor
//! presents the raw `AppConfig::servers` order through `MoonTree`'s typed drag-and-drop API.

use gpui::*;
use moon_ui::{MoonPalette, MoonTree, MoonTreeItem, h_flex, v_flex};
use rust_i18n::t;

use moon_core::config::CoreSortMode;
use moon_core::feed::ConnStatus;
use moon_core::session::CoreId;

use super::super::SettingsView;
use crate::design::{self, moon, moon_alpha};

/// The dragged core. Typed payload, so a foreign drag cannot drop onto these rows.
#[derive(Clone)]
pub(in crate::settings) struct CoreDrag(pub(in crate::settings) CoreId);

/// Chip rendered under the cursor while dragging.
pub(in crate::settings) struct CoreDragChip {
    label: SharedString,
}

impl Render for CoreDragChip {
    /// Render the dragged core label with connection-editor styling.
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        div()
            .px_2()
            .py_1()
            .rounded(design::r_button(cx))
            .bg(moon(p.shell_high))
            .border_1()
            .border_color(moon(p.blue))
            .text_color(moon(p.text))
            .text_size(design::t_body(cx))
            .font_family(design::mono())
            .child(self.label.clone())
    }
}

/// One editor row: the core id plus the labels and state needed to render it.
struct OrderRow {
    id: CoreId,
    name: String,
    group: String,
    active: bool,
    status: Option<ConnStatus>,
}

impl SettingsView {
    /// Snapshot of the draft's cores in the order this editor edits.
    fn order_rows(&self, cx: &App) -> Vec<OrderRow> {
        let b = self.backend.read(cx);
        let status = b.session.status_map();
        let d = b.preview.as_ref().unwrap_or(&b.config);
        // Show the raw Vec position because dragging rewrites that exact order.
        d.servers
            .iter()
            .map(|s| OrderRow {
                id: s.id,
                name: s.name.clone(),
                group: s.group.clone(),
                active: s.active,
                status: status.get(&s.id).cloned(),
            })
            .collect()
    }

    /// Move `dragged` to `target`'s original numeric slot in the draft, without swapping.
    ///
    /// Rebuilds connection rows because their edit closures capture row indices by value.
    fn move_core(
        &mut self,
        dragged: CoreId,
        target: CoreId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dragged == target {
            return;
        }
        self.backend.update(cx, |b, bcx| {
            if let Some(p) = b.preview.as_mut() {
                let (Some(from), Some(to)) = (
                    p.servers.iter().position(|s| s.id == dragged),
                    p.servers.iter().position(|s| s.id == target),
                ) else {
                    return;
                };
                reorder(&mut p.servers, from, to);
                bcx.notify();
            }
        });
        // Rebuild closures after row indices shift.
        let rows = super::build_conn(&self.backend, window, cx);
        self.conn = rows;
        cx.notify();
    }

    /// Render the manual-order section only when a draft exists in `Manual` mode.
    pub(in crate::settings) fn core_order_editor(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (manual, has_draft) = {
            let b = self.backend.read(cx);
            let d = b.preview.as_ref().unwrap_or(&b.config);
            (d.core_sort == CoreSortMode::Manual, b.preview.is_some())
        };
        // Hide the interactive editor when there is no writable draft.
        if !manual || !has_draft {
            return None;
        }
        let rows = self.order_rows(cx);
        if rows.len() < 2 {
            // Fewer than two cores — nothing to reorder.
            return None;
        }

        let p = MoonPalette::active(cx);
        let items: Vec<MoonTreeItem> = rows
            .iter()
            .map(|r| MoonTreeItem::new(SharedString::from(format!("core-order-{}", r.id)), ""))
            .collect();
        self.order_tree.update(cx, |st, c| st.set_items(items, c));

        let by_id: std::rc::Rc<Vec<(CoreId, String, String, bool, Option<ConnStatus>)>> =
            std::rc::Rc::new(
                rows.into_iter()
                    .map(|r| (r.id, r.name, r.group, r.active, r.status))
                    .collect(),
            );
        // WEAK, never a strong `cx.entity()`. `MoonTree` stores its row decorators in the
        // long-lived `MoonTreeState` (`tree.rs`, beside `expanded_ids`), and that state is a
        // field of this view — so a strong handle inside a decorator closes the cycle
        // `SettingsView -> order_tree -> decorator -> SettingsView`. The view then never drops,
        // its `on_release` never runs, and `preview` / `settings_window` stay set: the Settings
        // window silently refuses to reopen (`settings::open` bails on `preview.is_some()`).
        let view = cx.entity().downgrade();

        let render_data = by_id.clone();
        let tree = MoonTree::custom(&self.order_tree, move |entry, _meta, _window, app| {
            let pal = MoonPalette::active(app);
            let Some(idx) = core_of(&render_data, entry.item().id()) else {
                return div().into_any_element();
            };
            let (_, name, group, active, status) = render_data[idx].clone();
            // Reuse connection-table status semantics without moving disconnected cores.
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .px(design::ui_px(app, 6.0))
                .child(super::table::status_dot(idx, active, status.as_ref(), pal))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if active {
                            moon(pal.text)
                        } else {
                            moon(pal.text_muted)
                        })
                        .child(name),
                )
                // Truncate the secondary group label before the core name on narrow windows.
                .child(
                    div()
                        .flex_none()
                        .min_w_0()
                        .truncate()
                        .max_w(px(design::font_w(app, 120.0)))
                        .text_color(moon(pal.text_soft))
                        .text_size(design::t_caption(app))
                        .child(group),
                )
                .into_any_element()
        })
        .draggable::<CoreDrag, CoreDragChip, _, _>(
            {
                let data = by_id.clone();
                move |entry, _meta| core_of(&data, entry.item().id()).map(|i| CoreDrag(data[i].0))
            },
            {
                let data = by_id.clone();
                move |drag: &CoreDrag, _pos, _window, app| {
                    // Every draggable row has a name, so no untranslated placeholder is needed.
                    let label = data
                        .iter()
                        .find(|(id, ..)| *id == drag.0)
                        .map(|(_, n, ..)| n.clone())
                        .unwrap_or_default();
                    app.new(|_| CoreDragChip {
                        label: SharedString::from(label),
                    })
                }
            },
        )
        // Typed builders preserve the target `TreeEntry` that resolves the drop.
        .drag_over::<CoreDrag, _>(|style, _entry, _meta, _drag, _w, app| {
            style.bg(moon_alpha(MoonPalette::active(app).blue, 0.22))
        })
        .drop_target::<CoreDrag, _, _>(
            |_entry, _meta, _drag, _w, _app| true,
            move |entry, _meta, drag: &CoreDrag, window, app| {
                let Some(target) = core_id_of(entry.item().id()) else {
                    return;
                };
                // The decorator outlives the window; a dangling handle just means the editor is
                // gone and the drop has nothing to apply to.
                let Some(view) = view.upgrade() else {
                    return;
                };
                let dragged = drag.0;
                view.update(app, |this, cx| this.move_core(dragged, target, window, cx));
            },
        );

        Some(
            v_flex()
                .w_full()
                .gap_1()
                .child(Self::hint_label(
                    "h-core-order",
                    t!("conn.order_heading").to_string(),
                    t!("conn.order_tip").to_string().into(),
                    p,
                ))
                .child(
                    div()
                        .w_full()
                        .max_h(px(260.0))
                        .border_1()
                        .border_color(moon(p.border))
                        .rounded(design::r_button(cx))
                        .child(tree),
                )
                .into_any_element(),
        )
    }
}

/// Remove the item at `from` and insert it at numeric index `to` in the shortened vector.
fn reorder<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from == to || from >= items.len() || to >= items.len() {
        return;
    }
    let row = items.remove(from);
    items.insert(to, row);
}

#[cfg(test)]
mod tests {
    //! Manual reorder behavior tests.

    use super::reorder;

    /// Apply `reorder` to a compact string fixture.
    fn moved(items: &[&str], from: usize, to: usize) -> Vec<String> {
        let mut v: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        reorder(&mut v, from, to);
        v
    }

    /// Protects `reorder`: applying `to - 1` to downward moves makes adjacent drops no-ops and
    /// prevents reaching the bottom slot.
    #[test]
    fn a_dropped_core_takes_the_target_index_in_both_directions() {
        // Adjacent downward.
        assert_eq!(moved(&["A", "B"], 0, 1), ["B", "A"]);
        // Downward across a gap.
        assert_eq!(moved(&["A", "B", "C", "D"], 1, 3), ["A", "C", "D", "B"]);
        // Upward across a gap.
        assert_eq!(moved(&["A", "B", "C", "D"], 3, 1), ["A", "D", "B", "C"]);
        // Penultimate to bottom.
        assert_eq!(moved(&["A", "B", "C"], 1, 2), ["A", "C", "B"]);
        // Dropping onto itself changes nothing.
        assert_eq!(moved(&["A", "B"], 1, 1), ["A", "B"]);
    }
}

/// `CoreId` behind a tree row id (`core-order-<id>`).
fn core_id_of(row_id: &str) -> Option<CoreId> {
    row_id.strip_prefix("core-order-")?.parse().ok()
}

/// Index of the core behind a tree row id.
fn core_of(
    data: &[(CoreId, String, String, bool, Option<ConnStatus>)],
    row_id: &str,
) -> Option<usize> {
    let id = core_id_of(row_id)?;
    data.iter().position(|(cid, ..)| *cid == id)
}
