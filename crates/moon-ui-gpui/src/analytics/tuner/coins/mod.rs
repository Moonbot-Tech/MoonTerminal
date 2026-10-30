//! The "By coin" mode of the "Strategy tuning" tab.
//!
//! Layout mirrors the other axes: on the LEFT the strategy list with the coin table
//! pinned under it, on the RIGHT the shared "Fact vs variants" KPI matrix and the coin
//! picker (`picker`) — the `CoinsBlackList` field itself, ordered newest-first and shaded
//! by how recently each coin entered the list.
//!
//! The table's ROWS are every coin the selected cores traded in the period, while the
//! NUMBERS on each row come from the selected strategies only — so a coin the strategy
//! never touched (a blacklisted one, typically) is still visible, at zero. The two sets
//! come from two different scopes and are merged here, not in SQL.
//!
//! State lives in `coins_state`; this module owns the loads and the rendering.

// Files of this axis.
/// Which columns the coin table shows and how wide it lays them out.
/// The Auto-pick card: settings, the running pick, the verdict.
pub(in crate::analytics::tuner) mod auto;
pub(in crate::analytics::tuner) mod columns;
/// One editable list field and the "Not saved" band, as free render functions.
pub(in crate::analytics::tuner) mod fields;
/// The coin table's filter bar, as a free render function.
pub(in crate::analytics::tuner) mod filter_bar;

pub(in crate::analytics::tuner) mod layout;
/// Its background loads: per-coin aggregates, the strategies' saved lists, the KPI rescan.
mod load;
/// The coin picker: the `CoinsBlackList` field, ordered and shaded by age.
pub(in crate::analytics::tuner) mod picker;
/// Its row model and the cache that keeps a repaint from rebuilding it.
mod rows;
/// Writing the picked list into the strategies (Save / Make a copy).
mod save;
/// Its own state: the working coin lists, the view controls, the loaded results.
pub(in crate::analytics::tuner) mod state;

use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonCheckbox, MoonInputEvent, MoonInputState, MoonPalette, MoonScrollbarVisibility,
    MoonVirtualList, h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::kpi::kpi_matrix_card;
