//! Rendering of the "WL distribution" tab: one row per core, its whitelist and blacklist as coin
//! chips, and the tab switch that swaps this pane for the parameter panes.
//!
//! The model is rebuilt only when its inputs move — the selected strategies' two lists, the saved
//! order, and the rows' catalogs (by their own version, not the price-tick snapshot revision) —
//! because this window repaints on every backend wake and on hover, and the catalog walk behind
//! the coverage count is a pass over every market of the exchange.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonTag,
    MoonTone, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;
use moon_core::session::core_order::{OrderedCores, section_of};
use moon_core::symbol::coin_match_key;

use super::{
    BLACK_FIELD, Board, Chip, ChipState, SlotInput, StrategyInput, Unavailable, WHITE_FIELD, build,
    moved, ordered,
};
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
    /// Lists drawn in full: `(core, blacklist?)`.
    expanded: HashSet<(CoreId, bool)>,
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
            Ok(board) => self.board(board, cx),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(moon(p.panel))
            .child(body)
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
            .child(div().text_color(moon(p.text_soft)).child(counts));

        let last = board.slots.len().saturating_sub(1);
        let rows = board
            .slots
            .iter()
            .enumerate()
            .map(|(ix, slot)| self.slot_row(ix, ix == last, slot, cx));
        v_flex()
            .size_full()
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

    /// One core: its place, name and strategies on the left, its two lists on the right.
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
            .child(self.list_line(core, false, &slot.white, cx))
            .child(self.list_line(core, true, &slot.black, cx));
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
        black: bool,
        chips: &[Chip],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let expanded = self.dist.expanded.contains(&(core, black));
        let shown = if expanded {
            chips.len()
        } else {
            chips.len().min(CHIP_CAP)
        };
        let caption = match black {
            true => t!("strat.dist_black", n = chips.len()),
            false => t!("strat.dist_white", n = chips.len()),
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
                    .flex_none()
                    .w(design::ui_px(cx, 56.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(caption.to_string()),
            );
        if chips.is_empty() {
            let empty = match black {
                true => t!("strat.dist_black_empty"),
                false => t!("strat.dist_white_empty"),
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
        line = line.children(chips[..shown].iter().map(|chip| {
            let tone = match chip.state {
                ChipState::Normal => MoonTone::Default,
                ChipState::Gone => MoonTone::Muted,
                ChipState::Duplicate => MoonTone::Warning,
            };
            MoonTag::new().tone(tone).label(chip.coin.clone())
        }));
        if chips.len() > CHIP_CAP {
            let label = match expanded {
                true => t!("strat.dist_collapse").to_string(),
                false => format!("+{}", chips.len() - shown),
            };
            let side = if black { "b" } else { "w" };
            line = line.child(
                MoonButton::new(ElementId::Name(
                    format!("strat-dist-more-{core}-{side}").into(),
                ))
                .ghost()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.dist.expanded.remove(&(core, black)) {
                        this.dist.expanded.insert((core, black));
                    }
                    cx.notify();
                }))
                .render(),
            );
        }
        line.into_any_element()
    }
}
