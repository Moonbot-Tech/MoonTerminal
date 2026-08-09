//! The Alerts table: its columns, its rows, and the four cells that DO something — arm a figure,
//! open its chart, assign a strategy, and the pair of action buttons.

use super::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDataCell,
    MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDropdown, MoonMenuSize, MoonPopover,
    MoonPopoverPlacement, MoonTone, h_flex,
};
use rust_i18n::t;

use crate::panels::{RadioMark, radio_items};

/// Everything a row needs that is not in the row itself, gathered ONCE per render.
///
/// The table's row callback outlives the borrow that builds it, so each field is a handle or an
/// `Rc` rather than a reference — and gathering them here keeps a repaint from cloning the strategy
/// map once per visible row.
#[derive(Clone)]
struct RowCtx {
    view: Entity<AlertsPanel>,
    backend: Entity<Backend>,
    /// Owning group used to revalidate delayed row actions against the current Auto workspace.
    group: String,
    strategies: Rc<HashMap<CoreId, StrategyOptions>>,
    cols: Rc<Vec<AlCol>>,
    /// The figure whose settings panel is open; its row is marked, so the floating panel and the
    /// table agree on what is being edited.
    open_settings: Option<crate::figstyle::FigStyleTarget>,
    /// Gap between the two action glyphs, resolved through `design` once rather than per row: the
    /// row callback has no context to scale a raw pixel with.
    action_gap: Pixels,
    /// User-selected zone for the creation-time column.
    display_zone: chrono_tz::Tz,
    p: MoonPalette,
}

impl RowCtx {
    /// Revalidates a core-specific write, then updates the backend and refreshes the panel.
    ///
    /// The workspace check happens before the callback so a row rendered for an old Auto selection
    /// cannot mutate anything. After an authorized write, both halves matter: the backend
    /// notification wakes the chart and other panels, while the panel refresh keeps the clicked
    /// control from lagging a tick behind.
    fn commit_core(&self, app: &mut App, core: CoreId, f: impl FnOnce(&mut Backend)) {
        let allowed = self.backend.update(app, |b, bcx| {
            if !b.workspace_action_allows_core(Some(&self.group), core) {
                return false;
            }
            f(b);
            bcx.notify();
            true
        });
        if allowed {
            self.view.update(app, |this, cx| this.refresh(cx));
        }
    }
}

impl AlertsPanel {
    /// Builds the figure table.
    ///
    /// A method rather than a free function taking eight handles: everything it needs is already a
    /// field, and reading them off `self` avoids `cx.entity().read(cx)` — reading the panel while
    /// its own `render` is running is exactly the "cannot read while being updated" panic.
    pub(super) fn table(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let row_count = rows.len();
        let cols: Rc<Vec<AlCol>> = Rc::new(self.view.visible_columns());
        let ctx = RowCtx {
            view: cx.entity(),
            backend: self.backend.clone(),
            group: self.group.clone(),
            strategies: self.alert_strategies.clone(),
            cols: cols.clone(),
            open_settings: self.settings_for.clone(),
            action_gap: design::ui_px(cx, 2.0),
            display_zone: crate::chrome::clock::resolved_header_clock_zone(
                self.backend.read(cx).header_clock_zone(),
            ),
            p,
        };
        let sort_view = cx.entity();
        let table_state = self.table_state.clone();
        let state_reset = table_state.clone();

        crate::panels::common::data_table_host(
            "alerts-table-host",
            row_count == 0,
            t!("alerts.empty").to_string(),
            p,
            cx,
            MoonDataTable::new("alerts-table", row_count, move |ix, _window, app| {
                match rows.get(ix) {
                    Some(row) => build_row(row, &ctx, app),
                    // The virtual list can ask for an index the row set no longer holds, between a
                    // rebuild and the repaint that follows it. An empty row is what it draws then,
                    // rather than an index panic inside the frame loop.
                    None => MoonDataRow::new(
                        ctx.cols
                            .iter()
                            .map(|_| MoonDataCell::text(""))
                            .collect::<Vec<_>>(),
                    ),
                }
            })
            .columns(cols.iter().map(|c| column_def(*c)).collect::<Vec<_>>())
            .state(&table_state)
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
            // Row selection carries no meaning here — the marked row is the one whose settings are
            // open — so a click clears the fork's three coupled selection fields immediately.
            // Only when one of them is actually set: an unconditional `notify` here would wake the
            // table-state observer, and with it a column-width clone and a `Backend` write, on
            // every click that changed nothing.
            .on_select_row(move |_ix, _window, app| {
                state_reset.update(app, |s, c| {
                    if s.selected_row.is_none()
                        && s.selected_column.is_none()
                        && s.selected_cell.is_none()
                    {
                        return;
                    }
                    s.selected_row = None;
                    s.selected_column = None;
                    s.selected_cell = None;
                    c.notify();
                });
            })
            .on_sort(move |key, ascending, _window, app| {
                let Some(col) = AlCol::from_key(key).filter(|c| c.sortable()) else {
                    return;
                };
                AlertsPanel::mutate(&sort_view, app, |v| {
                    v.sort = col;
                    v.sort_asc = ascending;
                });
            }),
        )
    }
}

