//! The group overview: what the right pane shows while no strategy is selected.
//!
//! Four tiles (cores running, cores stopped, strategies on, open orders), the edits the cores have
//! not confirmed yet, the stopped cores, and the quick filters that narrow the tree to one of those
//! questions. Scope is the window's own: the same visible cores the tree is built from.
//!
//! The model is a whole-account walk, so it is derived only when the store half of the tree
//! signature moves — the same revisions the tree already watches — and handed out again on every
//! other frame. A mouse moving over the window repaints it at monitor rate, and this pane must not
//! turn that into a walk over every strategy of every core.

use super::filter::QuickFilter;
use super::tree::moon::confirmed_engine;
use super::*;
use moon_core::session::store::CoreData;

#[cfg(test)]
mod tests;

/// How many stopped cores the overview lists before folding the rest into "+N more".
pub(super) const STOPPED_LISTED: usize = 5;
/// How many unconfirmed edits the overview lists before folding the rest into "+N more".
pub(super) const PENDING_LISTED: usize = 6;

/// One strategy whose edit the core has not confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingEdit {
    /// Core-qualified strategy identity, which a click selects.
    pub(super) key: Key,
    /// Strategy name as the core lists it.
    pub(super) name: String,
    /// Display name of the core holding it.
    pub(super) core_name: String,
    /// Whether the core let the edit time out rather than still working on it.
    pub(super) timed_out: bool,
}

/// One core whose strategy engine reported stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StoppedCore {
    /// Core identity, which a click selects in the tree.
    pub(super) core: CoreId,
    /// Display name of the core.
    pub(super) name: String,
    /// How many strategies the core holds.
    pub(super) strategies: usize,
}

/// Everything the overview draws, derived from the store for the window's visible cores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct OverviewModel {
    /// Cores that confirmed their strategy engine runs.
    pub(super) cores_running: usize,
    /// Cores that reported their strategy engine stopped.
    pub(super) cores_stopped: usize,
    /// Strategies the cores hold ticked.
    pub(super) enabled: usize,
    /// Every strategy the cores hold.
    pub(super) total: usize,
    /// Open orders across the cores.
    pub(super) open_orders: usize,
    /// Every unconfirmed edit, in core order then the core's own order.
    pub(super) pending: Vec<PendingEdit>,
    /// Every stopped core, in canonical core order.
    pub(super) stopped: Vec<StoppedCore>,
}

/// What a click in the overview asks the window to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OverviewClick {
    /// Select this strategy in the tree and scroll to it.
    Strategy(Key),
    /// Select this core in the tree and scroll to it.
    Core(CoreId),
    /// Toggle this quick filter on the tree.
    Quick(QuickFilter),
}

/// The callback every clickable overview element shares.
pub(super) type OverviewHandler = Rc<dyn Fn(OverviewClick, &mut Window, &mut App)>;

/// The retained model and the store signature it was derived from.
pub(super) struct OverviewCache {
    /// Store half of the tree signature the model was built under.
    store_sig: u64,
    /// The model itself, shared with the frame that renders it.
    model: Rc<OverviewModel>,
}

/// Derive the overview from the store for the given cores.
///
/// The engine state is read through [`confirmed_engine`], the same reading the core rows and the
/// Stopped section use, so a core counted stopped here is the core drawn stopped in the tree. A
/// core that confirmed neither state is in neither tile.
///
/// Args:
///     cores: Visible cores in canonical order — the window's workspace scope.
///     lookup: Live per-core data by core, `None` for a core with no data yet.
///
/// Returns:
///     The complete model; a core with no data yet contributes nothing.
pub(super) fn overview_model<'a>(
    cores: &[(CoreId, String)],
    lookup: impl Fn(CoreId) -> Option<&'a CoreData>,
) -> OverviewModel {
    let mut model = OverviewModel::default();
    for (core, name) in cores {
        let Some(cd) = lookup(*core) else {
            continue;
        };
        match confirmed_engine(cd) {
            Some(true) => model.cores_running += 1,
            Some(false) => {
                model.cores_stopped += 1;
                model.stopped.push(StoppedCore {
                    core: *core,
                    name: name.clone(),
                    strategies: cd.strategies.len(),
                });
            }
            None => {}
        }
        model.total += cd.strategies.len();
        model.enabled += cd.strategies.iter().filter(|row| row.checked).count();
        model.open_orders += cd.orders.iter().filter(|order| !order.job_is_done).count();
        for edit in &cd.strategy_edits {
            // An edit for a strategy the core no longer lists has nothing to select; it resolves
            // on its own and is not worth a line the operator cannot act on.
            let Some(row) = cd.strategies.iter().find(|row| row.id == edit.id) else {
                continue;
            };
            model.pending.push(PendingEdit {
                key: (*core, edit.id),
                name: row.name.clone(),
                core_name: name.clone(),
                timed_out: edit.phase == StrategyEditPhase::TimedOut,
            });
        }
    }
    model
}

