//! MoonTree callbacks helpers.

use super::*;

impl StrategiesView {
    /// Builds the headless `MoonTree` element using the current frame's `data` side map.
    pub(in crate::strategies::tree) fn moon_tree_el(
        &self,
        data: Rc<HashMap<SharedString, NodeData>>,
        cx: &Context<Self>,
    ) -> AnyElement {
        // WEAK only, never a strong `cx.entity()`. On every frame, `MoonTree` moves BOTH the row
        // renderer AND decorators into long-lived `MoonTreeState` (MoonUI `tree.rs`,
        // `impl RenderOnce for Tree`), while that state lives in THIS view. A strong handle in
        // either closure closes `StrategiesView -> tree_state -> closure -> StrategiesView`, so
        // the view and its subscriptions never drop. Both back-references must be weak; one is not
        // enough.
        let view = cx.entity().downgrade();
        // Captured once per build rather than re-read per drag chip: the chip only ever renders
        // for the duration of one drag gesture, so a mid-drag preference change is not a concern
        // the row renderer above has to guard against.
        let step = self.prefs.tree_text_step;
        let tree_field = self.tree_field_bounds.clone();
        let strat_field = tree_field.clone();
        let folder_field = tree_field;

        // ── Row rendering ──
        let row_data = data.clone();
        let row_view = view.clone();
        let tree = MoonTree::custom(&self.tree_state, move |entry, meta, _window, app| {
            // A dead handle means the view is gone; render an empty row.
            let Some(row_view) = row_view.upgrade() else {
                return div().into_any_element();
            };
            render_row(&row_data, &row_view, entry, meta, app)
        })
        // ── DnD: strategies ──
        // Custom decorator (not `Tree::draggable`) so the payload can capture the originating
        // window. MoonTree's value closure does not receive `Window`.
        .row_decorator({
            let data = data.clone();
            let strat_field = strat_field.clone();
            move |row, entry, _meta, window, _app| {
                let Some(NodeData::Strategy {
                    core, id, drag_ids, ..
                }) = data.get(entry.item().id())
                else {
                    return row;
                };
                let origin_window = window.window_handle().window_id();
                let payload = StratDrag {
                    core: *core,
                    ids: drag_ids
                        .as_ref()
                        .map_or_else(|| vec![*id], |ids| ids.to_vec()),
                    origin_window,
                };
                let tree_field = strat_field.clone();
                row.on_drag(payload, move |drag: &StratDrag, _pos, _window, app| {
                    let n = drag.ids.len();
                    let origin_window = drag.origin_window;
                    let tree_field = tree_field.clone();
                    app.new(move |_| DragChip {
                        label: SharedString::from(if n > 1 {
                            format!("{n}×")
                        } else {
                            "≡".to_string()
                        }),
                        step,
                        origin_window,
                        tree_field,
                        stop_when_outside: true,
                    })
                })
            }
        })
        // ── DnD: folders ──
        .draggable::<FolderDrag, DragChip, _, _>(
            {
                let data = data.clone();
                move |entry, _meta| match data.get(entry.item().id()) {
                    Some(NodeData::Folder {
                        core, path, label, ..
                    }) => {
                        let _ = label;
                        Some(FolderDrag {
                            core: *core,
                            path: path.clone(),
                        })
                    }
                    _ => None,
                }
            },
            move |_drag: &FolderDrag, _pos, window, app| {
                let origin_window = window.window_handle().window_id();
                let tree_field = folder_field.clone();
                app.new(move |_| DragChip {
                    label: SharedString::from("▣"),
                    step,
                    origin_window,
                    tree_field,
                    // Folder drags share the application-global overlay, so the preview must hide
                    // and stop when it leaves this tree or paints in another native window.
                    stop_when_outside: true,
                })
            },
        )
        // ── Drop target: core or folder. Use one `can_drop` for both payload types because GPUI
        // stores only one slot; two drop targets would overwrite each other and disable dropping.
        // The decorator also supplies drag-over highlighting and payload-specific `on_drop`. ──
        .row_decorator({
            let data = data.clone();
            let view = view.clone();
            move |row, entry, _meta, _w, app| {
                let Some((core, target)) = data.get(entry.item().id()).and_then(super::drop_dest)
                else {
                    return row;
                };
                let p = MoonPalette::active(app);
                let hl = moon_alpha(p.blue, 0.22);
                let (vs, ts) = (view.clone(), target.clone());
                let (vf, tf) = (view.clone(), target.clone());
                row.can_drop(|drag, _w, _a| drag.is::<StratDrag>() || drag.is::<FolderDrag>())
                    .drag_over::<StratDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .drag_over::<FolderDrag>(move |s, _d, _w, _a| s.bg(hl))
                    .on_drop::<StratDrag>(move |drag: &StratDrag, _w, app| {
                        let d = drag.clone();
                        // The decorator outlives the frame; a dead handle means the view is gone
                        // and there is nowhere to apply the drop.
                        let Some(vs) = vs.upgrade() else {
                            return;
                        };
                        vs.update(app, |this, cx| {
                            this.drop_strategies(core, ts.clone(), &d, cx)
                        });
                    })
                    .on_drop::<FolderDrag>(move |drag: &FolderDrag, _w, app| {
                        let d = drag.clone();
                        let Some(vf) = vf.upgrade() else {
                            return;
                        };
                        vf.update(app, |this, cx| this.drop_folder(core, tf.clone(), &d, cx));
                    })
            }
        });

        tree.into_any_element()
    }
}

/// The folder-set key a heading menu acts on, or `None` for a heading that addresses no folder.
///
/// The Deleted heading is the `None` case: it holds no live strategy, so it is not part of the
/// folder selection and right-clicking it must not disturb one.
pub(super) fn menu_folder_key(target: &MenuTarget, core: CoreId) -> Option<(CoreId, String)> {
    match target {
        MenuTarget::Core => Some((core, String::new())),
        MenuTarget::Folder(path) => Some((core, strategy_path::join_path(path))),
        MenuTarget::Strategy(_) | MenuTarget::DeletedFolder | MenuTarget::DeletedStrategy(_) => {
            None
        }
    }
}

/// Resolves a core-root or folder drop target as `(target core, path)`.
///
/// Args:
///     node: Row data for the hovered tree entry.
///
/// Returns:
///     The destination core and folder path, or `None` for rows that must not accept a drop.
pub(in crate::strategies::tree) fn drop_dest(node: &NodeData) -> Option<(CoreId, Vec<String>)> {
    match node {
        NodeData::Exchange { .. } => None,
        NodeData::Core { core, .. } => Some((*core, Vec::new())),
        NodeData::Folder { core, path, .. } => Some((*core, path.clone())),
        NodeData::Strategy { .. }
        | NodeData::DeletedFolder { .. }
        | NodeData::DeletedStrategy { .. } => None,
    }
}