/// Builds a column's key, width and alignment. [`AlCol::ALL`] defines the canonical order.
///
/// `MoonDataTable` treats the declared width as a base width and an auto-layout weight, so a narrow
/// panel may shrink it toward the shared column floor; it is not a strict minimum.
fn column_def(col: AlCol) -> MoonDataTableColumn {
    let c = MoonDataTableColumn::new(col.key(), col.title(), col.width()).sortable(col.sortable());
    match col {
        // Numbers read right-aligned, as they do in every other table here.
        AlCol::Price | AlCol::Time => c.right(),
        // Two glyph buttons need exactly their own width and nothing more.
        AlCol::Actions => c.no_grow(),
        _ => c,
    }
}

/// Builds one row's cells.
///
/// Takes `&mut App` because the settings popover's CONTENT is built here, and building it needs a
/// `Context<AlertsPanel>` that only `Entity::update` can hand out. That is safe from this callback
/// and would not be from `render`: the table hands its rows to a `MoonVirtualList`, which asks for
/// them during layout — after the panel's own `render` has returned and released the entity.
fn build_row(row: &FigRow, ctx: &RowCtx, app: &mut App) -> MoonDataRow {
    MoonDataRow::new(
        ctx.cols
            .iter()
            .map(|c| cell_for(*c, row, ctx, app))
            .collect::<Vec<_>>(),
    )
    .selected(ctx.open_settings.as_ref().is_some_and(|t| row.is(t)))
}

fn cell_for(col: AlCol, row: &FigRow, ctx: &RowCtx, app: &mut App) -> MoonDataCell {
    match col {
        // Only a Moonbot type takes an alert at all. A Terminal type gets an EMPTY cell rather
        // than a box that can never be ticked: the Kind column beside it already says why, and a
        // permanently dead control only invites the click it refuses.
        AlCol::Alert => match row.alertable {
            true => MoonDataCell::element(alert_cell(row, ctx)),
            false => MoonDataCell::text(""),
        },
        AlCol::Core => MoonDataCell::text(row.core_name.clone()),
        AlCol::Coin => MoonDataCell::element(coin_cell(row, ctx)),
        AlCol::Figure => MoonDataCell::text(row.figure.clone()),
        AlCol::Kind => {
            // The figure's TYPE, not where it was drawn: a line drawn here is a Moonbot type and
            // can be armed, which is the whole question this column answers. It carries a tone of
            // its own so the answer reads at a glance instead of blending into the text beside it.
            if row.alertable {
                MoonDataCell::text(t!("alerts.kind.moonbot").to_string()).tone(MoonTone::Info)
            } else {
                MoonDataCell::text(t!("alerts.kind.terminal").to_string()).tone(MoonTone::Muted)
            }
        }
        AlCol::Price => MoonDataCell::text(fmt_price(row.price)),
        AlCol::Time => {
            MoonDataCell::text(fmt_time(row.time_ms, ctx.display_zone)).tone(MoonTone::Muted)
        }
        // The strategy is what the CORE runs when the alert fires, so it belongs to an armed
        // figure and to no other. An unarmed row leaves the cell empty.
        AlCol::Strategy => match row.armed {
            true => MoonDataCell::element(strategy_cell(row, ctx)),
            false => MoonDataCell::text(""),
        },
        AlCol::Actions => MoonDataCell::element(actions_cell(row, ctx, app)),
    }
}