impl StrategiesView {
    /// Return the overview model, rebuilt only when the store half of the tree signature moved.
    ///
    /// Args:
    ///     cx: Application context used to read the store and the visible cores.
    ///
    /// Returns:
    ///     The shared model for this frame.
    pub(super) fn overview_model_cached(&mut self, cx: &App) -> Rc<OverviewModel> {
        // The tree adapter computed this frame's signature before the right pane renders; its
        // store half covers the visible core list, their strategies, engine states, open-order
        // digests and open edits — everything `overview_model` reads.
        let store_sig = self.tree_cache.as_ref().map(|cache| cache.store_sig());
        if let (Some(sig), Some(cached)) = (store_sig, self.overview_cache.as_ref())
            && cached.store_sig == sig
        {
            return Rc::clone(&cached.model);
        }
        let model = {
            let backend = self.backend.read(cx);
            let cores = visible_strategy_cores(self, backend);
            let store = backend.session.store();
            Rc::new(overview_model(&cores, |core| store.core(core)))
        };
        if let Some(store_sig) = store_sig {
            self.overview_cache = Some(OverviewCache {
                store_sig,
                model: Rc::clone(&model),
            });
        }
        model
    }

    /// Carry out one overview click.
    ///
    /// A strategy goes through the window's ordinary reveal request, so it is selected by
    /// `selection::focus_strategy` like every other reveal and the single-writer rule holds. A core
    /// is selected the way a click on its row selects it, and scrolled to on the next frame. A
    /// quick filter toggles: picking the active one again removes it.
    ///
    /// Args:
    ///     click: What was clicked.
    ///     window: The Strategies window, for clearing the search input along with the filters.
    ///     cx: View context.
    pub(super) fn overview_click(
        &mut self,
        click: OverviewClick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match click {
            OverviewClick::Strategy((core, id)) => {
                self.backend.update(cx, |b, bcx| {
                    let workspace_group = b.singleton_workspace().map(|w| w.group);
                    b.strategies_goto = Some(StrategyRevealRequest::new(
                        core,
                        RevealTarget::Id(id),
                        workspace_group,
                    ));
                    bcx.notify();
                });
            }
            OverviewClick::Core(core) => {
                // A core the filters hide has no row to select or scroll to: clear them the way a
                // strategy reveal does, quick filter included.
                if self.core_hidden_by_filters(core, cx) {
                    self.clear_filters_for_reveal(window, cx);
                    self.prune_selection_to_filter(cx);
                }
                // Opened first: a core inside a folded section has no row to scroll to.
                self.stopped_collapsed = false;
                let order = self.nav_order.clone();
                self.apply_folder_click((core, String::new()), &order, false, false);
                self.pending_core_scroll = Some(core);
                self.persist_session(cx);
            }
            OverviewClick::Quick(quick) => {
                self.filter.quick = toggle_quick(self.filter.quick, quick);
                self.on_filter_changed(cx);
            }
        }
        cx.notify();
    }
}

/// The quick filter left in force after its chip is clicked.
///
/// Args:
///     current: Quick filter in force now.
///     clicked: Chip that was clicked.
///
/// Returns:
///     `None` when the clicked chip was the active one, otherwise the clicked one — quick filters
///     are exclusive, each answering one question.
pub(super) fn toggle_quick(
    current: Option<QuickFilter>,
    clicked: QuickFilter,
) -> Option<QuickFilter> {
    (current != Some(clicked)).then_some(clicked)
}