use super::list::MAX_ROWS;
use super::{
    COIN_COLS, COIN_DEFAULT_SORT, COIN_NAME_MIN_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COIN_TICK_W,
    SORT_NAME, metric_cell, sort_arrow_of, toggle_sort_key,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use columns::COIN_AUTO_W;
use moon_core::db::analytics::GroupStat;
use rows::{CoinCounts, CoinRow};
use state::coin_token;

impl AnalyticsView {
    /// Lazily-created coin search input; commits on every change so the table filters
    /// live as you type (the entity is cached, so the re-render keeps input focus).
    fn coin_search_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(s) = &self.coins.search_input {
            return s.clone();
        }
        let val = self.coins.search.clone();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(val)
                .placeholder(t!("analytics.coins.search_ph").to_string())
        });
        cx.subscribe(&state, |this, st, ev: &MoonInputEvent, cx| {
            if matches!(
                ev,
                MoonInputEvent::Change | MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. }
            ) {
                this.coins.search = st.read(cx).value().to_string();
                cx.notify();
            }
        })
        .detach();
        self.coins.search_input = Some(state.clone());
        state
    }

    /// Write the axis' filter and pick settings into the saved layout.
    pub(in crate::analytics::tuner) fn persist_coins_settings(&self, cx: &mut Context<Self>) {
        let value = Some(self.coins.to_layout());
        self.persist_setting(cx, |l| &mut l.analytics_coins, value);
    }

    /// One numeric setting's input: a valid value commits and is persisted on every change;
    /// text that is not one is ignored, and the field is put back to the value in force when
    /// it is left or Enter is pressed. A changed pick setting retires its previous result.
    ///
    /// Args:
    ///     placeholder: Hint shown in an empty field.
    ///     get: Reads the value in force.
    ///     set: Writes a parsed value.
    ///     parse: Accepts the field's text, or rejects it.
    ///     window: Window the input lives in.
    ///     cx: View context.
    ///
    /// Returns:
    ///     The new input entity; the caller keeps it.
    pub(in crate::analytics::tuner) fn numeric_setting_input<T>(
        &mut self,
        placeholder: &'static str,
        get: fn(&state::CoinsState) -> T,
        set: fn(&mut state::CoinsState, T),
        parse: fn(&str) -> Option<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState>
    where
        T: Copy + PartialEq + ToString + 'static,
    {
        let val = get(&self.coins).to_string();
        let state = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(val)
                .placeholder(placeholder)
        });
        cx.subscribe_in(
            &state,
            window,
            move |this, st, ev: &MoonInputEvent, window, cx| {
                let leaving =
                    matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. });
                if !leaving && !matches!(ev, MoonInputEvent::Change) {
                    return;
                }
                if let Some(v) = parse(st.read(cx).value().trim())
                    && v != get(&this.coins)
                {
                    let pick_cfg = this.coins.pick_cfg;
                    set(&mut this.coins, v);
                    if pick_cfg != this.coins.pick_cfg {
                        this.reset_coin_pick(cx);
                    }
                    this.persist_coins_settings(cx);
                    cx.notify();
                }
                if leaving {
                    let shown = get(&this.coins).to_string();
                    st.update(cx, |input, cx| input.set_value(shown, window, cx));
                }
            },
        )
        .detach();
        state
    }

    /// Lazily-created "fewer trades than" input.
    fn coin_min_trades_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(s) = &self.coins.min_trades_input {
            return s.clone();
        }
        let state = self.numeric_setting_input(
            "0",
            |s| s.min_trades,
            |s, v| s.min_trades = v,
            state::parse_min_trades,
            window,
            cx,
        );
        self.coins.min_trades_input = Some(state.clone());
        state
    }

    /// The filter bar: the free renderer fed from this view's state, with handlers that edit
    /// it and persist the axis' settings.
    fn coin_filter_bar(
        &self,
        search: &Entity<MoonInputState>,
        min_trades: &Entity<MoonInputState>,
        counts: CoinCounts,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let black_idle = self
            .coins
            .rows
            .as_ref()
            .map_or(&[][..], |c| c.black_idle.as_slice());
        let model = filter_bar::FilterBarModel::new(&self.coins, counts, black_idle);
        let view = cx.entity();
        let acts = filter_bar::FilterBarActs {
            on_filter: Rc::new({
                let view = view.clone();
                move |f, _: &mut Window, app: &mut App| {
                    view.update(app, |this, cx| {
                        this.coins.filter = f;
                        cx.notify();
                    });
                }
            }),
            on_hide_zero: Rc::new(move |on, _: &mut Window, app: &mut App| {
                view.update(app, |this, cx| {
                    if this.coins.hide_zero != on {
                        this.coins.hide_zero = on;
                        this.persist_coins_settings(cx);
                        cx.notify();
                    }
                });
            }),
        };
        filter_bar::filter_bar_el(&model, search, min_trades, &acts, p, cx)
    }

    /// Sortable heading aligned with [`coin_row`], with shared single-trade metric tooltips.
    fn coin_header(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let scale = design::font_scale(cx);
        let sortable =
            |id: SharedString, title: String, key: &'static str, w: Option<(f32, f32)>| {
                let arrow = sort_arrow_of(&self.coins.sort, key);
                let mut d = div()
                    .id(id)
                    .flex_none()
                    .truncate()
                    .cursor_pointer()
                    .text_color(if arrow.is_empty() {
                        moon(p.text_soft)
                    } else {
                        moon(p.amber)
                    })
                    .child(format!("{title}{arrow}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        toggle_sort_key(&mut this.coins.sort, key);
                        let preference = this
                            .coins
                            .sort
                            .as_ref()
                            .filter(|(column, descending)| {
                                column.as_str() != COIN_DEFAULT_SORT || !*descending
                            })
                            .map(
                                |(column, descending)| moon_core::config::TableSortPreference {
                                    column: column.clone(),
                                    ascending: !*descending,
                                },
                            );
                        crate::persistence::table_persist::set_sort(
                            &this.backend,
                            "analytics-tuner-coins:win",
                            preference,
                            cx,
                        );
                        cx.notify();
                    }));
                if let Some(tip) = super::columns::metric_header_tooltip(key) {
                    d = d.tooltip(crate::panels::common::text_tooltip(tip));
                } else if key == SORT_NAME {
                    d = d.tooltip(crate::panels::common::text_tooltip(
                        t!("analytics.coins.tip_sort_name").to_string(),
                    ));
                }
                match w {
                    // Shrinks with its body cell, floor included, or the heading leaves the
                    // column it labels as soon as the panel narrows.
                    Some((w, min)) => d
                        .text_right()
                        .w(px(w * scale))
                        .min_w(px(min * scale))
                        .flex_shrink_1(),
                    // Floored like the row below it, or a narrow panel drives the heading off
                    // the column it labels.
                    None => d.flex_1().min_w(px(COIN_NAME_MIN_W * scale)),
                }
            };
        h_flex()
            .w_full()
            .flex_none()
            .h(design::fit_h_px(cx, 22.0, 12.0, 5.0))
            .px(design::ui_px(cx, COIN_ROW_PAD_X))
            .gap(design::ui_px(cx, COIN_ROW_GAP))
            .items_center()
            .text_size(design::t_caption(cx))
            .text_color(moon(p.text_soft))
            .bg(moon(p.table_head))
            .child(sortable(
                "an-coin-hdr-name".into(),
                t!("analytics.col.coin").to_string(),
                SORT_NAME,
                None,
            ))
            .children(COIN_COLS.iter().map(|c| {
                sortable(
                    SharedString::from(format!("an-coin-hdr-{}", c.key)),
                    t!(c.key).to_string(),
                    c.key,
                    Some((c.w, c.min_w)),
                )
            }))
            // The pick's suggestion, read-only: a tag, never a tick.
            .child(
                div()
                    .id("an-coin-hdr-auto")
                    .w(design::font_w_px(cx, COIN_AUTO_W))
                    .flex_none()
                    .text_center()
                    .truncate()
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("analytics.coins.tip_col_auto").to_string(),
                    ))
                    .child(t!("analytics.coins.col_auto").to_string()),
            )
            // The two list ticks close the row. They carry headings but no sort: they are
            // the EDIT surface, and a master tick over thousands of coins would be one
            // click into a list nobody meant to build.
            .child(
                div()
                    .id("an-coin-hdr-bl")
                    .w(design::font_w_px(cx, COIN_TICK_W))
                    .flex_none()
                    .text_center()
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("analytics.coins.tip_col_bl").to_string(),
                    ))
                    .child(t!("analytics.coins.black").to_string()),
            )
            .child(
                div()
                    .id("an-coin-hdr-wl")
                    .w(design::font_w_px(cx, COIN_TICK_W))
                    .flex_none()
                    .text_center()
                    .tooltip(crate::panels::common::text_tooltip(
                        t!("analytics.coins.tip_col_wl").to_string(),
                    ))
                    .child(t!("analytics.coins.white").to_string()),
            )
    }

    /// The coin table sits under the strategy list and asks for a read-visible selection.
    pub(in crate::analytics::tuner) fn coins_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = super::StratMode::Coins;
        if self.bottom_collapsed(mode) {
            return self.folded_bottom(mode, t!("analytics.tab.coins").to_string(), p, cx);
        }
        let scale = design::font_scale(cx);
        let scope = self.scope_label();
        let search = self.coin_search_state(window, cx);
        let min_trades = self.coin_min_trades_state(window, cx);
        // Rows come from the base coin set (every coin the selected cores traded); the
        // numbers from the selected strategies. Either read failing is REPORTED, never
        // rendered as zeros — a read failure that looks like "this strategy traded nothing"
        // is exactly what `LoadState` exists to prevent. The two error arms are mutually
        // exclusive, so they share one note element.
        //
        // `|_| false` on the scoped read: an empty scope is a real answer (the strategy
        // traded nothing this period), so only loading / not-ready / failed produce a note.
        let ready = match (
            self.strategy_data.view(|d| d.coins.is_empty()),
            self.coins.stats.view(|_| false),
        ) {
            (Err(note), _) | (Ok(_), Err(note)) => Err(note),
            (Ok(base), Ok(scoped)) => Ok((base.clone(), scoped.clone())),
        };
        let selected = self.visible_target_count(self.read_core_ids()) > 0;
        let (body, counts, total) = match (selected, ready) {
            // No strategy: the table asks for one rather than listing every coin of every
            // strategy, which is not a list anyone edits.
            (false, _) => (
                div()
                    .w_full()
                    .p(design::ui_px(cx, 28.0))
                    .font_family(design::ui_font())
                    .text_center()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.coins.table_pick").to_string())
                    .into_any_element(),
                CoinCounts::default(),
                0usize,
            ),
            (true, Err(note)) => (
                super::super::note_el("an-coins-note", note, 10.0, p, cx),
                CoinCounts::default(),
                0usize,
            ),
            (true, Ok((base, scoped))) => {
                // Built once per change of what it depends on — NOT per frame. Hovering a
                // row repaints the whole view without touching any of those inputs.
                //
                // `drawn` is the cache's OWN length, not `total`: `rows::build` has already
                // cut the tail at `MAX_ROWS`, while `total` is the pre-cut count. Taking it
                // here, in the same block, is also what releases the borrow before
                // `cx.entity()` below needs the view.
                let (counts, total, drawn) = {
                    let cache = rows::rows_for(&mut self.coins, &base.coins, &scoped);
                    (cache.counts, cache.total, cache.rows.len())
                };
                if coins_body(true, true, drawn) == CoinsBody::NoMatch {
                    // Data exists but the search/filter matched nothing — say so instead of
                    // leaving a blank area.
                    let note = div()
                        .w_full()
                        .p(design::ui_px(cx, 18.0))
                        .font_family(design::ui_font())
                        .text_center()
                        .text_color(moon(p.text_muted))
                        .child(no_match_text(counts.hidden))
                        .into_any_element();
                    (note, counts, 0)
                } else {
                    // The factory runs only for rows on screen, keeping repaints of the shared
                    // Analytics view proportional to the visible rows; see `list::table` for the
                    // same ownership shape.
                    let weak = cx.entity().downgrade();
                    // Resolved once and captured, like `scale` above: the pitch and the height
                    // the row draws itself at must be the same number, and deriving it again
                    // inside the factory would pay for a theme lookup per drawn row per frame.
                    let row_h = coin_row_h(cx);
                    let list =
                        MoonVirtualList::new("an-coin-rows", drawn, row_h, move |ix, _w, app| {
                            weak.upgrade()
                                .and_then(|e| {
                                    // Cache and index are read from the view in ONE go, so an
                                    // index can never be applied to a row set it was not
                                    // computed against.
                                    let row = e.read(app).coins.rows.as_ref()?.rows.get(ix)?;
                                    Some(coin_row(&weak, row, p, scale, row_h, app))
                                })
                                .unwrap_or_else(|| div().into_any_element())
                        })
                        .surface(false)
                        .border(false)
                        .radius(0.0)
                        .scrollbar_visibility(MoonScrollbarVisibility::Hover)
                        .into_any_element();
                    (list, counts, total)
                }
            }
        };
        // Derived, never carried: the two cannot drift out of step.
        let shown = total.min(MAX_ROWS);
        if super::super::probe_enabled() {
            probe(
                "coins",
                format!(
                    "coins scope={scope} rows={shown}/{total} all={} bl={} wl={} changed={}",
                    counts.all, counts.black, counts.white, counts.changed
                ),
            );
        }

        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .flex_none()
                    // FIXED height: the "changed" badge and its revert button appear and
                    // disappear as the lists are edited, and a header sized by its contents
                    // twitched the whole table every time they did.
                    .h(design::fit_h_px(cx, 34.0, 14.0, 8.0))
                    .px(design::ui_px(cx, 12.0))
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .flex_none()
                            .font_family(design::ui_font())
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.tab.coins").to_string()),
                    )
                    .child(
                        // NOT flipped: `scope` (`scope_label()`) is sometimes the raw name of
                        // the single selected strategy, sometimes a caption ("all strategies" /
                        // "N selected") — it stays mono so the raw-name case remains legible.
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .font_family(design::mono())
                            .child(scope),
                    )
                    .when(total > shown, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .font_family(design::ui_font())
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(
                                    t!("analytics.strat.shown", shown = shown, total = total)
                                        .to_string(),
                                ),
                        )
                    })
                    .child(self.bottom_caret(mode, p, cx)),
            )
            .child(self.coin_filter_bar(&search, &min_trades, counts, p, cx))
            .child(self.coin_header(p, cx))
            // No `overflow_y_scroll` here: the virtual list owns its own scrolling, and an
            // outer scroller would hand it the full content height and defeat it entirely.
            .child(div().w_full().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// Fact / Manual / Auto for the read-visible coin selection; hidden retained rows keep the empty state.
    pub(in crate::analytics::tuner) fn coins_kpi(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        // No strategy: the matrix says so instead of scoring every strategy's trades.
        if coins_body(self.visible_target_count(self.read_core_ids()) > 0, true, 1)
            == CoinsBody::NoStrategy
        {
            return super::shared::card(
                t!("analytics.tuner.kpi_title").to_string(),
                String::new(),
                div()
                    .w_full()
                    .p(design::ui_px(cx, 18.0))
                    .font_family(design::ui_font())
                    .text_center()
                    .text_color(moon(p.text_muted))
                    .child(t!("analytics.coins.no_strategy").to_string())
                    .into_any_element(),
                None,
                p,
                cx,
            );
        }
        kpi_matrix_card(
            &self.coins.kpi,
            self.scope_label(),
            &coin_kpi_labels(&self.coins),
            self.kpi_collapsed,
            p,
            cx,
        )
    }
}

