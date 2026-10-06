//! MoonTree strategy row helpers.

use super::*;

/// Renders a muted deleted-strategy row without a checkbox or DnD.
/// Clicking selects it and jumps to its latest version; right-click opens its Restore context menu.
///
/// Args:
///     step: Local unscaled text-size step read from the tree's own preference.
#[allow(clippy::too_many_arguments)]
pub(super) fn deleted_strategy_row(
    view: &Entity<StrategiesView>,
    core: CoreId,
    id: u64,
    name: &str,
    kind: &str,
    _is_short: bool,
    highlighted: bool,
    indent: Pixels,
    step: f32,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let key: Key = (core, id);
    let view_click = view.clone();
    let view_menu = view.clone();
    let mut name_row = h_flex()
        .id(SharedString::from(format!("dstrat-{core}-{id}")))
        .flex_1()
        .min_w_0()
        .h(row_h(app, step))
        .items_center()
        .justify_between()
        .gap(design::ui_px(app, 6.0))
        .px(design::ui_px(app, 6.0))
        .rounded(design::ui_px(app, 3.0))
        .border_1()
        .border_color(moon_alpha(p.border, 0.0))
        .cursor_pointer()
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(name.to_string())
                    .mono(true)
                    .uppercase(false)
                    .color(p.text_muted)
                    .font_size(design::body_font_base(app, step))
                    .line_height(ROW_LINE_BASE + step)
                    .render(),
            ),
        )
        .child(
            MoonBadge::new(kind.to_string())
                .tone(MoonTone::Info)
                .variant(MoonBadgeVariant::Soft)
                .size(row_badge_size(step))
                .render_with_theme(p, MoonTheme::active_tokens(app)),
        )
        .on_click(move |_e, window, app| {
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                this.select_deleted_strategy(key, cx);
            });
        })
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                app.stop_propagation();
                let pos = e.position;
                view_menu.update(app, |this, cx| {
                    this.select_deleted_strategy(key, cx);
                    this.open_menu(
                        ContextMenu {
                            core,
                            target: MenuTarget::DeletedStrategy(id),
                            pos,
                        },
                        window,
                        cx,
                    );
                });
            },
        );
    if highlighted {
        name_row = name_row
            .bg(moon_alpha(p.amber, 0.16))
            .border_color(moon_alpha(p.amber, 0.55));
    } else {
        name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
    }
    // No outer `.py(...)`: `name_row` above already carries `row_h(app, step)`, and `uniform_list`
    // measures this row's total height, so an outer pad here would desync it from the heading rows
    // (C2, plan-corrections-1.md).
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .pl(indent + disclosure_run(app, step))
        .pr(design::ui_px(app, 2.0))
        .child(
            MoonText::new("✕")
                .mono(true)
                .uppercase(false)
                .color(p.text_muted)
                .font_size(design::body_font_base(app, step))
                .line_height(ROW_LINE_BASE + step)
                .render(),
        )
        .child(name_row)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
