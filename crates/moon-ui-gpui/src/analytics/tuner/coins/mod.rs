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
pub(in crate::analytics::tuner) mod columns;
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

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonInput,
    MoonInputEvent, MoonInputState, MoonPalette, MoonSlider, MoonSliderEvent, MoonSliderState,
    h_flex, v_flex,
};
use rust_i18n::t;

use super::super::AnalyticsView;
use super::kpi::kpi_matrix_card;
use super::list::MAX_ROWS;
use super::{
    COIN_COLS, COIN_NAME_MIN_W, COIN_ROW_GAP, COIN_ROW_PAD_X, COIN_TICK_W, SORT_NAME, metric_cell,
    sort_arrow_of, toggle_sort_key,
};
use crate::design;
use crate::design::{moon, moon_alpha};
use moon_core::db::analytics::GroupStat;
use rows::{CoinCounts, CoinRow};
use state::{CoinFilter, coin_token};

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

    /// Lazily-created "keep coins above this share of the busiest" slider.
    ///
    /// A fixed 0–100 scale: as a share it means the same thing in every scope, so it never
    /// needs rebuilding when the period changes — which an absolute-trades scale did, since
    /// its ceiling can only be set at construction.
    fn coin_min_trades_slider(&mut self, cx: &mut Context<Self>) -> Entity<MoonSliderState> {
        if let Some(s) = &self.coins.min_trades_slider {
            return s.clone();
        }
        let cur = self.coins.min_trades_pct as f32;
        let state = cx.new(|_| {
            MoonSliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(cur)
        });
        cx.subscribe(&state, |this, _e, ev: &MoonSliderEvent, cx| {
            if let MoonSliderEvent::Change(v) = ev {
                let pct = v.end().round() as i64;
                if this.coins.min_trades_pct != pct {
                    this.coins.min_trades_pct = pct;
                    cx.notify();
                }
            }
        })
        .detach();
        self.coins.min_trades_slider = Some(state.clone());
        state
    }

    /// Search box + the All / blacklist / whitelist views, each with its count. The two
    /// list chips are disabled while no lists are known (no selection, several selected,
    /// or no strategy DB) — there is nothing to filter by, and an enabled chip that
    /// empties the table would read as "none of these coins are listed".
    fn coin_filter_bar(
        &self,
        search: &Entity<MoonInputState>,
        min_trades: &Entity<MoonSliderState>,
        counts: CoinCounts,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        // "matched / in the strategy's list": the rows only cover coins that traded
        // somewhere in the period, so a listed coin nobody traded has no row at all.
        // Showing the matches alone would understate the list the user is reading.
        let listed = |shown: usize, total: usize| {
            if shown == total {
                shown.to_string()
            } else {
                format!("{shown}/{total}")
            }
        };
        let cur = self.coins.filter;
        let chip = |id: &'static str, f: CoinFilter, label: String| {
            let on = cur == f;
            let enabled = self.coins.filter_available(f);
            MoonButton::new(id)
                .variant(if on {
                    MoonButtonVariant::Amber
                } else {
                    MoonButtonVariant::Soft
                })
                .size(MoonButtonSize::Micro)
                .selected(on)
                .disabled(!enabled)
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.coins.filter = f;
                    cx.notify();
                }))
                .render()
        };
        let (bl_n, wl_n) = (self.coins.work.black.len(), self.coins.work.white.len());
        h_flex()
            .w_full()
            .flex_none()
            .px(design::ui_px(cx, 8.0))
            .py(design::ui_px(cx, 4.0))
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .bg(moon(p.table_head))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(MoonInput::new("an-coin-search").state(search).small()),
            )
            // Right after the search: both narrow WHICH rows are worth looking at, so they
            // belong together, ahead of the chips that only switch the view.
            .child(
                div().w(design::font_w_px(cx, 84.0)).flex_none().child(
                    MoonSlider::new(min_trades)
                        .id("an-coin-min-trades")
                        .height(18.0),
                ),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, 62.0))
                    .flex_none()
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    // The share is what the slider is set to; the count in brackets is what it
                    // comes to in trades here — that is the number deciding which rows go, and
                    // it changes with the scope while the share does not.
                    .child(format!(
                        "{}% ({})",
                        self.coins.min_trades_pct, self.coins.min_trades
                    )),
            )
            .child(chip(
                "an-coin-f-all",
                CoinFilter::All,
                format!("{} {}", t!("report.filter.all"), counts.all),
            ))
            .child(chip(
                "an-coin-f-bl",
                CoinFilter::Black,
                format!(
                    "{} {}",
                    t!("analytics.coins.black"),
                    listed(counts.black, bl_n)
                ),
            ))
            .child(chip(
                "an-coin-f-wl",
                CoinFilter::White,
                format!(
                    "{} {}",
                    t!("analytics.coins.white"),
                    listed(counts.white, wl_n)
                ),
            ))
            .child(chip(
                "an-coin-f-ch",
                CoinFilter::Changed,
                format!("{} {}", t!("analytics.coins.changed"), counts.changed),
            ))
            .into_any_element()
    }

    /// Sortable heading of the coin table, aligned column-for-column with [`Self::coin_row`].
    fn coin_header(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement + use<> {
        let scale = design::font_scale(cx);
        let sortable =
            |id: SharedString, title: String, key: &'static str, w: Option<(f32, f32)>| {
                let arrow = sort_arrow_of(&self.coins.sort, key);
                let d = div()
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
                        cx.notify();
                    }));
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
            // The two list ticks close the row. They carry headings but no sort: they are
            // the EDIT surface, and a master tick over thousands of coins would be one
            // click into a list nobody meant to build.
            .child(
                div()
                    .w(design::font_w_px(cx, COIN_TICK_W))
                    .flex_none()
                    .text_center()
                    .child(t!("analytics.coins.black").to_string()),
            )
            .child(
                div()
                    .w(design::font_w_px(cx, COIN_TICK_W))
                    .flex_none()
                    .text_center()
                    .child(t!("analytics.coins.white").to_string()),
            )
    }

    /// One coin row. Each tick box carries its OWN handler: with two of them per row a
    /// row-wide click could not say which list it meant, so the row itself is inert.
    fn coin_row_el(
        &self,
        row: &CoinRow,
        p: MoonPalette,
        scale: f32,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let g: &GroupStat = &row.stat;
        let name = row.name.clone();
        // One tick box. It carries its own handler now: with two of them per row, a
        // row-wide toggle could not say WHICH list the click meant.
        let tick = |black_side: bool, on: bool| {
            // Folded HERE, not per row per frame: the token is needed once per click, and
            // building it for every one of thousands of rows on every frame was pure waste
            // — worse, skipping it for speed left the handler editing the empty token.
            let coin = name.to_string();
            div()
                .w(design::font_w_px(cx, COIN_TICK_W))
                .flex_none()
                .flex()
                .justify_center()
                .child(
                    MoonCheckbox::new(SharedString::from(format!(
                        "an-coin-{}-{name}",
                        if black_side { "bl" } else { "wl" }
                    )))
                    .checked(on)
                    .size(MoonCheckboxSize::Compact)
                    .on_change({
                        let view = cx.entity();
                        move |_, _w, app| {
                            let token = coin_token(&coin);
                            view.update(app, |this, cx| {
                                this.toggle_coin_list(black_side, token, cx);
                            });
                        }
                    }),
                )
        };
        let mut el = h_flex()
            .id(SharedString::from(format!("an-coinrow-{name}")))
            .w_full()
            .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
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
                    .child(name.to_string()),
            )
            .children(COIN_COLS.iter().map(|col| metric_cell(col, g, p, scale)))
            .child(tick(true, row.black))
            .child(tick(false, row.white))
            // Clicking the ROW asks "who traded this?" — the tick boxes keep their own
            // handlers, so a click on one of them never reaches here as a selection.
            .on_click(cx.listener({
                let coin = name.to_string();
                move |this, ev: &ClickEvent, _, cx| {
                    this.pick_coin(coin.clone(), ev.modifiers().secondary(), cx);
                }
            }))
            .cursor_pointer();
        // Two different things get two different colours: BLUE is "this is the coin the
        // strategy list is currently answering for", AMBER is "this row carries an unsaved
        // edit". Selection wins the background when both are true — it is the transient one,
        // and the edit is still shown by the ticks.
        if row.picked {
            el = el.bg(moon_alpha(p.blue, 0.18));
        } else if row.changed {
            el = el.bg(moon_alpha(p.amber, 0.12));
        } else {
            el = el.hover(move |s| s.bg(moon_alpha(p.panel_high, 0.9)));
        }
        el
    }

    /// The coin table card — sits UNDER the strategy list.
    pub(in crate::analytics::tuner) fn coins_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scale = design::font_scale(cx);
        let scope = self.scope_label();
        let search = self.coin_search_state(window, cx);
        // The share is fixed; what it COMES TO depends on the busiest coin of the loaded
        // scope, so it is resolved here, where that scope is known.
        let busiest = self
            .coins
            .stats
            .data()
            .map(|v| v.iter().map(|g| g.n).max().unwrap_or(0))
            .unwrap_or(0);
        self.coins.min_trades = self.coins.trades_floor(busiest);
        let min_trades = self.coin_min_trades_slider(cx);
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
        let (body, counts, total) = match ready {
            Err(note) => (
                super::super::note_el("an-coins-note", note, 10.0, p, cx),
                CoinCounts::default(),
                0usize,
            ),
            Ok((base, scoped)) => {
                // Built once per change of what it depends on — NOT per frame. Hovering a
                // row repaints the whole view without touching any of those inputs.
                let (counts, total, empty) = {
                    let cache = rows::rows_for(&mut self.coins, &base.coins, &scoped);
                    (cache.counts, cache.total, cache.rows.is_empty())
                };
                if empty {
                    // Data exists but the search/filter matched nothing — say so instead of
                    // leaving a blank area.
                    let note = div()
                        .w_full()
                        .p(design::ui_px(cx, 18.0))
                        .text_center()
                        .text_color(moon(p.text_muted))
                        .child(t!("analytics.strat.no_match").to_string())
                        .into_any_element();
                    (note, counts, 0)
                } else {
                    let cache = self
                        .coins
                        .rows
                        .as_ref()
                        .expect("rows_for populates the cache above");
                    let rows: Vec<AnyElement> = cache
                        .rows
                        .iter()
                        .map(|r| self.coin_row_el(r, p, scale, cx).into_any_element())
                        .collect();
                    (
                        v_flex().w_full().children(rows).into_any_element(),
                        counts,
                        total,
                    )
                }
            }
        };
        // Derived, never carried: the two cannot drift out of step.
        let shown = total.min(MAX_ROWS);
        let changed = self.coins.has_changes();
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
                            .text_size(design::t_title(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("analytics.tab.coins").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(scope),
                    )
                    .when(changed, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.amber))
                                .child(
                                    t!("analytics.coins.changed_n", n = self.coins.changed_count())
                                        .to_string(),
                                ),
                        )
                        .child(
                            MoonButton::new("an-coin-revert")
                                .variant(MoonButtonVariant::Soft)
                                .size(MoonButtonSize::Micro)
                                .label(t!("analytics.coins.revert").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.coins.revert();
                                    this.coins.settle_filter();
                                    // Through the SAME debounced path as a tick: reverting
                                    // is one more edit of the same lists.
                                    this.arm_coin_kpi(cx);
                                    cx.notify();
                                }))
                                .render(),
                        )
                    })
                    .when(total > shown, |el| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(design::t_caption(cx))
                                .text_color(moon(p.text_muted))
                                .child(
                                    t!("analytics.strat.shown", shown = shown, total = total)
                                        .to_string(),
                                ),
                        )
                    }),
            )
            .child(self.coin_filter_bar(&search, &min_trades, counts, p, cx))
            .child(self.coin_header(p, cx))
            .child(
                div()
                    .id("an-coins-list")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }

    /// "Fact vs v1" for the coin axis — the SHARED matrix, v1 being the ticked coins.
    pub(in crate::analytics::tuner) fn coins_kpi(
        &self,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        // Titled with the count the SHOWN matrix was computed for, not the live one: a
        // recompute keeps the previous columns on screen, and the live count would label
        // them with a pick set they were never scored under.
        // Title and composition on separate lines: the composition grows with the lists, and
        // as one string it wrapped inside the fixed column width.
        let labels = vec![super::kpi::VarLabel::with_sub(
            t!("analytics.coins.v1").to_string(),
            t!(
                "analytics.coins.v1_lists",
                bl = self.coins.kpi_bl,
                wl = self.coins.kpi_wl
            )
            .to_string(),
        )];
        kpi_matrix_card(
            &self.coins.kpi,
            self.scope_label(),
            &labels,
            self.kpi_collapsed,
            p,
            cx,
        )
    }
}

/// One line of the env-gated observation channel (see `analytics::probe_enabled`), written
/// only when what that surface shows CHANGES — a per-frame log would drown the signal it
/// exists to carry. Appends to `analytics_probe.log` next to the executable, like
/// `render_diag.log`.
///
/// `key` names the surface, and the last line is remembered PER key: three surfaces sharing
/// one slot would each see the previous one's line, never match, and write on every frame —
/// the exact flood the deduplication is here to prevent.
pub(super) fn probe(key: &'static str, line: String) {
    use std::collections::HashMap;
    use std::io::Write;
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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("analytics_probe.log")
    {
        let _ = writeln!(f, "{line}");
    }
}