/// The Manual and Auto headings of the coin KPI matrix.
///
/// Titled with the counts the SHOWN matrix was computed for, not the live ones: a recompute
/// keeps the previous columns on screen, and the live counts would label them with lists
/// they were never scored under. Title and composition on separate lines: the composition
/// grows with the lists and wrapped inside the fixed column.
///
/// Args:
///     s: Coin state carrying the counts of the last scored plan.
///
/// Returns:
///     The two headings after Fact.
pub(in crate::analytics::tuner) fn coin_kpi_labels(
    s: &state::CoinsState,
) -> [super::kpi::VarLabel; 2] {
    let lists = |bl: usize, wl: usize| t!("analytics.coins.v1_lists", bl = bl, wl = wl).to_string();
    let manual = super::kpi::VarLabel::with_sub(
        t!("analytics.coins.manual").to_string(),
        lists(s.kpi_bl, s.kpi_wl),
    )
    .with_tip(t!("analytics.coins.tip_manual").to_string());
    // The Auto column is always drawn so the matrix does not grow a column under the user's
    // eyes: a dash until a pick finished, its verdict word when it finished with nothing to
    // score (a dash there read as "never ran").
    let auto = super::kpi::VarLabel::with_sub(
        t!("analytics.coins.auto_col").to_string(),
        s.kpi_auto
            .map_or_else(|| auto_idle_sub(&s.pick), |(bl, wl)| lists(bl, wl)),
    )
    .with_tip(t!("analytics.coins.tip_auto_col").to_string())
    .placeholder();
    [manual, auto]
}