/// The Alert checkbox — the cell that turns a drawing into a core alert.
///
/// Reached only for a Moonbot type; see the caller. Editable only when the click could actually
/// reach the core, and the tooltip always names the reason it cannot:
/// - a figure FROM a core is armed by definition and is not disarmed from here, because disarming
///   would delete Moonbot's own object; the delete button is the deliberate way to do that;
/// - a core that is not `Ready` would swallow the command — it is attempted once and never retried
///   — leaving the figure armed here and unknown to Moonbot;
/// - a figure shared across cores has no single core to arm it on;
/// - anything else toggles, and the wording follows the direction the click would take.
fn alert_cell(row: &FigRow, ctx: &RowCtx) -> AnyElement {
    let editable = !row.from_server && row.core_online && (row.can_arm || row.armed);
    let tip = if row.from_server {
        t!("alerts.arm.from_core").to_string()
    } else if !row.core_online {
        t!("alerts.arm.offline").to_string()
    } else if !row.can_arm && !row.armed {
        t!("alerts.arm.shared").to_string()
    } else if row.armed {
        t!("alerts.arm.disarm_hint").to_string()
    } else {
        t!("alerts.arm.hint").to_string()
    };
    let ctx = ctx.clone();
    let core = row.core;
    let market = row.market.clone();
    let id = row.id;
    div()
        .id(SharedString::from(format!("al-arm-tip-{core}-{id}")))
        .tooltip(crate::panels::common::text_tooltip(tip))
        .child(
            MoonCheckbox::new(SharedString::from(format!("al-arm-{core}-{id}")))
                .checked(row.armed)
                .disabled(!editable)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |on: &bool, _w, app| {
                    let (on, market) = (*on, market.clone());
                    ctx.commit_core(app, core, move |b| {
                        b.set_figure_alert(core, &market, id, on);
                    });
                }),
        )
        .into_any_element()
}

/// The coin cell: clicking it opens that market on Main and raises the window.
///
/// Raising is deliberate and is what this panel has always done — see the note on
/// `Backend::open_on_main`, which names the chart double-click and the Alerts coin click as the two
/// sources allowed to pull Main forward. Every other panel opens without stealing focus.
///
/// Args:
///     row: Figure row containing the captured core and market.
///     ctx: Render snapshot containing the owning group and Backend handle.
///
/// Returns:
///     A clickable cell whose retained callback raises Main only while its core remains visible.
fn coin_cell(row: &FigRow, ctx: &RowCtx) -> AnyElement {
    let backend = ctx.backend.clone();
    let group = ctx.group.clone();
    let core = row.core;
    let market = row.market.clone();
    div()
        .id(SharedString::from(format!("al-open-{core}-{}", row.id)))
        // The whole cell, not just the glyphs: a three-letter ticker is a narrow target.
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(rgb(MoonTone::Accent.color(ctx.p)))
        .child(row.coin.clone())
        .on_click(move |_, _w, app| {
            let market = market.clone();
            backend.update(app, |b, bcx| {
                if b.open_on_main_if_authorized(Some(&group), (core, market), true) {
                    bcx.notify();
                }
            });
        })
        .into_any_element()
}

/// The strategy assignment dropdown, offering this core's `Alerts` strategies plus the unassigned
/// placeholder. Assignment re-upserts an armed figure's blob with the strategy ID at offset 32.
fn strategy_cell(row: &FigRow, ctx: &RowCtx) -> AnyElement {
    let core = row.core;
    let id = row.id;
    // The option list is built once per core in `rebuild`, not once per row here: this runs for
    // every visible row on every repaint, and a core with fifty Alerts strategies would otherwise
    // format fifty keys and clone fifty names per row per frame.
    let options: StrategyOptions = ctx.strategies.get(&core).cloned().unwrap_or_default();
    let ctx = ctx.clone();
    // A `SharedString`, because `radio_items` clones the handler once per menu item — with a
    // `String` that is one heap copy of the market name per strategy.
    let market: SharedString = row.market.clone().into();
    let items = radio_items(
        options.iter().cloned(),
        row.strategy_id,
        RadioMark::Check,
        move |app, sid| {
            let market = market.clone();
            ctx.commit_core(app, core, move |b| {
                b.set_figure_strategy(core, &market, id, sid)
            });
        },
    );
    MoonDropdown::new(SharedString::from(format!("al-strat-{core}-{id}")))
        .label(row.strategy.clone())
        .trigger_caret(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Action)
        .trigger_width_scaled(150.0)
        .menu_width_scaled(220.0)
        .menu_size(MoonMenuSize::Compact)
        .items(items)
        .into_any_element()
}

