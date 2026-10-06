//! MoonTree headings helpers.

use super::*;

/// Render a clickable core, folder, or Deleted heading in the strategy tree.
///
/// Actual folders also receive the context menu assembled inside this function. The disclosure
/// glyph remains passive because the enclosing row handles interaction.
///
/// Args:
///     view: Strategies view updated by row interactions.
///     row_id: The node's own tree id, used verbatim as this row's `ElementId`.
///     expanded: Whether the heading's children are visible.
///     selected: Whether to draw the selected-folder highlight.
///     checked: Summary of covered strategies; ignored by a row that addresses no folder.
///     indent: Leading indentation for the tree depth.
///     label: Heading caption, counters excluded — they render in their own trailing column.
///     counts: The row's trailing counter column and its tooltip.
///     color: Heading text color.
///     weight: Heading font weight.
///     target: Core, folder, or Deleted collection toggled by the row.
///     engine: The core's confirmed global strategy engine; only a core heading draws the marker,
///         and only for `Some(false)`.
///     step: Local unscaled text-size step read from the tree's own preference.
///     app: Application context used for palette and sizing tokens.
///
/// Returns:
///     The complete interactive tree row.
#[allow(clippy::too_many_arguments)]
pub(super) fn core_folder_row(
    view: &Entity<StrategiesView>,
    row_id: SharedString,
    expanded: bool,
    selected: bool,
    checked: bool,
    indent: Pixels,
    label: String,
    counts: RowCounts,
    color: u32,
    weight: f32,
    target: ToggleTarget,
    fill: FolderFill,
    engine: Option<bool>,
    step: f32,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    // In this core/folder row type, only actual folders receive the egui-style context menu for
    // rename, copy, paste, create, and delete; core and Deleted headings do not.
    // The bulk checkbox stages this row's own subtree, which the core root spells as the empty
    // path. Deleted holds no live strategy, so it addresses no folder and carries no checkbox.
    // Resolved once: the row renders on every repaint, and each resolve deep-clones the path.
    let expandable = fill.has_contents();
    let folder_key = target.folder_key();
    let check_target = folder_key.clone().map(|(core, path)| (core, path, checked));
    // Every heading row carries a menu now. The core root and the Deleted heading each get their
    // own target rather than an empty folder path, so the menu can offer what only they can do:
    // paste into every visible core, and forget a whole Deleted folder.
    let menu = match &target {
        ToggleTarget::Folder(core, path) => Some((*core, MenuTarget::Folder(path.clone()))),
        ToggleTarget::Core(core) => Some((*core, MenuTarget::Core)),
        ToggleTarget::Deleted(core) => Some((*core, MenuTarget::DeletedFolder)),
    };
    // Taken before the row consumes `row_id`: the checkbox derives its own element id from this
    // node's id for the same reason the row does — see the note on `.id(row_id)` below.
    let check_row_id = row_id.clone();
    // Same rule for the counter column, which needs an id of its own to carry a tooltip. Derived
    // from the NODE id, never from the numbers it draws.
    let counts_row_id = SharedString::from(format!("cnt:{row_id}"));
    let view_click = view.clone();
    let view_menu = view.clone();
    // Resolved before the row so the caret slot and the mark after the checkbox
    // share one answer. The mark used to fill the caret slot and shove the glyph
    // into the previous indent, which stacked a folder caret on its core.
    let (folder_icon, caret) = heading_chrome(&target, fill, expanded);
    let edge = design::ui_px(app, design::DISCLOSURE_BOX + step);
    let caret_left = design::ui_px(app, disclosure_caret_shift(folder_icon.is_some(), step));
    h_flex()
        // The node's own id, NEVER the rendered text. GPUI keeps `pending_mouse_down` in element
        // state looked up by `ElementId`, so an id derived from the caption ("core  3/10  (2)")
        // becomes a DIFFERENT element the moment a counter moves — and a repaint between press and
        // release, which a trading core produces constantly, silently discards the click. That was
        // the "clicking a folder sometimes does not expand it" defect.
        .id(row_id)
        .w_full()
        .h(row_h(app, step))
        .pl(indent)
        .pr(design::ui_px(app, 6.0))
        .items_center()
        .gap(design::ui_px(app, HEADING_GAP))
        .cursor_pointer()
        .rounded(design::ui_px(app, 3.0))
        .when(selected, |s| s.bg(moon_alpha(p.amber, 0.14)))
        .when(!selected, |s| {
            s.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
        })
        .child(
            div()
                .relative()
                .flex_none()
                .size(edge)
                .when_some(caret, |slot, expanded| {
                    slot.child(
                        div().absolute().left(caret_left).top_0().child(
                            MoonDisclosure::glyph(expanded)
                                .size(design::DISCLOSURE_GLYPH_MARKER + step)
                                .box_size(design::DISCLOSURE_BOX + step),
                        ),
                    )
                }),
        )
        .child(match check_target.filter(|_| fill.has_contents()) {
            Some((core, path, checked)) => {
                checks::bulk_check(view, &check_row_id, core, path, checked)
            }
            // Reserved rather than omitted, so this row's caption stays on the same control column
            // as every sibling at its depth.
            None => checks::bulk_check_slot(&check_row_id),
        })
        // Folder mark after the checkbox, in the same place a strategy row draws its
        // status dot. The caret slot above stays one disclosure box wide, so the
        // checkbox column does not move and strategy rows keep their indent.
        .when_some(folder_icon, |row, path| {
            row.child(
                div()
                    .flex_none()
                    .child(svg().path(path).size(edge).text_color(rgb(color))),
            )
        })
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(label)
                    .mono(true)
                    .uppercase(false)
                    .color(color)
                    .weight(weight)
                    .font_size(design::body_font_base(app, step))
                    .line_height(ROW_LINE_BASE + step)
                    .render(),
            ),
        )
        .when(
            matches!(engine, Some(false)) && matches!(&target, ToggleTarget::Core(_)),
            |row| {
                row.child(
                    div().flex_none().child(
                        MoonText::new(rust_i18n::t!("strat.tree_engine_stopped_tag").to_string())
                            .mono(false)
                            .uppercase(false)
                            .color(p.amber)
                            .font_size(design::body_font_base(app, step))
                            .line_height(ROW_LINE_BASE + step)
                            .render(),
                    ),
                )
            },
        )
        // The counters, muted and right-aligned in fixed slots after the flexible caption, so they
        // land on one column across every row instead of wherever each name happened to end.
        //
        // A tooltip gives this element a hitbox (fork `elements/div.rs`, `should_insert_hitbox`),
        // but it keeps the default `HitboxBehavior::Normal`, which by contract "doesn't affect
        // mouse handling for other hitboxes" — so unlike an interactive `MoonDisclosure::button`
        // it cannot swallow the click that expands this row.
        .child(
            h_flex()
                .id(counts_row_id)
                .flex_none()
                .items_center()
                .gap(design::ui_px(app, COUNTS_GAP))
                .child(counts_slot(
                    counts.primary,
                    COUNTS_SLOT_W,
                    p.text_muted,
                    step,
                    app,
                ))
                // One tier softer than the fraction beside it: the two numbers mean different
                // things, and drawn in one colour "57/57 (50)" reads as a single three-part figure.
                .child(counts_slot(
                    counts.orders,
                    ORDERS_SLOT_W,
                    p.text_soft,
                    step,
                    app,
                ))
                .tooltip(crate::panels::common::text_tooltip(counts.tip)),
        )
        .children({
            let slots = RunSlots {
                status: false,
                trading: true,
                auto: false,
            };
            match &target {
                ToggleTarget::Core(core) => {
                    let backend = view.read(app).backend.clone();
                    let scope = RunScope {
                        key: RunKey::Core(*core),
                        cores: std::rc::Rc::from(vec![*core]),
                        reserve: slots,
                        offers: slots,
                    };
                    run_cell(&scope, &backend, p, app)
                }
                _ => reserved_cell(slots, app),
            }
        })
        .on_click(move |e: &ClickEvent, window, app| {
            let m = e.modifiers();
            let (shift, cmd) = (m.shift, m.secondary());
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                match &target {
                    ToggleTarget::Core(c) => {
                        // A MODIFIED click is selecting, not navigating: Ctrl-clicking a core to
                        // add it to the set must not also collapse the subtree the operator is
                        // building that set from.
                        if !shift && !cmd {
                            this.toggle_core_expanded(*c);
                        }
                        let order = this.nav_order.clone();
                        this.apply_folder_click((*c, String::new()), &order, shift, cmd);
                    }
                    ToggleTarget::Folder(c, path) => {
                        // Nothing to open, so nothing is toggled: the caret is not drawn for an
                        // empty folder, and flipping hidden expansion state would still churn the
                        // hashed set the whole tree is cached on. Selecting it below is what a
                        // click on it is for.
                        if expandable && !shift && !cmd {
                            toggle(&mut this.expanded_folders, (*c, path.join("/")));
                        }
                        // Match Moonbot by selecting the clicked folder for highlighting and Ctrl+C.
                        let order = this.nav_order.clone();
                        this.apply_folder_click((*c, path.join("/")), &order, shift, cmd);
                    }
                    ToggleTarget::Deleted(c) => {
                        toggle(&mut this.expanded_deleted, *c);
                        this.clear_folder_selection();
                    }
                }
                this.persist_session(cx);
                cx.notify();
            });
        })
        .when_some(menu, |row, (core, target)| {
            row.on_mouse_down(
                MouseButton::Right,
                move |e: &MouseDownEvent, window, app| {
                    app.stop_propagation();
                    let pos = e.position;
                    let target = target.clone();
                    view_menu.update(app, |this, cx| {
                        // Right-clicking OUTSIDE the current set acts on the clicked node alone,
                        // the way `strategy_row` does: the menu must never act on something the
                        // operator cannot see they selected.
                        if let Some(key) = menu_folder_key(&target, core)
                            && !this.folder_sel.contains(&key)
                        {
                            let order = this.nav_order.clone();
                            this.apply_folder_click(key, &order, false, false);
                            this.persist_session(cx);
                        }
                        this.open_menu(ContextMenu { core, target, pos }, window, cx);
                    });
                },
            )
        })
        .into_any_element()
}