/// The Auto heading's second line while no Auto variant is scored.
///
/// Args:
///     pick: The card's pick state.
///
/// Returns:
///     The finished pick's verdict word when it suggested nothing, else a dash.
pub(in crate::analytics::tuner) fn auto_idle_sub(pick: &state::PickState) -> String {
    use moon_core::db::tuner::coin_pick::{NoiseReason, PickVerdict};
    let state::PickState::Done { pick, .. } = pick else {
        return "—".to_string();
    };
    match pick.verdict {
        PickVerdict::TooFewTrades | PickVerdict::Noise(NoiseReason::FewHoldout) => {
            t!("analytics.coins.auto_sub_few").to_string()
        }
        PickVerdict::NothingFound => t!("analytics.coins.auto_sub_nothing").to_string(),
        PickVerdict::Noise(_) => t!("analytics.coins.auto_sub_noise").to_string(),
        PickVerdict::Useful => "—".to_string(),
    }
}

/// Height of one coin row, in base px.
///
/// Follows the single-source pitch rule documented by `list::table::strat_row_h`: the virtual
/// list and [`coin_row`] both consume this value, because differing pitches would overlap or gap
/// rows.
fn coin_row_h(cx: &App) -> f32 {
    design::fit_h_value(cx, 24.0, 14.0, 5.0)
}