/// The settings gear, and — for an armed figure only — the delete button.
///
/// The gear opens a `MoonPopover` anchored to itself. A popover and not a panel inside the table:
/// it is drawn in the window's deferred overlay, so it neither reflows the table nor gets clipped
/// by the dock tab's own bottom edge — both of which a framed pane did, and a two-row-tall tab left
/// half of it unreachable. Its content is [`crate::figstyle::rows`] BARE, because the popover
/// paints the surface itself.
///
/// Delete is offered on EVERY row: this panel lists the whole drawing layer, so it is also where a
/// figure is thrown away, armed or not. What differs is the reach — an armed figure takes its core
/// alert with it, and a figure drawn in Moonbot takes Moonbot's own object — which the tooltip says
/// before the click rather than after.
fn actions_cell(row: &FigRow, ctx: &RowCtx, app: &mut App) -> AnyElement {
    let core = row.core;
    let id = row.id;
    let target = crate::figstyle::FigStyleTarget {
        core,
        market: row.market.clone(),
        id,
    };
    let open = ctx.open_settings.as_ref() == Some(&target);
    let delete_ctx = ctx.clone();
    let delete_market = row.market.clone();
    // Three reaches, three words: a figure from a core takes Moonbot's own object with it, an armed
    // local one takes its core alert, and an unarmed one is only a drawing here.
    let delete_tip = match (row.from_server, row.armed) {
        (true, _) => t!("alerts.delete_core").to_string(),
        (false, true) => t!("alerts.delete_armed").to_string(),
        (false, false) => t!("alerts.delete").to_string(),
    };
    h_flex()
        .gap(ctx.action_gap)
        .items_center()
        .child(settings_popover(target, open, ctx, app))
        .child(
            MoonButton::new(SharedString::from(format!("al-del-{core}-{id}")))
                .label("✕")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .tooltip(delete_tip)
                .on_click(move |_, _w, app| {
                    let market = delete_market.clone();
                    delete_ctx.commit_core(app, core, move |b| b.remove_figure(core, &market, id));
                })
                .render(),
        )
        .into_any_element()
}

/// The gear button with its settings popover.
///
/// The content is built ONLY while open — `MoonPopover` takes it eagerly, and this runs for every
/// visible row on every repaint. Opening and closing both go through the panel's own
/// `settings_for`, so exactly one row can be open and the table can mark it. Opening revalidates
/// the row's current Auto visibility, and the shared figure-style callbacks receive this group's
/// authority so an already-open popover cannot write after the workspace moves.
fn settings_popover(
    target: crate::figstyle::FigStyleTarget,
    open: bool,
    ctx: &RowCtx,
    app: &mut App,
) -> impl IntoElement {
    let gear = MoonButton::new(SharedString::from(format!(
        "al-set-{}-{}",
        target.core, target.id
    )))
    .label("⚙")
    .size(MoonButtonSize::Micro)
    .variant(MoonButtonVariant::Ghost)
    .tooltip(t!("alerts.settings").to_string())
    .render();
    let toggle_view = ctx.view.clone();
    let toggle_target = target.clone();
    let toggle_backend = ctx.backend.clone();
    let toggle_group = ctx.group.clone();
    let mut popover = MoonPopover::new(SharedString::from(format!(
        "al-set-pop-{}-{}",
        target.core, target.id
    )))
    // Grown from the gear, which sits at the table's right edge: anchoring to that end keeps the
    // popup inside the window instead of off its right side.
    .placement(MoonPopoverPlacement::BottomEnd)
    .content_width(f32::from(design::font_w_px(app, 220.0)))
    // A click inside picks a colour or flips a switch; it must not dismiss the popup.
    .close_on_content_click(false)
    .open(open)
    .on_open_change(move |is_open, _window, app| {
        let allowed = !is_open
            || toggle_backend
                .read(app)
                .workspace_action_allows_core(Some(&toggle_group), toggle_target.core);
        toggle_view.update(app, |this, cx| {
            this.settings_for = (is_open && allowed).then(|| toggle_target.clone());
            cx.notify();
        });
    })
    .trigger(gear);
    if open {
        let backend = ctx.backend.clone();
        let content = ctx.view.update(app, |_this, cx| {
            crate::figstyle::rows(
                &backend,
                &crate::figstyle::Target::Figure(target),
                crate::figstyle::WorkspaceAuthority::Group(ctx.group.clone()),
                None,
                cx,
            )
        });
        if let Some(content) = content {
            popover = popover.content(content);
        }
    }
    popover
}

/// Formats a figure's anchor price through the shared adaptive formatter, leaving a figure with no
/// meaningful anchor blank.
///
/// Adaptive rather than a fixed six decimals: this table sits beside Orders and Assets, and a fixed
/// width printed BTC as `68000.000000` there and `68000` here.
fn fmt_price(price: f64) -> String {
    if price == 0.0 {
        String::new()
    } else {
        crate::panels::common::num(price)
    }
}

/// Format a creation instant as selected-zone `MM-DD HH:MM` for the narrow column.
///
/// Args:
///     ms: Absolute UTC Unix timestamp in milliseconds.
///     zone: Selected IANA display zone.
///
/// Returns:
///     Compact civil timestamp, or the shared formatter's fallback text.
fn fmt_time(ms: i64, zone: chrono_tz::Tz) -> String {
    let full = moon_core::util::display_time::format_minute(ms / 1000, zone);
    if full.len() >= 16 {
        format!("{} {}", &full[5..10], &full[11..16])
    } else {
        full
    }
}

#[cfg(test)]
mod tests;