/// Render the overview pane.
///
/// Args:
///     model: What to draw.
///     quick: Quick filter in force, drawn as the active chip.
///     on_click: Shared handler every clickable element calls.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The pane, laid out to wrap its tiles and blocks rather than clip them.
pub(super) fn overview_panel(
    model: &OverviewModel,
    quick: Option<QuickFilter>,
    on_click: OverviewHandler,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let gap = design::ui_px(cx, 12.0);
    let tiles = h_flex()
        .debug_selector(|| "ov-tiles".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 10.0))
        .child(tile(
            0,
            t!("strat.overview_cores_label").to_string(),
            model.cores_running.to_string(),
            t!(crate::display_text::plural_key(
                "strat.overview_running_unit",
                model.cores_running as u64,
            ))
            .to_string(),
            false,
            cx,
        ))
        .child(tile(
            1,
            t!("strat.overview_stopped_label").to_string(),
            model.cores_stopped.to_string(),
            t!(crate::display_text::plural_key(
                "strat.overview_stopped_unit",
                model.cores_stopped as u64,
            ))
            .to_string(),
            model.cores_stopped > 0,
            cx,
        ))
        .child(tile(
            2,
            t!("strat.overview_enabled_label").to_string(),
            model.enabled.to_string(),
            t!("strat.overview_of", n = model.total).to_string(),
            false,
            cx,
        ))
        .child(tile(
            3,
            t!("strat.overview_orders_label").to_string(),
            model.open_orders.to_string(),
            String::new(),
            false,
            cx,
        ));

    let pending_lines: Vec<AnyElement> = model
        .pending
        .iter()
        .take(PENDING_LISTED)
        .map(|edit| {
            let meta = match edit.timed_out {
                true => format!(
                    "{} · {}",
                    edit.core_name,
                    t!("strat.overview_pending_timed_out")
                ),
                false => edit.core_name.clone(),
            };
            line(
                SharedString::from(format!("ov-pending:{}:{}", edit.key.0, edit.key.1)),
                edit.name.clone(),
                meta,
                OverviewClick::Strategy(edit.key),
                on_click.clone(),
                cx,
            )
        })
        .collect();
    let stopped_lines: Vec<AnyElement> = model
        .stopped
        .iter()
        .take(STOPPED_LISTED)
        .map(|core| {
            let meta = match core.strategies {
                0 => t!("strat.overview_core_empty").to_string(),
                n => t!("strat.overview_core_strats", n = n).to_string(),
            };
            line(
                SharedString::from(format!("ov-stopped:{}", core.core)),
                core.name.clone(),
                meta,
                OverviewClick::Core(core.core),
                on_click.clone(),
                cx,
            )
        })
        .collect();

    let blocks = h_flex()
        .debug_selector(|| "ov-blocks".into())
        .w_full()
        .flex_wrap()
        .items_start()
        .gap(gap)
        .child(block(
            "ov-block-pending",
            t!("strat.overview_pending_title").to_string(),
            QuickFilter::Unconfirmed,
            pending_lines,
            model.pending.len().saturating_sub(PENDING_LISTED),
            on_click.clone(),
            cx,
        ))
        .child(block(
            "ov-block-stopped",
            t!("strat.overview_stopped_title").to_string(),
            QuickFilter::Stopped,
            stopped_lines,
            model.stopped.len().saturating_sub(STOPPED_LISTED),
            on_click.clone(),
            cx,
        ));

    let chips = h_flex()
        .debug_selector(|| "ov-chips".into())
        .w_full()
        .flex_wrap()
        .gap(design::ui_px(cx, 6.0))
        .children(
            QuickFilter::ALL
                .into_iter()
                .map(|filter| quick_chip(filter, quick == Some(filter), on_click.clone(), cx)),
        );

    v_flex()
        .debug_selector(|| "ov-root".into())
        .w_full()
        .min_w_0()
        .gap(design::ui_px(cx, 16.0))
        .font_family(design::ui_font())
        .child(
            v_flex()
                .gap(design::ui_px(cx, 2.0))
                .child(
                    div()
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.text))
                        .child(t!("strat.overview_title").to_string()),
                )
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.no_selection").to_string()),
                ),
        )
        .child(tiles)
        .child(blocks)
        .child(
            v_flex()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.overview_quick_title").to_string()),
                )
                .child(chips),
        )
        .into_any_element()
}

