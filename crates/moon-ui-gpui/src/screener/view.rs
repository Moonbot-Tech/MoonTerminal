//! Screener state and window lifecycle.
//!
//! [`ScreenerView`] owns filters, sorting, cached-row rebuilds, window rendering, and the `open`
//! entry point. Column definitions, value sorting, and row rendering live in [`super::table`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButtonVariant, MoonDataTable, MoonDataTableColumn,
    MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;

use crate::panels::{RenderGate, data_table_host};
use crate::workspace::scope_marker::{self, ScopeMarker};
use crate::{Backend, design};

use super::table::{COLS, ColDef, Entry, column_title, moon, moon_alpha, parse_vol, screener_row};

const SCREENER_HEADER_H: f32 = 32.0;

/// Core source filter: every core or one specific core, matching the Orders source selector.
#[derive(Clone, Copy, PartialEq)]
enum ScrSource {
    All,
    Core(CoreId),
}

/// State for the singleton Screener window.
pub struct ScreenerView {
    pub(super) backend: Entity<Backend>,
    /// Cached rows after the current source, coin, volume, and sort selections are applied.
    rows: Rc<Vec<usize>>,
    /// Source rows and their dense counting identities are published together.
    prepared: std::sync::Arc<super::prepared::Prepared>,
    /// A newer refresh retires the previous publication task.
    prepare_task: Option<Task<()>>,
    /// Publication identity independent of user controls, which apply to either snapshot.
    prepare_seq: u64,
    /// Distinct markets after source grouping but before the coin and volume filters, the footer's
    /// "total". Markets, not rows: a market has one row per core sharing its exchange.
    total: usize,
    /// Distinct markets among [`Self::rows`], the footer's "shown" — counted the same way.
    shown: usize,
    /// Workspace scope marker over every live session, or `None` when the preset is unresolved.
    /// A resolved full scope still keeps a marker whose facts are empty. Screener is a group-less
    /// singleton with no `EffectiveCoreScope` of its own, so this is built with
    /// `ScopeMarker::from_membership` instead of `Backend::new`.
    scope_marker: Option<ScopeMarker>,
    /// Live session count, the universe `scope_marker` was built over. Distinguishes "no session
    /// at all" (the original connect-a-core advice still applies) from "sessions exist but no
    /// rows survived filtering" (a different, non-actionable empty sentence).
    sessions_total: usize,
    /// Case-insensitive market or coin substring filter.
    coin_input: Entity<MoonInputState>,
    /// Minimum 24-hour volume filter, accepting `k` and `m` suffixes.
    dvol_input: Entity<MoonInputState>,
    /// Sort key from `COLS` and descending/ascending direction.
    sort_key: String,
    sort_desc: bool,
    /// Session-only core filter, not persisted, as in Orders.
    source: ScrSource,
    /// Visible `COLS` keys persisted in per-context table storage.
    ///
    /// Legacy `layout.screener_columns` is read only as a migration fallback.
    visible_cols: HashSet<String>,
    /// Retained table state for drag order, column widths, and the sort indicator.
    table_state: Entity<MoonDataTableState>,
    /// Order this table last observed. A resize or a header click notifies the same observer
    /// as a drag, and must not write an order that did not move.
    col_order_cache: Vec<SharedString>,
    gate: RenderGate,
    focus: FocusHandle,
}