/// One coin row: each tick box toggles its own list, while clicking elsewhere selects the coin.
///
/// Uses the weak-handle rule documented by `list::table::strategy_row`: the `'static` factory
/// must not retain a strong entity because that closes
/// `AnalyticsView -> element state -> closure -> AnalyticsView` and leaks the window.
fn coin_row(
    weak: &WeakEntity<AnalyticsView>,
    row: &CoinRow,
    p: MoonPalette,
    scale: f32,
    row_h: f32,
    cx: &App,
) -> AnyElement {
    let g: &GroupStat = &row.stat;
    let name = row.name.clone();
    // Each tick box carries its own handler because a row-wide toggle could not identify which
    // of the two lists the click targets.
    let tick = |black_side: bool, on: bool| {
        // A refcount bump, not a copy: `CoinRow.name` is already a `SharedString`, and the
        // handler only needs an owned token once, at click time.
        let coin = name.clone();
        let weak = weak.clone();
        let side = if black_side { "bl" } else { "wl" };
        div()
            .id(SharedString::from(format!("an-coin-{side}-cell-{name}")))
            .w(design::font_w_px(cx, COIN_TICK_W))
            .flex_none()
            .flex()
            .justify_center()
            .tooltip(crate::panels::common::text_tooltip(
                t!(if black_side {
                    "analytics.coins.tip_col_bl"
                } else {
                    "analytics.coins.tip_col_wl"
                })
                .to_string(),
            ))
            .child(
                MoonCheckbox::new(SharedString::from(format!(
                    "an-coin-{}-{name}",
                    if black_side { "bl" } else { "wl" }
                )))
                .checked(on)
                .on_change(move |_, _w, app| {
                    let token = coin_token(&coin);
                    // The view may already be gone; a dropped window is not an error here.
                    let _ = weak.update(app, |this, cx| {
                        this.toggle_coin_list(black_side, token, cx);
                    });
                }),
            )
    };
    let mut el = h_flex()
        .id(SharedString::from(format!("an-coinrow-{name}")))
        .w_full()
        .h(px(row_h))
        .px(design::ui_px(cx, COIN_ROW_PAD_X))
        .gap(design::ui_px(cx, COIN_ROW_GAP))
        .items_center()
        .bg(moon(p.table_body))
        .border_t_1()
        .border_color(moon_alpha(p.border, 0.5))
        // The floor goes on the flex ITEM: at zero width the name would paint outside
        // its own box, over the numbers next to it, instead of truncating.
        .child(
            div()
                .flex_1()
                .min_w(design::font_w_px(cx, COIN_NAME_MIN_W))
                .truncate()
                .child(name.clone()),
        )
        .children(COIN_COLS.iter().map(|col| metric_cell(col, g, p, scale)))
        .child(auto_cell(row.auto, p, cx))
        .child(tick(true, row.black))
        .child(tick(false, row.white))
        // Clicking the ROW asks "who traded this?" — the tick boxes keep their own
        // handlers, so a click on one of them never reaches here as a selection.
        .on_click({
            let weak = weak.clone();
            let coin = name.clone();
            move |ev: &ClickEvent, _window, app| {
                let (coin, multi) = (coin.to_string(), ev.modifiers().secondary());
                let _ = weak.update(app, |this, cx| this.pick_coin(coin, multi, cx));
            }
        })
        .cursor_pointer();
    // Two different things get two different colours: BLUE is "this is the coin the
    // strategy list is currently answering for", AMBER is "this row carries an unsaved
    // edit". Selection wins the background when both are true — it is the transient one,
    // and the edit is still shown by the ticks.
    if row.picked {
        el = el.bg(moon_alpha(p.blue, 0.18));
    } else if row.changed {
        el = el.bg(moon_alpha(p.amber, 0.12));
    } else if let Some(black) = row.auto {
        // A suggested coin is tinted with its list's colour, faintly: a hint, not an edit.
        el = el.bg(moon_alpha(if black { p.red } else { p.green }, 0.08));
    } else {
        el = el.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
    }
    el.into_any_element()
}