/// One figure tile: a caption over a large number and its unit.
///
/// Args:
///     index: Position, used for the tile's debug selector.
///     label: Caption above the number.
///     value: The number.
///     unit: Muted words after the number, possibly empty.
///     warn: Whether to draw the tile amber (stopped cores present).
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     A tile that grows to share its row and wraps under a minimum width.
fn tile(
    index: usize,
    label: String,
    value: String,
    unit: String,
    warn: bool,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let border = match warn {
        true => moon_alpha(p.amber, 0.55),
        false => moon(p.border),
    };
    let figure = match warn {
        true => p.amber,
        false => p.text,
    };
    v_flex()
        .debug_selector(move || format!("ov-tile-{index}"))
        .flex_1()
        .min_w(design::ui_px(cx, 120.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 10.0))
        .gap(design::ui_px(cx, 4.0))
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(border)
        .child(
            div()
                .truncate()
                .text_size(design::t_caption(cx))
                .text_color(moon(p.text_muted))
                .child(label),
        )
        .child(
            h_flex()
                .debug_selector(move || format!("ov-tile-value-{index}"))
                .flex_wrap()
                .items_end()
                .gap(design::ui_px(cx, 6.0))
                .child(
                    div()
                        .font_family(design::mono())
                        .text_size(design::t_title(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(figure))
                        .child(value),
                )
                .when(!unit.is_empty(), |row| {
                    row.child(
                        div()
                            .font_family(design::mono())
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(unit),
                    )
                }),
        )
        .into_any_element()
}

/// One titled list block with a header link to its quick filter and a "none" line when empty.
///
/// Args:
///     id: Element id and debug selector.
///     title: Block caption.
///     filter: Quick filter its header link applies.
///     lines: Rendered list lines, at most the listed cap.
///     more: How many items the cap left out.
///     on_click: Shared overview handler.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The block, never an empty box.
fn block(
    id: &'static str,
    title: String,
    filter: QuickFilter,
    lines: Vec<AnyElement>,
    more: usize,
    on_click: OverviewHandler,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let empty = lines.is_empty();
    let link = filter.label();
    v_flex()
        .id(id)
        .debug_selector(move || id.into())
        .flex_1()
        .min_w(design::ui_px(cx, 220.0))
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(moon(p.border))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .px(design::ui_px(cx, 12.0))
                .py(design::ui_px(cx, 8.0))
                .border_b_1()
                .border_color(moon(p.border))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(design::t_body(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(moon(p.text))
                        .child(title),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-link")))
                        .flex_none()
                        .cursor_pointer()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.accent))
                        .child(link)
                        .on_click(move |_: &ClickEvent, window, app| {
                            on_click(OverviewClick::Quick(filter), window, app)
                        }),
                ),
        )
        .children(lines)
        .when(empty, |el| {
            el.child(
                div()
                    .debug_selector(move || format!("{id}-none"))
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 6.0))
                    .text_size(design::t_body(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("strat.overview_none").to_string()),
            )
        })
        .when(more > 0, |el| {
            el.child(
                div()
                    .px(design::ui_px(cx, 12.0))
                    .py(design::ui_px(cx, 6.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(t!("strat.overview_stopped_more", n = more).to_string()),
            )
        })
        .into_any_element()
}

/// One clickable list line: a truncating name and a muted trailing fact.
///
/// Args:
///     id: Element id, derived from the line's identity.
///     name: Strategy or core name.
///     meta: Muted trailing fact.
///     click: What clicking the line asks for.
///     on_click: Shared overview handler.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The interactive line.
fn line(
    id: SharedString,
    name: String,
    meta: String,
    click: OverviewClick,
    on_click: OverviewHandler,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .id(id)
        .w_full()
        .min_w_0()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .px(design::ui_px(cx, 12.0))
        .py(design::ui_px(cx, 6.0))
        .border_t_1()
        .border_color(moon(p.border_soft))
        .cursor_pointer()
        .hover(move |s| s.bg(moon_alpha(p.panel, 0.74)))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(moon(p.text))
                .child(name),
        )
        .child(
            div()
                .flex_none()
                .max_w(relative(0.5))
                .truncate()
                .text_color(moon(p.text_muted))
                .child(meta),
        )
        .on_click(move |_: &ClickEvent, window, app| on_click(click, window, app))
        .into_any_element()
}

/// One quick-filter chip, drawn active when it is the filter in force.
///
/// Args:
///     filter: The quick filter the chip toggles.
///     active: Whether it is in force now.
///     on_click: Shared overview handler.
///     cx: Application context used for palette and scaled geometry.
///
/// Returns:
///     The chip.
pub(super) fn quick_chip(
    filter: QuickFilter,
    active: bool,
    on_click: OverviewHandler,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    div()
        .id(SharedString::from(format!("ov-chip-{}", filter.key())))
        .debug_selector(move || format!("ov-chip-{}", filter.key()))
        .flex_none()
        .px(design::ui_px(cx, 8.0))
        .py(design::ui_px(cx, 2.0))
        .rounded(design::ui_px(cx, 10.0))
        .border_1()
        .border_color(match active {
            true => moon(p.accent),
            false => moon(p.border),
        })
        .bg(moon_alpha(p.panel, 0.74))
        .cursor_pointer()
        .text_size(design::t_caption(cx))
        .text_color(match active {
            true => moon(p.accent),
            false => moon(p.text),
        })
        .child(filter.label())
        .on_click(move |_: &ClickEvent, window, app| {
            on_click(OverviewClick::Quick(filter), window, app)
        })
        .into_any_element()
}