impl ScreenerView {
    /// Retain control state while the first market publication is prepared off-thread.
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let coin_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC"));
        let dvol_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("500k"));
        for input in [&coin_input, &dvol_input] {
            cx.subscribe(input, |this: &mut Self, _, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    this.rebuild(cx);
                    cx.notify();
                }
            })
            .detach();
        }

        // Screener always runs in a separate window, so its width context is fixed to `:win`.
        let widths_id = crate::persistence::table_persist::ctx_id("screener-table", true);

        // Prefer the shared per-context `table_visible_columns` entry for `screener-table:win`.
        // When absent, migrate from legacy `screener_columns`. Ignore unknown keys left by renamed
        // or removed columns; an empty valid set falls back to every current column.
        //
        // A layout saved before the PnL column was renamed keys it `session`, a key that is now
        // retired; the list, the sort and the widths all bring it current on read (see
        // `table::current_key`).
        let saved_cols = crate::persistence::table_persist::visible(backend.read(cx), &widths_id)
            .or_else(|| backend.read(cx).layout.screener_columns.clone())
            .map(super::table::migrate_legacy_keys);
        let visible_cols: HashSet<String> = match saved_cols {
            Some(list) => {
                let set: HashSet<String> = list
                    .iter()
                    .filter(|k| COLS.iter().any(|c| c.0 == k.as_str()))
                    .cloned()
                    .collect();
                if set.is_empty() {
                    COLS.iter().map(|c| c.0.to_string()).collect()
                } else {
                    set
                }
            }
            None => COLS.iter().map(|c| c.0.to_string()).collect(),
        };
        let restored_sort = super::table::restore_sort(
            super::table::migrate_legacy_sort(crate::persistence::table_persist::saved_sort(
                backend.read(cx),
                &widths_id,
            )),
            &visible_cols,
        );
        let saved_widths = super::table::migrate_legacy_widths(
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id),
        );
        let screener_keys: Vec<&str> = COLS.iter().map(|column| column.0).collect();
        let saved_order = crate::persistence::table_persist::restored_order(
            backend.read(cx),
            &widths_id,
            &screener_keys,
        );
        let col_order_cache = saved_order.clone();
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.set_sort(restored_sort.0.clone(), !restored_sort.1);
            s.column_widths = saved_widths;
            s.column_order = saved_order;
            s
        });
        // Column resizing and dragging mutate table state. Order uses its own helper so a panel
        // that already stores order elsewhere is not given a second copy. The cache skips a
        // notification that did not move the columns.
        let widths_id_obs = widths_id.clone();
        cx.observe(&table_state, move |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &widths_id_obs, &state, cx);
            crate::persistence::table_persist::persist_order(
                &this.backend,
                &widths_id_obs,
                &state,
                &mut this.col_order_cache,
                cx,
            );
        })
        .detach();

        // Market-only feed changes publish new snapshots without notifying `Backend`. Notifications
        // caused by other UI-state events provide opportunities to sample the latest snapshots. With
        // a constant signature, the gate accepts at most one rebuild per wall-clock second when such
        // notifications occur; there is no independent timer.
        cx.observe(&backend, |this, _, cx| {
            let now = moon_chart::paint::now_unix_ms();
            if this.gate.should_notify(0, now) {
                this.rebuild(cx);
                cx.notify();
            }
        })
        .detach();

        // A membership-only save advances `workspace_revision` without notifying `Backend`, so the
        // gate above never fires for it; observe the revision directly to pick up the new preset.
        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            this.rebuild(cx);
            cx.notify();
        })
        .detach();

        // Persist window position and size in the layout so the window reopens in the same geometry.
        cx.observe_window_bounds(window, |this, window, cx| {
            let geom = crate::window::windowing::window_geom_rect(window, cx);
            this.backend.update(cx, |b, _| {
                let geom = geom.keeping_display_of(b.layout.screener_window);
                if b.layout.screener_window != Some(geom) {
                    b.layout.screener_window = Some(geom);
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        let mut this = Self {
            backend,
            rows: Rc::new(Vec::new()),
            prepared: Default::default(),
            prepare_task: None,
            prepare_seq: 0,
            total: 0,
            shown: 0,
            scope_marker: None,
            sessions_total: 0,
            coin_input,
            dvol_input,
            sort_key: restored_sort.0,
            sort_desc: restored_sort.1,
            source: ScrSource::All,
            visible_cols,
            table_state,
            col_order_cache,
            gate: RenderGate::default(),
            focus: cx.focus_handle(),
        };
        this.rebuild(cx);
        this
    }

    /// Prepare a coherent feed publication off-thread; the current controls remain immediate.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        crate::diag::bump(&crate::diag::SCREENER_REBUILD);
        self.filter_cached(cx);
        let b = self.backend.read(cx);
        let source = b.session.market_source();
        let mut groups: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
        let mut names = HashMap::new();
        let mut orders = HashMap::new();
        let store = b.session.store();
        for session in b.session.sessions() {
            names.insert(session.id, SharedString::from(session.name.clone()));
            if let Some(data) = store.core(session.id) {
                let mut counts: HashMap<&str, u32> = HashMap::new();
                for order in &data.orders {
                    *counts.entry(order.market.as_str()).or_default() += 1;
                }
                orders.insert(
                    session.id,
                    counts
                        .into_iter()
                        .map(|(market, count)| (market.to_string(), count))
                        .collect::<HashMap<_, _>>(),
                );
            }
            let Some(provider) = source.provider_of(session.id) else {
                continue;
            };
            match groups.iter_mut().find(|(id, _)| *id == provider) {
                Some((_, members)) => members.push(session.id),
                None => groups.push((provider, vec![session.id])),
            }
        }
        self.prepare_seq = self.prepare_seq.wrapping_add(1);
        let seq = self.prepare_seq;
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;
        self.prepare_task = Some(cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_spawn(async move {
                    let mut entries = Vec::new();
                    for (provider, members) in groups {
                        let mut rows = source.screener_rows(provider, &members);
                        for row in &mut rows {
                            row.orders = orders
                                .get(&row.core)
                                .and_then(|counts| counts.get(row.market.as_str()))
                                .copied()
                                .unwrap_or(0);
                        }
                        entries.extend(entries_for(rows, &names));
                    }
                    std::sync::Arc::new(super::prepared::Prepared::new(
                        entries, &sort_key, sort_desc,
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.prepare_seq != seq {
                    return;
                }
                this.prepared = prepared;
                // The source snapshot is unfiltered: completion always applies the LATEST
                // source, scope, text, volume and sort, never the controls a job started with.
                this.filter_cached(cx);
                cx.notify();
            });
        }));
    }

    /// Filter a prepared publication using the live preset and controls without cloning rows.
    fn filter_cached(&mut self, cx: &mut Context<Self>) {
        let coin = self.coin_input.read(cx).value().trim().to_uppercase();
        let min_vol = parse_vol(self.dvol_input.read(cx).value().trim());
        let b = self.backend.read(cx);
        let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
        let sessions = b.session.sessions();
        self.sessions_total = sessions.len();
        let displayed: HashMap<_, _> = sessions
            .iter()
            .enumerate()
            .map(|(rank, session)| {
                (
                    session.id,
                    b.core_displayed(preset, session.id).then_some(rank),
                )
            })
            .collect();
        self.scope_marker = ScopeMarker::from_membership(
            preset,
            sessions
                .iter()
                .map(|session| displayed[&session.id].is_some()),
        );
        if let ScrSource::Core(only) = self.source
            && !b.core_displayed(preset, only)
        {
            self.source = ScrSource::All;
        }
        let allowed: Vec<_> = self
            .prepared
            .cores
            .iter()
            .map(|core| {
                displayed
                    .get(core)
                    .copied()
                    .flatten()
                    .filter(|_| match self.source {
                        ScrSource::All => true,
                        ScrSource::Core(only) => only == *core,
                    })
            })
            .collect();
        let (rows, total, shown) =
            self.prepared
                .select(&allowed, &coin, min_vol, &self.sort_key, self.sort_desc);
        self.rows = Rc::new(rows);
        self.total = total;
        self.shown = shown;
    }

    /// Apply the header order immediately and refresh its source data for the next publication.
    fn set_sort(&mut self, key: &str, desc: bool, cx: &mut Context<Self>) {
        let next = super::table::restore_sort(
            Some(moon_core::config::TableSortPreference {
                column: key.to_string(),
                ascending: !desc,
            }),
            &self.visible_cols,
        );
        self.sort_key = next.0;
        self.sort_desc = next.1;
        let indicator_key = self.sort_key.clone();
        let indicator_ascending = !self.sort_desc;
        self.table_state.update(cx, move |state, _| {
            state.set_sort(indicator_key, indicator_ascending)
        });
        let preference = (self.sort_key != "vol24" || !self.sort_desc).then(|| {
            moon_core::config::TableSortPreference {
                column: self.sort_key.clone(),
                ascending: !self.sort_desc,
            }
        });
        let id = crate::persistence::table_persist::ctx_id("screener-table", true);
        crate::persistence::table_persist::set_sort(&self.backend, &id, preference, cx);
        self.rebuild(cx);
        cx.notify();
    }

    /// Change the session-only core filter while preserving control-triggered source sampling.
    fn set_source(&mut self, source: ScrSource, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// Toggle a column and persist the visible keys in canonical `COLS` order.
    ///
    /// The last visible column cannot be hidden because that would empty the table.
    fn toggle_col(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.visible_cols.contains(key) {
            if self.visible_cols.len() == 1 {
                return;
            }
            self.visible_cols.remove(key);
        } else {
            self.visible_cols.insert(key.to_string());
        }
        self.persist_visible_cols(cx);
        if !self.visible_cols.contains(&self.sort_key) {
            let hidden_sort = self.sort_key.clone();
            self.set_sort(&hidden_sort, self.sort_desc, cx);
        }
    }

    /// Toggle every column on, or retain only the first when all are already visible.
    ///
    /// Keeping one column prevents the table from becoming empty.
    fn toggle_all_cols(&mut self, cx: &mut Context<Self>) {
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        self.visible_cols = if all_on {
            std::iter::once(COLS[0].0.to_string()).collect()
        } else {
            COLS.iter().map(|c| c.0.to_string()).collect()
        };
        self.persist_visible_cols(cx);
        if !self.visible_cols.contains(&self.sort_key) {
            let hidden_sort = self.sort_key.clone();
            self.set_sort(&hidden_sort, self.sort_desc, cx);
        }
    }

    /// Persist visible column keys in canonical `COLS` order.
    fn persist_visible_cols(&self, cx: &mut Context<Self>) {
        let list: Vec<String> = COLS
            .iter()
            .filter(|c| self.visible_cols.contains(c.0))
            .map(|c| c.0.to_string())
            .collect();
        // Screener has only the `screener-table:win` context because it is always a window. Store an
        // explicit key list, including the complete list when all columns are visible. Legacy
        // `screener_columns` is no longer written and is used only during read-side migration.
        let id = crate::persistence::table_persist::ctx_id("screener-table", true);
        crate::persistence::table_persist::set_visible(&self.backend, &id, list, cx);
        cx.notify();
    }

    /// Request the row's market on Main for the row's own core, as in an Orders token click.
    ///
    /// The request does not activate or raise the owning window.
    fn open_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(e) = self
            .rows
            .get(ix)
            .and_then(|&index| self.prepared.entries.get(index))
        else {
            return;
        };
        let core = e.row.core;
        let market = e.row.market.clone();
        self.backend.update(cx, |b, bcx| {
            b.open_on_main((core, market), false);
            bcx.notify();
        });
    }

    /// Render each virtual row from the same publication that owns its selected index.
    fn table(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = self.rows.clone();
        let row_count = rows.len();
        let view = cx.entity();
        let table_rows = rows.clone();
        let prepared = self.prepared.clone();
        let sort_view = cx.entity();
        let dbl_view = cx.entity();
        let state_reset = self.table_state.clone();
        // Share one canonically ordered visible-column list between the header and row renderer.
        let visible: Rc<Vec<&'static ColDef>> = Rc::new(
            COLS.iter()
                .filter(|c| self.visible_cols.contains(c.0))
                .collect(),
        );
        let row_cols = visible.clone();

        data_table_host(
            "screener-table-host",
            row_count == 0,
            // Three-way choice, and the order matters: `scope_empty_text` takes precedence over
            // both fallbacks, so a hidden-by-preset Screener never reaches either sentence below
            // it. Of the two genuine reasons, only "no session at all" asks for an action — a
            // core hidden by the preset, or connected with no book yet, must not tell the user to
            // go connect something that is already connected.
            scope_marker::scope_empty_text(
                self.scope_marker.as_ref(),
                if self.sessions_total == 0 {
                    t!("screener.empty").to_string()
                } else {
                    t!("screener.empty_no_data").to_string()
                },
            ),
            p,
            cx,
            MoonDataTable::new("screener-table", row_count, move |ix, _window, _app| {
                screener_row(&prepared.entries[table_rows[ix]], &view, p, &row_cols)
            })
            .columns(
                visible
                    .iter()
                    .map(|&&(key, title, width, right)| {
                        let col = MoonDataTableColumn::new(key, title, width).sortable(true);
                        let col = if right { col.right() } else { col };
                        if key == "market" {
                            col.fixed_left()
                        } else {
                            col
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .state(&self.table_state)
            .style(design::table_style(p))
            .on_sort(move |key, ascending, _window, app| {
                let key = key.to_string();
                sort_view.update(app, |t, cx| t.set_sort(&key, !ascending, cx));
            })
            .on_double_click_row(move |ix, _window, app| {
                dbl_view.update(app, |t, cx| t.open_chart(ix, cx));
            })
            .on_select_row(move |_ix, _window, app| {
                // Row selection is unused, as in Orders, so clear every selection field immediately.
                state_reset.update(app, |s, c| {
                    s.selected_row = None;
                    s.selected_column = None;
                    s.selected_cell = None;
                    c.notify();
                });
            }),
        )
    }

    /// Build the All-cores plus exchange-grouped session-core source selector.
    ///
    /// A disconnected selected core remains visible as its numeric id until the user chooses a
    /// current source, rather than being mislabeled as All cores.
    ///
    /// Args:
    ///     cx: Screener context used to read sessions, exchanges, and selection state.
    ///
    /// Returns:
    ///     The configured source dropdown.
    fn source_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let (cores, venues) = {
            let b = self.backend.read(cx);
            let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
            (
                moon_core::session::core_order::CoreOrder::new(&b.config)
                    .from_sessions(b.session.sessions(), |s| b.core_displayed(preset, s.id)),
                b.session.core_venues(),
            )
        };
        // Resolve the localized All-cores label once for every use in this selector.
        let all_label = t!("screener.all_cores").to_string();
        let cur = match self.source {
            ScrSource::All => all_label.clone(),
            ScrSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| format!("#{id}")),
        };
        let sections = crate::controls::core_menu_sections(&cores, venues);
        let view = cx.entity();
        let all_view = view.clone();
        let mut items = vec![
            MoonMenuItem::with_key("all", all_label.clone())
                .checked(self.source == ScrSource::All)
                .on_click(move |_, _, app| {
                    all_view.update(app, |this, cx| this.set_source(ScrSource::All, cx));
                }),
        ];
        if !sections.is_empty() {
            items.push(MoonMenuItem::separator());
        }
        for (venue, members) in &sections {
            let exchange_label = crate::controls::venue_section_label(*venue);
            items.push(MoonMenuItem::label(exchange_label));
            for (id, name) in members {
                let id = *id;
                let item_view = view.clone();
                items.push(
                    MoonMenuItem::with_key(format!("core-{id}"), *name)
                        .checked(self.source == ScrSource::Core(id))
                        .on_click(move |_, _, app| {
                            item_view
                                .update(app, |this, cx| this.set_source(ScrSource::Core(id), cx));
                        }),
                );
            }
        }
        MoonDropdown::new("screener-source")
            .label(cur)
            .trigger_caret(true)
            .trigger_variant(MoonButtonVariant::Soft)
            // Same lower bound as every other core selector; it grows past it for a long core name.
            .fit_trigger_width(crate::controls::CORE_COMBO_TRIGGER_W, 260.0)
            .fit_menu_width(160.0, 560.0)
            .menu_max_height_ui(360.0)
            .items(items)
    }

    /// Build the persistent visible-column menu with checkbox-style toggles.
    ///
    /// The menu remains open after clicks, and its last visible column is disabled so the table
    /// cannot be emptied.
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible_count = self.visible_cols.len();
        let mut menu = MoonDropdown::new("screener-columns")
            // The shared column-selector icon button instead of a text-field trigger; the asset
            // and the childless trigger are `design::COLUMN_SELECTOR_ICON`'s contract.
            .trigger_icon(design::COLUMN_SELECTOR_ICON)
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_width(design::glyph_btn_w(cx))
            .fit_menu_width(170.0, 560.0)
            .close_on_select(false);
        // The All item enables every column or, when already checked, leaves only the first.
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, cx| t.toggle_all_cols(cx));
                }),
        );
        for &(key, title, ..) in COLS {
            let shown = self.visible_cols.contains(key);
            let last_visible = shown && visible_count == 1;
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{key}"), title)
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, cx| t.toggle_col(key, cx));
                    }),
            );
        }
        div()
            .id("screener-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("screener.columns").to_string()))
                    .into()
            })
            .child(menu)
    }

    /// Build the bottom bar with Moonbot-style source, Market, and Vol. controls on the left and the
    /// filtered/total count plus column menu on the right.
    fn bottom_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_soft))
                .child(text)
        };
        // Frozen render idiom (`workspace::scope_marker`): head is the existing formatted count
        // and never clips; the marker's facts trail it, clipping at their right edge when the
        // window narrows, exactly as a dock footer's fixed height requires.
        let count_head = format!("{} / {} {}", self.shown, self.total, t!("screener.coins"));
        let footer = scope_marker::scope_footer(count_head, self.scope_marker.as_ref());
        let footer_tip = scope_marker::scope_footer_tooltip(&footer, self.scope_marker.as_ref());
        let has_tail = !footer.tail.is_empty();
        // The shared helper draws nothing when nothing is hidden, which is what keeps this row's
        // child count, gaps and shrink behaviour identical to its pre-feature form.
        let footer_tail = scope_marker::scope_footer_tail(
            "screener-footer-tail",
            footer.tail,
            footer_tip,
            design::t_body(cx),
            p.text_muted,
        );
        h_flex()
            .flex_none()
            .w_full()
            .h(design::fit_h_px(cx, 30.0, 14.0, 9.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .bg(moon(p.shell_high))
            .border_t(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .child(self.source_combo(cx))
            .child(label(column_title("market")))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-coin")
                        .state(&self.coin_input)
                        .size(design::INPUT_SIZE)
                        .cleanable(true),
                ),
            )
            .child(label(column_title("vol24")))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-dvol")
                        .state(&self.dvol_input)
                        .size(design::INPUT_SIZE)
                        .cleanable(true),
                ),
            )
            .child(div().flex_1())
            // Head and tail are DIRECT children of the row, never nested in one shared wrapper —
            // nested, the wrapper would take a zero flex basis while the `flex_none` head keeps
            // painting at full width, straight over the tail.
            .child(
                div()
                    // `flex_none` only while a tail exists to yield in its place. Pinning the count
                    // unconditionally would take space from this bar's inputs at a narrow width on
                    // behalf of a marker that is not on screen.
                    .when(has_tail, |el| el.flex_none())
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(footer.head),
            )
            .children(footer_tail)
            .child(self.columns_menu(cx))
    }
}

