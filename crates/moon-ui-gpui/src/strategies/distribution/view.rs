//! Rendering of the "WL distribution" tab: one row per core with its whitelist, what an empty
//! whitelist trades, and its blacklist as coin chips; and the tab switch that swaps this pane for
//! the parameter panes.
//!
//! The model is rebuilt only when its inputs move — the selected strategies' two lists, the saved
//! order, and the rows' catalogs (by their own version, not the price-tick snapshot revision) —
//! because this window repaints on hover, on strategy changes and on report commits, and the
//! catalog walk behind the coverage count is a pass over every market of the exchange.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuItem,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonTag, MoonTone, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;
use moon_core::session::core_order::{OrderedCores, section_of};
use moon_core::symbol::coin_match_key;

use super::{
    BLACK_FIELD, Board, Chip, ChipState, Look, SlotInput, StrategyInput, Unavailable, WHITE_FIELD,
    build, by_profit, look, moved, ordered,
};
use crate::analytics::period::Period;
use crate::design;
use crate::design::{moon, moon_alpha};
use crate::strategies::StrategiesView;
use crate::strategies::logic::selected_keys;

/// Chips a collapsed list draws before its "+N" button. A first core holding everybody else's
/// coins as its blacklist runs to hundreds of chips, and a hover repaints the whole window.
const CHIP_CAP: usize = 60;

/// The tab's own state on the Strategies view.
#[derive(Default)]
pub(in crate::strategies) struct DistState {
    /// Whether the right side shows this tab instead of the parameter panes.
    pub(in crate::strategies) open: bool,
    /// The last model and the signature of the inputs it was built from.
    cache: Option<(u64, Rc<Result<Board, Unavailable>>)>,
    /// Lists drawn in full.
    expanded: HashSet<(CoreId, ListKind)>,
    /// Report figures: the period, the chip order, the clicked coin and the reads behind them.
    pub(super) stats: super::stats::StatsState,
    /// Column widths and scroll of the trades table, kept across repaints.
    pub(super) trades_table: Option<Entity<moon_ui::MoonDataTableState>>,
    /// The trades table's rows in header order, with their strategy names.
    pub(super) trades_rows: Option<Rc<super::trades::TradeRows>>,
}

/// Which line of a row a chip list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ListKind {
    White,
    /// What an empty whitelist trades — derived, not written in the strategy.
    Traded,
    Black,
}

impl ListKind {
    /// Stable fragment for element ids.
    fn id(self) -> &'static str {
        match self {
            ListKind::White => "w",
            ListKind::Traded => "t",
            ListKind::Black => "b",
        }
    }
}

