//! Rendering of the "WL distribution" tab: one row per core with its whitelist, what an empty
//! whitelist trades, and its blacklist as coin chips; and the tab switch that swaps this pane for
//! the parameter panes.
//!
//! The model is rebuilt only when its inputs move — the covered strategies (`scope`) and their two
//! lists, the saved order, and the rows' catalogs (by their own version, not the price-tick
//! snapshot revision) —
//! because this window repaints on hover, on strategy changes and on report commits, and the
//! catalog walk behind the coverage count is a pass over every market of the exchange.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::feed::SchemaField;
use moon_core::session::CoreId;
use moon_core::session::core_order::{OrderedCores, section_of};
use moon_core::symbol::coin_match_key;

use super::{
    BLACK_FIELD, Board, Edit, SlotInput, StrategyInput, Unavailable, WHITE_FIELD, build, moved,
    ordered,
};
use crate::analytics::period::Period;
use crate::design;
use crate::design::moon;
use crate::strategies::StrategiesView;
use crate::strategies::logic::{selected_keys, strategy_core_is_visible};

use super::scope::board_scope;

/// The tab's own state on the Strategies view.
#[derive(Default)]
pub(in crate::strategies) struct DistState {
    /// Whether the right side shows this tab instead of the parameter panes.
    pub(in crate::strategies) open: bool,
    /// The last model and the signature of the inputs it was built from.
    pub(super) cache: Option<(u64, Rc<Result<Board, Unavailable>>)>,
    /// Lists drawn in full.
    pub(super) expanded: HashSet<(CoreId, ListKind)>,
    /// Report figures: the period, the chip order, the clicked coin and the reads behind them.
    pub(super) stats: super::stats::StatsState,
    /// "Distribute": the first row keeps no whitelist and blacklists every other part.
    pub(in crate::strategies) first_blacklists: bool,
    /// "Distribute": leave the common blacklist's coins out of the parts.
    pub(super) skip_common_black: bool,
    /// Column widths and scroll of the trades table, kept across repaints.
    pub(super) trades_table: Option<Entity<moon_ui::MoonDataTableState>>,
    /// The trades table's rows in header order, with their strategy names.
    pub(super) trades_rows: Option<Rc<super::trades::TradeRows>>,
}

/// Which line of a row a chip list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ListKind {
    White,
    /// What an empty whitelist trades — derived, not written in the strategy.
    Traded,
    Black,
}

impl ListKind {
    /// Stable fragment for element ids.
    pub(super) fn id(self) -> &'static str {
        match self {
            ListKind::White => "w",
            ListKind::Traded => "t",
            ListKind::Black => "b",
        }
    }
}

/// A list field as it will be: this window's draft, else an edit still on its way to the core,
/// else what the core stores — the same tiers the parameters pane shows.
pub(super) fn effective(
    view: &StrategiesView,
    store: &moon_core::session::CoreStore,
    core: CoreId,
    row: &moon_core::feed::StrategyRow,
    name: &str,
) -> String {
    let schema =
        crate::strategies::logic::schema_field_in_kind(store, core, row.kind_ordinal, name);
    effective_with(view, store, core, row, name, schema)
}

/// [`effective`] with the field's schema entry already resolved, for a caller walking many rows
/// of one kind: the lookup is a linear scan of the kind's schema, and the distribution model runs
/// it over every covered row on every repaint.
///
/// Args:
///     schema: `name`'s entry in the row kind's schema, or `None` while the schema is unknown.
fn effective_with(
    view: &StrategiesView,
    store: &moon_core::session::CoreStore,
    core: CoreId,
    row: &moon_core::feed::StrategyRow,
    name: &str,
    schema: Option<&SchemaField>,
) -> String {
    use crate::strategies::logic::edited_field_value;
    match schema {
        Some(schema) => {
            let pending = store.core(core).and_then(|cd| cd.strategy_edit(row.id));
            edited_field_value(view, (core, row.id), row, schema, pending)
        }
        // No schema yet: a draft or the stored value, never a guess at the pending one.
        None => view
            .field_edits
            .get(&(core, row.id, name.to_string()))
            .cloned()
            .unwrap_or_else(|| field(&row.fields, name).to_string()),
    }
}

/// The value of a strategy field as the core sent it; an omitted field is empty.
pub(super) fn field<'a>(fields: &'a [(String, String)], name: &str) -> &'a str {
    fields
        .iter()
        .find(|(n, _)| n == name)
        .map_or("", |(_, v)| v.as_str())
}

