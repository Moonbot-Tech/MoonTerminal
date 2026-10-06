//! MoonTree rows helpers.

use super::*;

/// Renders one row from `NodeData`.
pub(super) fn render_row(
    data: &HashMap<SharedString, NodeData>,
    view: &Entity<StrategiesView>,
    entry: &MoonTreeEntry,
    meta: MoonTreeRowMeta,
    app: &mut App,
) -> AnyElement {
    let _ = meta;
    crate::diag::bump(&crate::diag::STRAT_ROW_RENDER);
    let p = MoonPalette::active(app);
    // Read from the entity each render rather than a value captured into the row closure, so a
    // preference change is never served stale.
    let step = view.read(app).prefs.tree_text_step;
    let depth = entry.depth();
    let indent = design::ui_px(app, tree_row_indent(depth as f32));
    let node_id = entry.item().id().clone();
    let Some(node) = data.get(entry.item().id()) else {
        return div().into_any_element();
    };

    match node {
        NodeData::Exchange { label, logo } => {
            exchange_row(node_id, indent, label, logo.clone(), step, app)
        }
        NodeData::Core {
            core,
            label,
            active,
            total,
            open_orders,
            selected,
            checked,
            engine,
        } => {
            let core = *core;
            core_folder_row(
                view,
                node_id,
                entry.is_expanded(),
                *selected,
                *checked,
                indent,
                label.clone(),
                RowCounts::subtree(*active, *total, *open_orders, *engine),
                p.blue,
                600.0,
                ToggleTarget::Core(core),
                // A core root is a heading, not a folder: it keeps its caret and its bulk box even
                // with nothing under it, because what it covers is the whole core.
                FolderFill::Populated,
                *engine,
                step,
                app,
            )
        }
        NodeData::Folder {
            fill,
            core,
            path,
            label,
            active,
            total,
            selected,
            checked,
            engine,
        } => {
            let core = *core;
            let path = path.clone();
            core_folder_row(
                view,
                node_id,
                entry.is_expanded(),
                *selected,
                *checked,
                indent,
                label.clone(),
                // A folder carries no order count of its own; the core root above it owns that.
                // An empty one shows no numbers at all rather than `0/0`: there is nothing to
                // count, and the slot's tooltip says what the row is instead.
                match fill.has_contents() {
                    true => RowCounts::subtree(*active, *total, 0, *engine),
                    false => RowCounts::empty_folder(fill.empty_tip()),
                },
                // Empty folder captions remain quieter than populated ones beside their icon.
                match fill.has_contents() {
                    true => p.text_soft,
                    false => p.text_muted,
                },
                400.0,
                ToggleTarget::Folder(core, path),
                *fill,
                None,
                step,
                app,
            )
        }
        NodeData::Strategy {
            core,
            id,
            name,
            kind,
            open_orders,
            server_checked,
            staged,
            engine,
            highlighted,
            is_short,
            cut,
            ..
        } => strategy_row(
            view,
            *core,
            *id,
            name,
            kind,
            *open_orders,
            *server_checked,
            *staged,
            *engine,
            *highlighted,
            *is_short,
            *cut,
            indent,
            step,
            app,
        ),
        NodeData::DeletedFolder { core, count } => {
            let core = *core;
            core_folder_row(
                view,
                node_id,
                entry.is_expanded(),
                false,
                // Deleted addresses no folder, so `core_folder_row` draws it no checkbox at all.
                false,
                indent,
                rust_i18n::t!("strat.deleted_folder").to_string(),
                RowCounts::deleted(*count),
                p.text_muted,
                400.0,
                ToggleTarget::Deleted(core),
                // Deleted is only ever drawn when it holds rows.
                FolderFill::Populated,
                None,
                step,
                app,
            )
        }
        NodeData::DeletedStrategy {
            core,
            id,
            name,
            kind,
            is_short,
            highlighted,
        } => deleted_strategy_row(
            view,
            *core,
            *id,
            name,
            kind,
            *is_short,
            *highlighted,
            indent,
            step,
            app,
        ),
    }
}

/// Render a passive exchange section heading above its core children.
///
/// Args:
///     row_id: Stable exchange identity used as the row element ID.
///     indent: Tree-provided indentation for the heading depth.
///     label: Localized shared venue-section caption.
///     logo: Prewarmed brand logo, absent for unidentified venues.
///     step: Local unscaled text-size step read from the tree's own preference.
///     app: Application context providing palette and scaled geometry.
///
/// Returns:
///     Non-interactive hierarchy row with no selection, hover, disclosure, drag, or drop behavior.
pub(super) fn exchange_row(
    row_id: SharedString,
    indent: Pixels,
    label: &str,
    logo: Option<Arc<RenderImage>>,
    step: f32,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    h_flex()
        .id(row_id)
        .w_full()
        .h(row_h(app, step))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .when_some(logo, |row, logo| {
            row.child(
                img(logo)
                    .flex_none()
                    .w(design::ui_px(app, 13.0))
                    .h(design::ui_px(app, 13.0))
                    .rounded(design::ui_px(app, 2.0)),
            )
        })
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(label.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text_soft)
                    .weight(600.0)
                    .font_size(design::body_font_base(app, step))
                    .line_height(ROW_LINE_BASE + step)
                    .render(),
            ),
        )
        .into_any_element()
}