impl EventEmitter<()> for ScreenerView {}
impl Focusable for ScreenerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ScreenerView {
    /// Render this window with native minimize, maximize, and close controls.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = crate::window::windowing::responsive_width(window);
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(screener_header(p, cx))
            .child(self.table(cx))
            .child(self.bottom_bar(p, cx))
            .child(
                MoonWindowFrame::tool("screener-window-frame-hit", chrome_width)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SCREENER_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

/// Normalize a provider's markets once while preserving member order and account identity.
fn entries_for(
    rows: Vec<moon_core::market::SharedScreenerRow>,
    names: &HashMap<CoreId, SharedString>,
) -> Vec<Entry> {
    use std::sync::Arc;
    let mut search_text: HashMap<usize, (Arc<str>, Arc<str>)> = HashMap::new();
    rows.into_iter()
        .map(|row| {
            let (market_uc, coin_uc) = search_text
                .entry(Arc::as_ptr(&row.market_data) as usize)
                .or_insert_with(|| {
                    (
                        row.market.to_uppercase().into(),
                        row.coin.to_uppercase().into(),
                    )
                })
                .clone();
            let core_name = names
                .get(&row.core)
                .cloned()
                .unwrap_or_else(|| SharedString::from(format!("#{}", row.core)));
            Entry {
                row,
                core_name,
                market_uc,
                coin_uc,
            }
        })
        .collect()
}

/// Match an already-normalized filter against cached market and coin search text.
pub(super) fn matches_coin_filter(entry: &Entry, filter_uc: &str) -> bool {
    entry.market_uc.contains(filter_uc) || entry.coin_uc.contains(filter_uc)
}

#[cfg(test)]
pub(super) mod tests;

/// Build the window header with native minimize, maximize, and close controls.
fn screener_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("screener-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SCREENER_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("screener-titlebar-title", 0.0)
                .title_cluster(t!("screener.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("screener-window-frame-visual", 0.0)
                    .controls(moon_ui::MoonWindowFrameControls::MinimizeMaximizeClose)
                    .header_height(SCREENER_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Open the singleton Screener tool window, focusing the existing backend-tracked handle when valid.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Focus an existing live window instead of opening a duplicate.
    if let Some(handle) = backend.read(cx).screener_window
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
    }
    let saved = backend.read(cx).layout.screener_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(100.0), px(80.0)),
            size: size(px(1500.0), px(760.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Choose a display from saved geometry when supported, otherwise fall back to the owner display.
    let display_id = crate::window::windowing::saved_or_owner_display_id(
        saved.and_then(|g| g.display_uuid),
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::window::windowing::tool_window_options(
        t!("screener.window_title").to_string(),
        crate::window::windowing::restored_window_bounds(
            saved,
            crate::window::windowing::reachable_window_bounds(bounds, display_id, None, cx),
        ),
        Some(size(px(900.0), px(480.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::window::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| ScreenerView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.screener_window = Some(handle));
        crate::window::windowing::activate_new_window(handle.into(), cx);
    }
}