/// The Auto cell: the pick's "-> BL" / "-> WL" tag for a suggested coin, empty otherwise.
fn auto_cell(auto: Option<bool>, p: MoonPalette, cx: &App) -> AnyElement {
    div()
        .w(design::font_w_px(cx, COIN_AUTO_W))
        .flex_none()
        .flex()
        .justify_center()
        .when_some(auto, |el, black| {
            let (bg, fg, label) = if black {
                (p.red, p.red_text, t!("analytics.coins.tag_to_bl"))
            } else {
                (p.green, p.green_text, t!("analytics.coins.tag_to_wl"))
            };
            el.child(
                div()
                    .px(design::ui_px(cx, 5.0))
                    .rounded(design::ui_px(cx, 3.0))
                    .bg(moon_alpha(bg, 0.16))
                    .text_color(moon(fg))
                    .font_family(design::ui_font())
                    .text_size(design::t_caption(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(label.to_string()),
            )
        })
        .into_any_element()
}

/// What the coin table's body shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoinsBody {
    /// No strategy selected: the table asks for one instead of listing every coin.
    NoStrategy,
    /// A read is loading or failed: its note.
    Note,
    /// Data, but the search and views left no row.
    NoMatch,
    Rows,
}

/// Decide the coin table's body.
///
/// Args:
///     selected: Whether a strategy is selected.
///     ready: Whether both reads are available.
///     drawn: Rows the model produced.
///
/// Returns:
///     The body variant to render.
fn coins_body(selected: bool, ready: bool, drawn: usize) -> CoinsBody {
    match (selected, ready, drawn) {
        (false, _, _) => CoinsBody::NoStrategy,
        (true, false, _) => CoinsBody::Note,
        (true, true, 0) => CoinsBody::NoMatch,
        (true, true, _) => CoinsBody::Rows,
    }
}

/// One line of the env-gated observation channel (see `analytics::probe_enabled`), written
/// only when what that surface shows CHANGES — a per-frame log would drown the signal it
/// exists to carry. Appends to `logs/analytics_probe.log`, beside every other channel file.
///
/// The GATE stays on the environment rather than moving into `cfg/diagnostics.toml`, because
/// `MOON_ANALYTICS_PROBE` also opens the Analytics window at startup: a checkbox in a settings
/// file must not open windows. The FILE still goes through the shared writer, so it lands where
/// the others do.
///
/// `key` names the surface, and the last line is remembered PER key: three surfaces sharing
/// one slot would each see the previous one's line, never match, and write on every frame —
/// the exact flood the deduplication is here to prevent.
pub(super) fn probe(key: &'static str, line: String) {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static LAST: OnceLock<Mutex<HashMap<&'static str, String>>> = OnceLock::new();
    let map = LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut seen) = map.lock() else { return };
    let last = seen.entry(key).or_default();
    if *last == line {
        return;
    }
    last.clone_from(&line);
    drop(seen);
    moon_core::diagnostics::channel_line("analytics_probe.log", &line);
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
/// Locale form of `base` for count `n`.
///
/// The Slavic rule lives in [`crate::display_text::plural_key`]. This adapter exists because the
/// tuner counts with `usize`.
///
/// Args:
///     base: The message key without its form suffix.
///     n: The count the message is about.
///
/// Returns:
///     The full key of the form to draw.
pub(in crate::analytics) fn plural_key(base: &str, n: usize) -> String {
    crate::display_text::plural_key(base, n as u64)
}

/// The empty-table sentence when rows exist but none is drawn: if the trade threshold hid
/// every coin, it names that rule instead of blaming the search.
fn no_match_text(hidden: usize) -> String {
    if hidden > 0 {
        let key = plural_key("analytics.coins.all_hidden", hidden);
        t!(key.as_str(), hidden = hidden).to_string()
    } else {
        t!("analytics.strat.no_match").to_string()
    }
}

#[cfg(test)]
mod tests;