/// The value of a strategy field as the core sent it; an omitted field is empty.
fn field<'a>(fields: &'a [(String, String)], name: &str) -> &'a str {
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
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .child(switch)
            .into_any_element()
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
                let names: BTreeSet<String> = board
                    .slots
                    .iter()
                    .flat_map(|s| s.strategies.iter().cloned())
                    .collect();
                let row_cores: Vec<CoreId> = board.slots.iter().map(|s| s.core).collect();
                self.ensure_distribution_stats(&row_cores, &names, cx);
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
        // Selected ids per core, so the walk below visits only cores that hold a selection — it
        // runs on every repaint, before the cache can answer.
        let mut keys: HashMap<CoreId, HashSet<u64>> = HashMap::new();
        for (core, id) in selected_keys(self) {
            keys.entry(core).or_default().insert(id);
        }
        let backend = self.backend.read(cx);
        let store = backend.session.store();
        let venues = backend.session.core_venues();
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
            for r in picked {
                r.name.hash(&mut h);
                field(&r.fields, WHITE_FIELD).hash(&mut h);
                field(&r.fields, BLACK_FIELD).hash(&mut h);
            }
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
            .map(|(core, name, picked)| {
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
                        .map(|r| StrategyInput {
                            name: r.name.clone(),
                            white: field(&r.fields, WHITE_FIELD).to_string(),
                            black: field(&r.fields, BLACK_FIELD).to_string(),
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

        let last = board.slots.len().saturating_sub(1);
        let rows = board
            .slots
            .iter()
            .enumerate()
            .map(|(ix, slot)| self.slot_row(ix, ix == last, slot, cx));
        v_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(header)
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
            .child(self.list_line(core, ListKind::White, &slot.white, cx))
            .children(
                slot.traded
                    .as_ref()
                    .map(|traded| self.list_line(core, ListKind::Traded, traded, cx)),
            )
            .child(self.list_line(core, ListKind::Black, &slot.black, cx));
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

    /// One list of one core as a wrapped line of chips; the blacklist sits on a red tint.
    fn list_line(
        &self,
        core: CoreId,
        kind: ListKind,
        chips: &[Chip],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let black = kind == ListKind::Black;
        let expanded = self.dist.expanded.contains(&(core, kind));
        let shown = if expanded {
            chips.len()
        } else {
            chips.len().min(CHIP_CAP)
        };
        let caption = match kind {
            ListKind::White => t!("strat.dist_white", n = chips.len()),
            ListKind::Traded => t!("strat.dist_traded", n = chips.len()),
            ListKind::Black => t!("strat.dist_black", n = chips.len()),
        };
        let mut line = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap(design::ui_px(cx, 4.0))
            .p(design::ui_px(cx, 4.0))
            .rounded(design::ui_px(cx, 4.0))
            .when(black, |el| el.bg(moon_alpha(p.red, 0.08)))
            .child(
                div()
                    .id(ElementId::Name(
                        format!("strat-dist-caption-{core}-{}", kind.id()).into(),
                    ))
                    .flex_none()
                    .w(design::ui_px(cx, 80.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .when(kind == ListKind::Traded, |el| {
                        el.tooltip(crate::panels::common::text_tooltip(
                            t!("strat.dist_traded_tip").to_string(),
                        ))
                    })
                    .child(caption.to_string()),
            );
        if chips.is_empty() {
            let empty = match kind {
                ListKind::White => t!("strat.dist_white_empty"),
                ListKind::Traded => t!("strat.dist_traded_empty"),
                ListKind::Black => t!("strat.dist_black_empty"),
            };
            return line
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(moon(p.text_faint))
                        .child(empty.to_string()),
                )
                .into_any_element();
        }
        let stats = self.dist.stats.coin_stats();
        let stat_of = |coin: &str| stats.and_then(|s| s.get(coin));
        let mut ordered: Vec<&Chip> = match self.dist.stats.by_profit {
            true => by_profit(chips, |coin| {
                stat_of(coin).and_then(|s| s.comparable_profit())
            }),
            false => chips.iter().collect(),
        };
        if self.dist.stats.reversed {
            ordered.reverse();
        }
        let selected = self.dist.stats.coin.as_deref();
        let side = kind.id();
        line = line.children(ordered[..shown].iter().map(|chip| {
            let stat = stat_of(&chip.coin);
            let tone = match look(chip.state, stat.and_then(|s| s.comparable_profit())) {
                Look::Gone => MoonTone::Muted,
                Look::Blocked => MoonTone::Danger,
                Look::Duplicate => MoonTone::Warning,
                Look::Profit => MoonTone::Positive,
                Look::Loss => MoonTone::Negative,
                Look::Flat => MoonTone::Default,
            };
            let tip = match stat.and_then(|s| s.win_rate().map(|wr| (s, wr))) {
                Some((s, wr)) => t!(
                    "strat.dist_chip_tip",
                    coin = chip.coin,
                    n = s.trades,
                    wr = wr,
                    profit = super::trades::profit_text(s)
                )
                .to_string(),
                None => t!("strat.dist_chip_tip_none", coin = chip.coin).to_string(),
            };
            // The reason a chip is not traded leads its tooltip: the figures below it are history.
            let blocked = chip.state == ChipState::Blocked;
            let tip = match blocked {
                true => format!("{}\n{tip}", t!("strat.dist_chip_blocked")),
                false => tip,
            };
            let coin = chip.coin.clone();
            // The selection is a tint BEHIND the chip, not a tone of its own: a selected coin that
            // is gone from the exchange or traded twice must still say so.
            let is_selected = selected == Some(chip.coin.as_str());
            div()
                .id(ElementId::Name(
                    format!("strat-dist-chip-{core}-{side}-{}", chip.coin).into(),
                ))
                .flex_none()
                .rounded_full()
                .cursor_pointer()
                .when(is_selected, |el| el.bg(moon_alpha(p.accent, 0.35)))
                .tooltip(crate::panels::common::text_tooltip(tip))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.toggle_distribution_coin(&coin, cx)),
                )
                .child(MoonTag::new().tone(tone).label(match blocked {
                    // A mark the colour alone could not carry: the blacklist line under it is drawn
                    // on a red tint, so a red chip by itself would read as "this is the BL".
                    true => format!("⊘ {}", chip.coin),
                    false => chip.coin.clone(),
                }))
        }));
        if chips.len() > CHIP_CAP {
            let label = match expanded {
                true => t!("strat.dist_collapse").to_string(),
                false => format!("+{}", chips.len() - shown),
            };
            line = line.child(
                MoonButton::new(ElementId::Name(
                    format!("strat-dist-more-{core}-{side}").into(),
                ))
                .ghost()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.dist.expanded.remove(&(core, kind)) {
                        this.dist.expanded.insert((core, kind));
                    }
                    cx.notify();
                }))
                .render(),
            );
        }
        line.into_any_element()
    }
}