/// Render one strategy row already admitted by the effective workspace tree.
///
/// Args:
///     view: Owning Strategies view used by row callbacks.
///     core: Workspace-visible core containing the strategy.
///     id: Live strategy id within the core.
///     name: Displayed strategy name.
///     kind: Displayed strategy kind.
///     open_orders: Current open-order count.
///     server_checked: Checkbox state acknowledged by the core.
///     staged: Visible retained checkbox override, when one exists for this row.
///     engine: Confirmed core-engine state; stopped or unknown rows cannot look running.
///     highlighted: Whether filtering should emphasize the row.
///     is_short: Whether the strategy trades the short side.
///     cut: Whether a pending Cut dims the strategy caption.
///     indent: Tree indentation for the row.
///     step: Local unscaled text-size step read from the tree's own preference.
///     app: Application context used for theme and design tokens.
///
/// Returns:
///     The rendered row; staging retained on hidden Classic cores never reaches this function.
pub(super) fn strategy_row(
    view: &Entity<StrategiesView>,
    core: CoreId,
    id: u64,
    name: &str,
    kind: &str,
    open_orders: usize,
    server_checked: bool,
    staged: Option<bool>,
    engine: Option<bool>,
    highlighted: bool,
    _is_short: bool,
    cut: bool,
    indent: Pixels,
    step: f32,
    app: &App,
) -> AnyElement {
    let p = MoonPalette::active(app);
    let key: Key = (core, id);
    let val = staged.unwrap_or(server_checked);
    let (tone, active) = checks::strategy_state_style(server_checked, staged, engine == Some(true));
    let dot = if active { p.green } else { p.text_muted };
    let kind_txt = if open_orders > 0 {
        format!("{kind}({open_orders})")
    } else {
        kind.to_string()
    };

    // Make the name and kind/open-order count the clickable selection area.
    let view_click = view.clone();
    let view_menu = view.clone();
    let mut name_row = h_flex()
        .id(SharedString::from(format!("strat-{core}-{id}")))
        .flex_1()
        .min_w_0()
        .h(row_h(app, step))
        .items_center()
        .justify_between()
        .gap(design::ui_px(app, 6.0))
        .px(design::ui_px(app, 6.0))
        .rounded(design::ui_px(app, 3.0))
        .border_1()
        .border_color(moon_alpha(p.border, 0.0))
        .cursor_pointer()
        .child(
            div().flex_1().min_w_0().truncate().child(
                MoonText::new(name.to_string())
                    .mono(true)
                    .uppercase(false)
                    // Cut rows read as "on their way out" until the paste lands or the clipboard
                    // replaces them. A tone change rather than a badge: the row keeps its shape,
                    // and the tree stays scannable while several rows are marked.
                    .color(if cut { p.text_muted } else { p.text })
                    .font_size(design::body_font_base(app, step))
                    .line_height(ROW_LINE_BASE + step)
                    .render(),
            ),
        )
        .child(
            // Keep the strategy kind informational regardless of trade direction.
            MoonBadge::new(kind_txt)
                .tone(MoonTone::Info)
                .variant(MoonBadgeVariant::Soft)
                .size(row_badge_size(step))
                .render_with_theme(p, MoonTheme::active_tokens(app)),
        )
        .on_click(move |e: &ClickEvent, window, app| {
            let m = e.modifiers();
            let shift = m.shift;
            let cmd = m.secondary();
            view_click.update(app, |this, cx| {
                window.focus(&this.focus, cx);
                let order = this.flat_order.clone();
                if this.apply_click(key, &order, shift, cmd) {
                    this.clamp_selected_section(cx);
                    this.persist_session(cx);
                    cx.notify();
                }
            });
        })
        .on_mouse_down(
            MouseButton::Right,
            move |e: &MouseDownEvent, window, app| {
                app.stop_propagation();
                let pos = e.position;
                view_menu.update(app, |this, cx| {
                    if !this.sel.contains(&key) {
                        this.focus_strategy(key);
                        this.clamp_selected_section(cx);
                        this.persist_session(cx);
                    }
                    this.open_menu(
                        ContextMenu {
                            core,
                            target: MenuTarget::Strategy(id),
                            pos,
                        },
                        window,
                        cx,
                    );
                });
            },
        );
    if highlighted {
        name_row = name_row
            .bg(moon_alpha(p.amber, 0.16))
            .border_color(moon_alpha(p.amber, 0.55));
    } else {
        name_row = name_row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
    }

    let view_chk = view.clone();
    // No outer `.py(...)`: `name_row` above already carries `row_h(app, step)`, and `uniform_list`
    // measures this row's total height, so an outer pad here would desync it from the heading rows
    // (C2, plan-corrections-1.md).
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(app, 6.0))
        .pl(indent + disclosure_run(app, step))
        .pr(design::ui_px(app, 2.0))
        .child(
            checks::row_checkbox(
                SharedString::from(format!("chk:{}", id_strat(core, id))),
                val,
            )
            .tone(tone)
            .on_change(move |ch: &bool, _window, app| {
                let v = *ch;
                view_chk.update(app, |this, cx| {
                    if !strategy_core_is_visible(this.workspace_cores.as_deref(), key.0) {
                        return;
                    }
                    let before = this.staged.get(&key).copied();
                    this.stage_check(key, v, server_checked);
                    if before != this.staged.get(&key).copied() {
                        cx.notify();
                    }
                });
            }),
        )
        .child(
            // A solid nine-unit disc stays legible without depending on a font's tiny bullet.
            design::status_dot_sized(dot, 9.0 + step, app),
        )
        .child(name_row)
        .into_any_element()
}