impl StrategiesView {
    /// The two-item switch between the parameter panes and this tab.
    pub(in crate::strategies) fn right_tab_switch(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let open = self.dist.open;
        let switch = MoonSegmentedControl::new("strat-right-tab")
            .items([
                MoonSegmentItem::new("", t!("strat.tab_params").to_string())
                    .fit_width(cx, 64.0, 140.0)
                    .selected(!open),
                MoonSegmentItem::new("", t!("strat.tab_distribution").to_string())
                    .fit_width(cx, 64.0, 140.0)
                    .tooltip(t!("strat.tab_distribution_tip").to_string())
                    .selected(open),
            ])
            .on_click(move |ix, _, _window, app| {
                view.update(app, |this, cx| {
                    let open = ix == 1;
                    if this.dist.open != open {
                        this.dist.open = open;
                        cx.notify();
                    }
                });
            })
            .render();
        h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap_2()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .child(switch)
            .child(div().flex_1())
            .children(self.field_edit_actions(cx))
            .into_any_element()
    }

    /// Apply, Apply-and-refresh-buys and Revert for every draft of the window — ONE set above
    /// both tabs, since a whitelist edited in the distribution is a strategy field draft like any
    /// other and leaves through the same path.
    ///
    /// Moved here verbatim from the parameters header.
    fn field_edit_actions(&self, cx: &Context<Self>) -> Option<AnyElement> {
        let dirty = crate::strategies::logic::field_edit_count(self);
        if dirty == 0 {
            return None;
        }
        // Capture the complete visible draft set in the rendered Apply button. If the singleton
        // workspace moves before its callback runs, `apply_field_edits` rejects this plan whole.
        let apply_plan = std::sync::Arc::new(self.field_edit_plan(cx));
        // What Apply will actually land: drafts the core would refuse are not part of it.
        let (sendable, can_refresh) = {
            let backend = self.backend.read(cx);
            let store = backend.session.store();
            let keys: Vec<_> = self
                .sendable_field_edits(apply_plan.edit_keys(), store)
                .into_iter()
                .cloned()
                .collect();
            (keys.len(), self.can_refresh_buys(&keys, store))
        };
        // Apply counts what the plan will actually send, which excludes every draft the core
        // would refuse: promising "Apply 3" and landing 2 is the silence this change exists to
        // end. Revert stays on the full draft count, because a refused draft is exactly what one
        // wants to take back.
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .gap_2()
                .when(sendable > 0, |row| {
                    row.child(
                        MoonButton::new("strat-fields-apply")
                            .success()
                            .label(t!("strat.fields_apply", n = sendable).to_string())
                            .on_click({
                                let apply_plan = apply_plan.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.apply_field_edits(apply_plan.as_ref(), false, cx)
                                })
                            })
                            .render(),
                    )
                })
                .child(
                    MoonButton::new("strat-fields-refresh-buys")
                        .label(t!("strat.fields_refresh_buys"))
                        .tooltip(t!("strat.fields_refresh_buys_tip"))
                        .disabled(!can_refresh)
                        .on_click({
                            let apply_plan = apply_plan.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.apply_field_edits(apply_plan.as_ref(), true, cx)
                            })
                        })
                        .render(),
                )
                .child(
                    MoonButton::new("strat-fields-revert")
                        .ghost()
                        .label(t!("strat.fields_revert").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.discard_field_edits(cx)))
                        .render(),
                )
                .into_any_element(),
        )
    }

    /// The tab's pane, filling the space the parameter panes otherwise take.
    pub(in crate::strategies) fn distribution_pane(
        &mut self,
        cores: &OrderedCores,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.distribution_model(cores, cx);
        let p = MoonPalette::active(cx);
        let body = match model.as_ref() {
            Err(why) => {
                let key = match why {
                    Unavailable::NoSelection => "strat.dist_no_selection",
                    Unavailable::EmptyFolders => "strat.dist_empty_folders",
                    Unavailable::MixedVenues => "strat.dist_mixed_venues",
                    Unavailable::UnknownVenue => "strat.dist_unknown_venue",
                };
                div()
                    .flex_1()
                    .p(design::ui_px(cx, 16.0))
                    .text_color(moon(p.text_muted))
                    .child(t!(key).to_string())
                    .into_any_element()
            }
            Ok(board) => {
                let row_cores: Vec<CoreId> = board.slots.iter().map(|s| s.core).collect();
                self.ensure_distribution_stats(&row_cores, &board.names, cx);
                self.board(board, cx)
            }
        };
        let trades = model
            .as_ref()
            .is_ok()
            .then(|| self.distribution_trades(cx))
            .flatten();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(moon(p.panel))
            .child(body)
            .children(trades)
            .into_any_element()
    }

    /// The model for the current selection, rebuilt only when an input moved.
    fn distribution_model(
        &mut self,
        cores: &OrderedCores,
        cx: &mut Context<Self>,
    ) -> Rc<Result<Board, Unavailable>> {
        let backend = self.backend.read(cx);
        let store = backend.session.store();
        let venues = backend.session.core_venues();
        // Covered ids per core — the selected folders' strategies, else the strategy selection —
        // so the walk below visits only cores that hold some. It runs on every repaint, before the
        // cache can answer.
        let folders: Vec<(CoreId, String)> = self
            .folder_sel
            .iter()
            .filter(|(core, _)| strategy_core_is_visible(self.workspace_cores.as_deref(), *core))
            .cloned()
            .collect();
        let keys = board_scope(
            &folders,
            || selected_keys(self),
            |core| store.core(core).map(|cd| cd.strategies.as_slice()),
            &self.filter.prepare(),
            |core| self.filter.core_matches(venues.get(&core)),
        );
        if !folders.is_empty() && keys.is_empty() {
            // The last board goes too: chip-menu actions read the cache as the board on screen, and
            // a menu still open over a board that just emptied must not stage edits through it.
            self.dist.cache = None;
            return Rc::new(Err(Unavailable::EmptyFolders));
        }
        let source = backend.session.market_source();
        let saved: &[CoreId] = backend
            .layout
            .strategies_dist_order
            .as_deref()
            .unwrap_or(&[]);

        // Rows in tree order; within a core, strategies in the core's own order.
        let mut rows = Vec::new();
        for (core, core_name) in cores.iter() {
            let (Some(ids), Some(cd)) = (keys.get(core), store.core(*core)) else {
                continue;
            };
            let picked: Vec<_> = cd
                .strategies
                .iter()
                .filter(|r| ids.contains(&r.id))
                .collect();
            if !picked.is_empty() {
                rows.push((*core, core_name, picked));
            }
        }

        // Each kind's two list fields resolved once rather than twice per row: the lookup scans
        // the kind's schema, and this loop runs on every repaint over every covered row.
        let mut schemas: HashMap<(CoreId, u8), [Option<&SchemaField>; 2]> = HashMap::new();
        // The effective lists, kept for the build below so a miss does not resolve them twice.
        let mut lists: Vec<Vec<(String, String)>> = Vec::with_capacity(rows.len());
        let mut h = DefaultHasher::new();
        saved.hash(&mut h);
        for (core, name, picked) in &rows {
            core.hash(&mut h);
            name.hash(&mut h);
            section_of(venues.get(core)).hash(&mut h);
            // The catalog's own version — NOT the snapshot revision, which moves on every price
            // tick and rebuilt this model on nearly every frame (measured 02.10: rebuilds equal to
            // renders, ~4.5 ms each) — and the quote the core trades in, which is read off the
            // core's OWN client rather than the catalog's provider.
            source.catalog_revision(*core).hash(&mut h);
            source.traded_quote(*core).hash(&mut h);
            let mut slot_lists = Vec::with_capacity(picked.len());
            for r in picked {
                // The id too: the board bakes it into its rows, and an edit writes to it.
                r.id.hash(&mut h);
                r.name.hash(&mut h);
                field(&r.fields, WHITE_FIELD).hash(&mut h);
                field(&r.fields, BLACK_FIELD).hash(&mut h);
                // Drafts and edits on their way change what the row will hold.
                let [white_schema, black_schema] =
                    *schemas.entry((*core, r.kind_ordinal)).or_insert_with(|| {
                        [WHITE_FIELD, BLACK_FIELD].map(|name| {
                            crate::strategies::logic::schema_field_in_kind(
                                store,
                                *core,
                                r.kind_ordinal,
                                name,
                            )
                        })
                    });
                let white = effective_with(self, store, *core, r, WHITE_FIELD, white_schema);
                let black = effective_with(self, store, *core, r, BLACK_FIELD, black_schema);
                white.hash(&mut h);
                black.hash(&mut h);
                slot_lists.push((white, black));
            }
            lists.push(slot_lists);
        }
        let sig = h.finish();
        if let Some((cached, model)) = &self.dist.cache
            && *cached == sig
        {
            return model.clone();
        }

        crate::diag::bump(&crate::diag::STRAT_DIST_BUILD);
        let mut catalogs = HashMap::new();
        let inputs: Vec<SlotInput> = rows
            .iter()
            .zip(lists)
            .map(|((core, name, picked), slot_lists)| {
                let catalog = source
                    .tradable_coins(*core)
                    .map(|coins| coins.iter().map(|c| coin_match_key(c)).collect());
                catalogs.insert(*core, catalog);
                SlotInput {
                    core: *core,
                    core_name: (*name).clone(),
                    section: section_of(venues.get(core)),
                    strategies: picked
                        .iter()
                        .zip(slot_lists)
                        .map(|(r, (white, black))| StrategyInput {
                            id: r.id,
                            name: r.name.clone(),
                            white,
                            black,
                            live_white: field(&r.fields, WHITE_FIELD).to_string(),
                            live_black: field(&r.fields, BLACK_FIELD).to_string(),
                        })
                        .collect(),
                }
            })
            .collect();
        let model = Rc::new(build(inputs, saved, &catalogs));
        self.dist.cache = Some((sig, model.clone()));
        model
    }

    /// Move a row one place and save the new order.
    ///
    /// The rows come from the last drawn model, but their ORDER is re-derived from the saved one:
    /// a second click landing before the next frame must move from where the first one left the
    /// row, not from where the stale model still draws it.
    fn move_distribution_row(&mut self, core: CoreId, up: bool, cx: &mut Context<Self>) {
        let Some(drawn) = self.dist.cache.as_ref().and_then(|(_, m)| {
            m.as_ref()
                .as_ref()
                .ok()
                .map(|b| b.slots.iter().map(|s| s.core).collect::<Vec<_>>())
        }) else {
            return;
        };
        self.backend.update(cx, |backend, _| {
            let saved = backend
                .layout
                .strategies_dist_order
                .clone()
                .unwrap_or_default();
            let shown = ordered(&drawn, &saved);
            if let Some(order) = moved(&shown, &saved, core, up) {
                backend.layout.strategies_dist_order = Some(order);
                backend.layout_dirty = true;
            }
        });
        cx.notify();
    }

    /// The header and the rows of a drawable distribution.
    fn board(&self, board: &Board, cx: &mut Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let venue = match board.section {
            moon_core::session::core_order::ExchangeSection::Venue(id) => {
                crate::controls::venue_id_label(id)
            }
            moon_core::session::core_order::ExchangeSection::Unidentified => String::new(),
        };
        let counts = match board.coverage {
            Some(c) => t!(
                "strat.dist_coverage",
                total = c.total,
                traded = c.traded,
                idle = c.total - c.traded
            )
            .to_string(),
            None => t!("strat.dist_no_catalog").to_string(),
        };
        let header = h_flex()
            .w_full()
            .flex_none()
            .flex_wrap()
            .gap(design::ui_px(cx, 12.0))
            .items_center()
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 6.0))
            .border_b_1()
            .border_color(moon(p.border_soft))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(design::t_title(cx))
                    .child(venue),
            )
            .child(div().text_color(moon(p.text_soft)).child(counts))
            .when_some(self.dist.stats.profit_error(), |el, error| {
                el.child(
                    div()
                        .text_color(moon(p.red))
                        .child(t!("strat.dist_profit_failed", error = error).to_string()),
                )
            })
            .child(div().flex_1())
            .child(self.distribution_sort_switch(cx))
            .child(self.distribution_period_dropdown(cx));

        // Before the rows: their builder holds `cx` for as long as the iterator lives.
        let toolbar = self.distribution_toolbar(board, cx);
        let last = board.slots.len().saturating_sub(1);
        // Whether any list holds a draft: then the unchanged chips step back.
        let edited = board.slots.iter().any(|slot| {
            [&slot.white, &slot.black]
                .into_iter()
                .chain(slot.traded.as_ref())
                .flatten()
                .any(|c| c.edit != Edit::Same)
        });
        let rows = board
            .slots
            .iter()
            .enumerate()
            .map(|(ix, slot)| self.slot_row(ix, ix == last, slot, edited, cx));
        v_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(header)
            .child(toolbar)
            .child(
                v_flex()
                    .id("strat-dist-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    /// Name / profit order of the chips within each list.
    ///
    /// A click on the order already active turns it round; a click on the other one starts it in
    /// its natural direction (names A to Z, best result first). The arrow shows which way it runs.
    fn distribution_sort_switch(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let by_profit = self.dist.stats.by_profit;
        let reversed = self.dist.stats.reversed;
        let arrow = |active: bool, natural_down: bool| match (active, natural_down != reversed) {
            (false, _) => "",
            (true, true) => " ↓",
            (true, false) => " ↑",
        };
        MoonSegmentedControl::new("strat-dist-sort")
            .items([
                MoonSegmentItem::new(
                    "",
                    format!("{}{}", t!("strat.dist_sort_name"), arrow(!by_profit, false)),
                )
                .fit_width(cx, 48.0, 130.0)
                .selected(!by_profit),
                MoonSegmentItem::new(
                    "",
                    format!("{}{}", t!("strat.dist_sort_profit"), arrow(by_profit, true)),
                )
                .fit_width(cx, 48.0, 130.0)
                .tooltip(t!("strat.dist_sort_profit_tip").to_string())
                .selected(by_profit),
            ])
            .on_click(move |ix, _, _window, app| {
                view.update(app, |this, cx| {
                    let stats = &mut this.dist.stats;
                    let profit = ix == 1;
                    if stats.by_profit == profit {
                        stats.reversed = !stats.reversed;
                    } else {
                        stats.by_profit = profit;
                        stats.reversed = false;
                    }
                    cx.notify();
                });
            })
            .render()
            .into_any_element()
    }

    /// The period the profits and trades cover — the Analytics presets.
    fn distribution_period_dropdown(&self, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        let zone = self.display_zone;
        let items = Period::ALL.into_iter().map(|period| {
            let view = view.clone();
            MoonMenuItem::with_key(period.id(), period.title(zone)).on_click(
                move |_, _window, app| {
                    view.update(app, |this, cx| {
                        this.dist.stats.period = period;
                        cx.notify();
                    });
                },
            )
        });
        MoonDropdown::new("strat-dist-period")
            .label(self.dist.stats.period.title(zone))
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::density(cx))
            .fit_trigger_width(96.0, 160.0)
            .menu_width_scaled(180.0)
            .items(items.collect::<Vec<_>>())
            .into_any_element()
    }

    /// One core: its place, name and strategies on the left, its chip lines on the right (WL, what an empty WL trades, BL).
    fn slot_row(
        &self,
        ix: usize,
        last: bool,
        slot: &super::Slot,
        edited: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let core = slot.core;
        let arrow = |up: bool| {
            let (id, icon, tip) = match up {
                true => ("up", "icons/arrow-up.svg", t!("strat.dist_move_up")),
                false => ("down", "icons/arrow-down.svg", t!("strat.dist_move_down")),
            };
            MoonButton::new(ElementId::Name(format!("strat-dist-{id}-{core}").into()))
                .outline()
                .width(design::glyph_btn_w(cx))
                .leading_icon(MoonButtonIconSlot::new(icon))
                .tooltip(tip.to_string())
                .disabled(if up { ix == 0 } else { last })
                .on_click(
                    cx.listener(move |this, _, _, cx| this.move_distribution_row(core, up, cx)),
                )
                .render()
        };
        let mut left = v_flex()
            .flex_none()
            .w(design::ui_px(cx, 190.0))
            .gap(design::ui_px(cx, 2.0))
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 6.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(moon(p.text_muted))
                            .child(format!("{}.", ix + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(slot.core_name.clone()),
                    )
                    .child(arrow(true))
                    .child(arrow(false)),
            )
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(slot.strategies.join(", ")),
            );
        if slot.lists_differ {
            left = left.child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.amber))
                    .child(t!("strat.dist_lists_differ").to_string()),
            );
        }
        let lists = v_flex()
            .flex_1()
            .min_w_0()
            .gap(design::ui_px(cx, 4.0))
            .child(self.list_line(core, ListKind::White, &slot.white, edited, cx))
            .children(
                slot.traded
                    .as_ref()
                    .map(|traded| self.list_line(core, ListKind::Traded, traded, edited, cx)),
            )
            .child(self.list_line(core, ListKind::Black, &slot.black, edited, cx));
        h_flex()
            .w_full()
            .items_start()
            .gap(design::ui_px(cx, 12.0))
            .px(design::ui_px(cx, 12.0))
            .py(design::ui_px(cx, 8.0))
            .border_b_1()
            .border_color(moon(p.border_soft))
            .child(left)
            .child(lists)
            .into_any_element()
    }
}
