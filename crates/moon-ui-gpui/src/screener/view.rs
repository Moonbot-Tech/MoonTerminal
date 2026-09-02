//! Screener state and window lifecycle.
//!
//! [`ScreenerView`] owns filters, sorting, cached-row rebuilds, window rendering, and the `open`
//! entry point. Column definitions, value sorting, and row rendering live in [`super::table`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButtonSize, MoonButtonVariant, MoonDataTable, MoonDataTableColumn,
    MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;

use crate::panels::{RenderGate, data_table_host};
use crate::workspace::scope_marker::{self, ScopeMarker};
use crate::{Backend, design};

use super::table::{
    COLS, ColDef, Entry, column_title, moon, moon_alpha, parse_vol, screener_row, sort_entries,
};

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
    rows: Rc<Vec<Entry>>,
    /// Row count after source grouping but before coin and volume filters, shown in the footer.
    total: usize,
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
    gate: RenderGate,
    focus: FocusHandle,
}

impl ScreenerView {
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
        let saved_cols = crate::persistence::table_persist::visible(backend.read(cx), &widths_id)
            .or_else(|| backend.read(cx).layout.screener_columns.clone());
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
            crate::persistence::table_persist::saved_sort(backend.read(cx), &widths_id),
            &visible_cols,
        );
        let saved_widths = crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.set_sort(restored_sort.0.clone(), !restored_sort.1);
            s.column_widths = saved_widths;
            s
        });
        // Column resizing mutates table state; persist widths through the shared storage.
        let widths_id_obs = widths_id.clone();
        cx.observe(&table_state, move |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &widths_id_obs, &state, cx);
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
            total: 0,
            scope_marker: None,
            sessions_total: 0,
            coin_input,
            dvol_input,
            sort_key: restored_sort.0,
            sort_desc: restored_sort.1,
            source: ScrSource::All,
            visible_cols,
            table_state,
            gate: RenderGate::default(),
            focus: cx.focus_handle(),
        };
        this.rebuild(cx);
        this
    }

    /// Rebuild cached rows by grouping cores, overlaying orders, filtering, and sorting.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        crate::diag::bump(&crate::diag::SCREENER_REBUILD);
        let coin_filter = self.coin_input.read(cx).value().trim().to_uppercase();
        let min_vol = parse_vol(self.dvol_input.read(cx).value().trim());

        let b = self.backend.read(cx);
        let source = b.session.market_source();
        // Screener is a group-less singleton, following the Strategies-window pattern (`mod.rs`),
        // so it inherits the last focused group's preset, Auto or Classic, rather than owning a
        // group of its own.
        let preset = b.display_preset(crate::workspace::DisplayOwner::Singleton);
        // The universe is LIVE SESSIONS, and that is load-bearing: it is exactly the universe the
        // group-building loop below filters with the same `core_displayed(preset, s.id)` call. A
        // different universe (e.g. connected-only) would make the marker's `N of M` describe a set
        // the table never drew rows from.
        //
        // Answered ONCE per session and reused by the grouping loop below: `core_displayed` scans
        // `config.servers` linearly, so asking twice per session doubles that scan on every
        // rebuild for an answer that cannot have changed in between.
        let sessions = b.session.sessions();
        self.sessions_total = sessions.len();
        let displayed: Vec<bool> = sessions
            .iter()
            .map(|s| b.core_displayed(preset, s.id))
            .collect();
        self.scope_marker = ScopeMarker::from_membership(preset, displayed.iter().copied());
        // A retained core filter can point at a core the current preset just hid; keep it selected
        // and it filters every group away, producing an unexplained empty view. Fall back to the
        // all-cores view instead, same shape as `Backend::valid_auto_workspace_core`.
        if let ScrSource::Core(only) = self.source
            && !b.core_displayed(preset, only)
        {
            self.source = ScrSource::All;
        }
        // Deduplicate by market-data provider: each group contains the consumer cores sharing that
        // provider. With a core filter, the group contains only that core for account-derived fields,
        // while market rows still come from its provider and charts open on the selected core.
        let mut groups: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
        let mut names: HashMap<CoreId, SharedString> = HashMap::new();
        for s in b.session.sessions() {
            if !b.core_displayed(preset, s.id) {
                continue;
            }
            names.insert(s.id, SharedString::from(s.name.clone()));
            if let ScrSource::Core(only) = self.source {
                if s.id != only {
                    continue;
                }
            }
            let Some(provider) = source.provider_of(s.id) else {
                continue;
            };
            match groups.iter_mut().find(|(p, _)| *p == provider) {
                Some((_, members)) => members.push(s.id),
                None => groups.push((provider, vec![s.id])),
            }
        }

        let store = b.session.store();
        let mut entries: Vec<Entry> = Vec::new();
        for (provider, members) in &groups {
            let mut rows = source.screener_rows(*provider, members);
            // Overlay open-order counts by exact market, summed across the group's member cores.
            let mut order_counts: HashMap<&str, u32> = HashMap::new();
            let member_data: Vec<_> = members.iter().filter_map(|&m| store.core(m)).collect();
            for cd in &member_data {
                for o in &cd.orders {
                    *order_counts.entry(o.market.as_str()).or_default() += 1;
                }
            }
            for row in &mut rows {
                row.orders = order_counts.get(row.market.as_str()).copied().unwrap_or(0);
            }
            // Use the selected core for the Core column and chart actions under a source filter;
            // otherwise use the group's market-data provider.
            let open_core = match self.source {
                ScrSource::Core(id) => id,
                ScrSource::All => *provider,
            };
            let core_name = names
                .get(&open_core)
                .cloned()
                .unwrap_or_else(|| SharedString::from(format!("#{open_core}")));
            entries.extend(rows.into_iter().map(|row| Entry {
                row,
                core_name: core_name.clone(),
                open_core,
            }));
        }

        self.total = entries.len();
        if !coin_filter.is_empty() {
            entries.retain(|e| {
                e.row.market.to_uppercase().contains(&coin_filter)
                    || e.row.coin.to_uppercase().contains(&coin_filter)
            });
        }
        if min_vol > 0.0 {
            entries.retain(|e| e.row.vol_24h >= min_vol);
        }
        sort_entries(&mut entries, &self.sort_key, self.sort_desc);
        self.rows = Rc::new(entries);
    }

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

    /// Request the row's market on Main for its selected/provider core, as in an Orders token click.
    ///
    /// The request does not activate or raise the owning window.
    fn open_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(e) = self.rows.get(ix) else {
            return;
        };
        let core = e.open_core;
        let market = e.row.market.clone();
        self.backend.update(cx, |b, bcx| {
            b.open_on_main((core, market), false);
            bcx.notify();
        });
    }

    fn table(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = self.rows.clone();
        let row_count = rows.len();
        let view = cx.entity();
        let table_rows = rows.clone();
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
                screener_row(&table_rows[ix], &view, p, &row_cols)
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
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
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
                crate::core_order::CoreOrder::new(&b.config)
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
        let sections = crate::controls::core_menu_sections(&cores, &venues);
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
            .trigger_size(MoonButtonSize::Action)
            // Same lower bound as every other core selector; it grows past it for a long core name.
            .fit_trigger_width(crate::controls::CORE_COMBO_TRIGGER_W, 260.0)
            .fit_menu_width(160.0, 560.0)
            .menu_size(MoonMenuSize::Compact)
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
            // Use the shared column-selector glyph button instead of a text-field trigger.
            .segment(moon_ui::MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::glyph_btn_w(cx))
            .menu_width_scaled(170.0)
            .menu_size(MoonMenuSize::Compact)
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
        let count_head = format!(
            "{} / {} {}",
            self.rows.len(),
            self.total,
            t!("screener.coins")
        );
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
                        .small()
                        .cleanable(true),
                ),
            )
            .child(label(column_title("vol24")))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-dvol")
                        .state(&self.dvol_input)
                        .small()
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
                    .header_height(SCREENER_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

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
    if let Some(handle) = backend.read(cx).screener_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
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
        crate::window::windowing::restored_window_bounds(saved, bounds),
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
