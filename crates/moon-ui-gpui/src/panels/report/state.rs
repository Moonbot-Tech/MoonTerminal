//! [ReportPanel] construction, retained table state, and per-context column layout.

use super::*;

/// Insert or refresh a scoped strategy before the periodic metadata snapshot contains it.
///
/// Args:
///     strategies: Current exact strategy choices.
///     key: Scoped strategy identity.
///     name: Scoped display name.
///
/// Returns:
///     Nothing; an existing exact key receives the latest display name.
fn upsert_strategy_choice(
    strategies: &mut Vec<ReportStrategy>,
    key: ReportStrategyKey,
    name: String,
) {
    if let Some(strategy) = strategies.iter_mut().find(|strategy| strategy.key == key) {
        strategy.name = name;
    } else {
        strategies.push(ReportStrategy { key, name });
    }
}

/// Highest one-shot Report column migration this build knows how to apply.
const REPORT_COLUMNS_MIGRATION: u32 = 1;

/// Append every column added since the `v2` schema to each saved per-context set under `prefix`.
///
/// The column list is `db::COLUMNS_ADDED_SINCE_V2`, shared with the `app_meta` half of the same
/// repair, so the two stores cannot restore different sets. Pure, so the decision is testable
/// without a window: the caller owns the guard and the write.
///
/// Args:
///     sets: Every table's stored visible-column set, keyed by context id.
///     prefix: Context-id prefix selecting the one table to migrate.
///
/// Returns:
///     Nothing; `sets` is rewritten in place.
pub(super) fn migrate_visible_sets(
    sets: &mut std::collections::HashMap<String, Vec<String>>,
    prefix: &str,
) {
    for (id, keys) in sets.iter_mut() {
        // An empty stored set is not a user choice — `save_ctx_columns` refuses to write one — so
        // leave it alone rather than turning it into a one-column table.
        if !id.starts_with(prefix) || keys.is_empty() {
            continue;
        }
        for column in db::COLUMNS_ADDED_SINCE_V2 {
            if !keys.iter().any(|key| key == column) {
                keys.push((*column).to_string());
            }
        }
    }
}

/// Restore those columns into every saved per-context visible set of one table, exactly once.
///
/// `db::load_visible` carries the same migration for the `app_meta` seed, but a per-context set
/// OVERRIDES that seed wherever one exists — so without this half, the only users who never see a
/// new column are the ones who care enough about columns to have arranged them.
///
/// The completion marker lives in the window layout beside the sets it guards, for the reason
/// given on `WindowLayout::report_columns_migration`. The layout is marked dirty unconditionally:
/// the marker must reach disk even when no stored set needed rewriting, or this reruns forever.
///
/// Args:
///     backend: Shared application backend owning the window layout.
///     ctx_id: Any context id of the table to migrate; every context of it is covered.
///     cx: Application context used to update the layout.
///
/// Returns:
///     Nothing.
fn migrate_ctx_visible(backend: &Entity<Backend>, ctx_id: &str, cx: &mut App) {
    // The base id is recovered rather than spelled out: `theme_contract` pins how many places that
    // literal may appear, and a third copy is exactly the drift that pin exists to catch.
    let Some(base) = crate::persistence::table_persist::base_of(ctx_id) else {
        return;
    };
    let prefix = format!("{base}:");
    backend.update(cx, |backend, _| {
        if backend.layout.report_columns_migration.unwrap_or(0) >= REPORT_COLUMNS_MIGRATION {
            return;
        }
        migrate_visible_sets(&mut backend.layout.table_visible_columns, &prefix);
        backend.layout.report_columns_migration = Some(REPORT_COLUMNS_MIGRATION);
        backend.layout_dirty = true;
    });
}

/// Initial Report metadata loaded away from the GPUI application thread.
struct ReportInitialMetadata {
    cores: Vec<(u64, String)>,
    visible: HashSet<String>,
    columns: Vec<String>,
    sort_key: String,
    sort_desc: bool,
    dock_comment: bool,
    detached_comment: bool,
}

impl ReportInitialMetadata {
    /// Open the retained metadata connection and read initial preferences on a background worker.
    ///
    /// Returns:
    ///     Loaded values, with the same safe defaults used when the Report database is unavailable.
    fn load() -> Self {
        let connection = db::open_reader().ok();
        let cores = connection
            .as_ref()
            .and_then(|connection| db::distinct_cores(connection).ok())
            .unwrap_or_default();
        let visible = connection
            .as_ref()
            .and_then(db::load_visible)
            .map(|saved| saved.into_iter().collect())
            .unwrap_or_else(|| {
                DEFAULT_VISIBLE
                    .iter()
                    .map(|column| column.to_string())
                    .collect()
            });
        let columns = connection
            .as_ref()
            .and_then(|connection| db::display_columns(connection).ok())
            .unwrap_or_default();
        let (sort_key, sort_desc) = connection
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));
        let dock_comment = connection
            .as_ref()
            .and_then(|connection| db::load_comment_pane(connection, false))
            .unwrap_or(true);
        let detached_comment = connection
            .as_ref()
            .and_then(|connection| db::load_comment_pane(connection, true))
            .unwrap_or(true);
        Self {
            cores,
            visible,
            columns,
            sort_key,
            sort_desc,
            dock_comment,
            detached_comment,
        }
    }
}

/// Per-panel user-edit generations for asynchronously loaded Report preferences.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ReportPreferenceRevisions {
    /// Visible-column edits performed after initial metadata loading began.
    pub(super) visible: u64,
    /// Sort edits performed after initial metadata loading began.
    pub(super) sort: u64,
    /// Comment-pane edits performed after initial or host-specific loading began.
    pub(super) comment: u64,
}

/// One complete independently persisted Report preference authority.
pub(super) enum ReportPreferenceWrite {
    /// Full ordered visible-column set.
    Visible(Vec<String>),
    /// Atomic sort key and direction pair.
    Sort { key: String, desc: bool },
    /// Comment-pane visibility for one docked or detached host class.
    Comment { detached: bool, visible: bool },
}

static REPORT_PREFERENCE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REPORT_VISIBLE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REPORT_SORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REPORT_DOCK_COMMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REPORT_WINDOW_COMMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static REPORT_PREFERENCE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Return the process-wide last-write marker for one independent preference authority.
///
/// Args:
///     write: Preference command whose storage key class selects the marker.
///
/// Returns:
///     Shared sequence marker for visible columns, sort, or one comment host class.
fn preference_sequence(write: &ReportPreferenceWrite) -> &'static AtomicU64 {
    match write {
        ReportPreferenceWrite::Visible(_) => &REPORT_VISIBLE_SEQUENCE,
        ReportPreferenceWrite::Sort { .. } => &REPORT_SORT_SEQUENCE,
        ReportPreferenceWrite::Comment {
            detached: false, ..
        } => &REPORT_DOCK_COMMENT_SEQUENCE,
        ReportPreferenceWrite::Comment { detached: true, .. } => &REPORT_WINDOW_COMMENT_SEQUENCE,
    }
}

/// Persist one complete Report preference authority on a serialized background path.
///
/// A newer request marks the older one stale before either can acquire the shared write lock. If
/// an older request already owns the lock, the newer request necessarily writes after it. This
/// preserves last-write-wins ordering across every live Report panel without blocking GPUI.
///
/// Args:
///     cx: Application context used only to clone the background executor.
///     write: Complete visible-column, sort-pair, or host-specific comment preference.
///
/// Returns:
///     Nothing; database unavailability keeps the same best-effort preference semantics.
pub(super) fn schedule_report_preference(cx: &App, write: ReportPreferenceWrite) {
    let request = REPORT_PREFERENCE_SEQUENCE
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    let latest = preference_sequence(&write);
    latest.store(request, Ordering::Release);
    cx.background_executor()
        .spawn(async move {
            let _guard = REPORT_PREFERENCE_WRITE_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if latest.load(Ordering::Acquire) != request {
                return;
            }
            let Ok(connection) = db::open_reader() else {
                return;
            };
            match write {
                ReportPreferenceWrite::Visible(columns) => {
                    let columns = columns.iter().map(String::as_str).collect::<Vec<_>>();
                    db::save_visible(&connection, &columns);
                }
                ReportPreferenceWrite::Sort { key, desc } => {
                    db::save_sort(&connection, &key, desc);
                }
                ReportPreferenceWrite::Comment { detached, visible } => {
                    db::save_comment_pane(&connection, detached, visible);
                }
            }
        })
        .detach();
}

/// Decide which asynchronously loaded preferences are still untouched by the user.
///
/// Args:
///     current: Current per-panel user-edit generations.
///     expected: Generations captured before the metadata read began.
///     allow_app_visible: Whether a context-specific column set does not already outrank app meta.
///
/// Returns:
///     Independent apply decisions for visible columns, sort, and comment visibility.
fn metadata_apply_plan(
    current: ReportPreferenceRevisions,
    expected: ReportPreferenceRevisions,
    allow_app_visible: bool,
) -> (bool, bool, bool) {
    (
        allow_app_visible && current.visible == expected.visible,
        current.sort == expected.sort,
        current.comment == expected.comment,
    )
}

impl ReportPanel {
    /// Refresh the comment-pane preference after this panel changes host context.
    ///
    /// Args:
    ///     detached: Host context whose preference should be loaded.
    ///     expected: Current value; a user edit before completion must win.
    ///     expected_revision: User-edit generation captured before the background read.
    ///     cx: Panel context used to run the blocking read on the background executor.
    ///
    /// Returns:
    ///     Nothing; stale results and unavailable databases leave the current value unchanged.
    fn reload_comment_preference(
        &mut self,
        detached: bool,
        expected: bool,
        expected_revision: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let preference = executor
                .spawn(async move {
                    db::open_reader()
                        .ok()
                        .and_then(|connection| db::load_comment_pane(&connection, detached))
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.detached != detached
                        || this.show_comment != expected
                        || this.preference_revisions.comment != expected_revision
                    {
                        return;
                    }
                    if let Some(show_comment) = preference {
                        this.show_comment = show_comment;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    /// Load retained Report metadata without blocking panel construction or workspace switching.
    ///
    /// Args:
    ///     expected_revisions: User-edit generations captured before the background read.
    ///     allow_app_visible: Whether no per-context column set outranks the database seed.
    ///     cx: Panel context used to run the blocking read on the background executor.
    ///
    /// Returns:
    ///     Nothing; a released panel silently drops the completed metadata.
    fn load_initial_metadata(
        &mut self,
        expected_revisions: ReportPreferenceRevisions,
        allow_app_visible: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let metadata = executor
                .spawn(async move { ReportInitialMetadata::load() })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let ReportInitialMetadata {
                        cores,
                        visible,
                        columns,
                        sort_key,
                        sort_desc,
                        dock_comment,
                        detached_comment,
                    } = metadata;
                    let (apply_visible, apply_sort, apply_comment) = metadata_apply_plan(
                        this.preference_revisions,
                        expected_revisions,
                        allow_app_visible,
                    );
                    if apply_visible {
                        this.visible = visible;
                    }
                    if apply_sort {
                        this.sort_key = sort_key;
                        this.sort_desc = sort_desc;
                        let sort_key = this.sort_key.clone();
                        let sort_desc = this.sort_desc;
                        this.table_state.update(cx, |state, _| {
                            state.set_sort(sort_key, !sort_desc);
                        });
                    }
                    if apply_comment {
                        this.show_comment = if this.detached {
                            detached_comment
                        } else {
                            dock_comment
                        };
                    }
                    let publish_cores = this.last_metadata_at.is_none();
                    if publish_cores {
                        this.cores = cores;
                    }
                    if this.cols.is_empty() {
                        this.cols = Rc::new(columns);
                        let columns = this.cols.clone();
                        this.table_state.update(cx, |state, cx| {
                            if !state.column_widths.is_empty() {
                                complete_widths(&mut state.column_widths, &columns);
                                cx.notify();
                            }
                        });
                    }
                    this.comment_metadata_loaded = true;
                    this.natural_widths.clear();
                    if publish_cores {
                        this.queue_strategy_select_sync(true, cx);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Create a regular docked or detachable Report panel with its default filters.
    ///
    /// Args:
    ///     backend: Shared application backend.
    ///     group: Group used by report coin search.
    ///     window: Owning GPUI window.
    ///     cx: Construction context.
    ///
    /// Returns:
    ///     A panel with the default Report filters and its first query pending for active render.
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_scope(backend, group, None, window, cx)
    }

    /// Create a Report panel, seeding an optional Analytics scope before its first query.
    ///
    /// Args:
    ///     backend: Shared application backend.
    ///     group: Group used by report coin search.
    ///     scope: Optional exact Analytics filter to seed atomically.
    ///     window: Owning GPUI window.
    ///     cx: Construction context.
    ///
    /// Returns:
    ///     A scoped or default Report panel with one query pending for its first active render.
    pub(crate) fn new_with_scope(
        backend: Entity<Backend>,
        group: String,
        scope: Option<ReportScope>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.clone());
        let valuation_generation = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.generation.clone());
        let (last_status_rev, valuation_status) = backend
            .read(cx)
            .valuation
            .as_ref()
            .map(|valuation| valuation.seed_status())
            .unwrap_or_default();
        let seeded_valuation_mode = backend.read(cx).valuation_mode();
        let display_zone =
            crate::chrome::clock::resolved_header_clock_zone(backend.read(cx).header_clock_zone());
        // The retained connection, schema, and preferences are loaded after construction so a
        // workspace transition never waits for SQLite open/schema work on the GPUI thread.
        let cores = Vec::new();
        // Strategy discovery can scan a large covering index, so the first background batch owns
        // it. A scoped window still seeds its selected label immediately.
        let mut strategies: Vec<ReportStrategy> = Vec::new();
        if let Some(scope) = &scope {
            upsert_strategy_choice(&mut strategies, scope.strategy, scope.strategy_name.clone());
        }
        let last_gen = generation
            .as_ref()
            .map(|g| g.load(Ordering::Relaxed))
            .unwrap_or(0)
            .wrapping_add(
                valuation_generation
                    .as_ref()
                    .map(|generation| generation.load(Ordering::Relaxed))
                    .unwrap_or(0),
            );
        // Restore visible column names from `app_meta`, or use the defaults when never saved.
        let default_visible: HashSet<String> = DEFAULT_VISIBLE
            .iter()
            .map(|column| column.to_string())
            .collect();
        let init_cols = Vec::new();
        let (sort_key, sort_desc) = ("buydate".to_string(), true);
        let widths_id = crate::persistence::table_persist::ctx_id("report-table-v2", false);
        migrate_ctx_visible(&backend, &widths_id, cx);
        let context_visible =
            crate::persistence::table_persist::visible(backend.read(cx), &widths_id)
                .filter(|columns| !columns.is_empty());
        let allow_app_visible = context_visible.is_none();
        let visible = context_visible
            .map(|columns| columns.into_iter().collect())
            .unwrap_or(default_visible);
        let expected_revisions = ReportPreferenceRevisions::default();
        let mut saved_widths =
            crate::persistence::table_persist::saved(backend.read(cx), &widths_id);
        complete_widths(&mut saved_widths, &init_cols);
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
            state.column_widths = saved_widths;
        });
        // A column resize mutates table state; persist the exact user widths through shared storage.
        cx.observe(&table_state, |this, state, cx| {
            crate::persistence::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let coin_query = String::new();
        // Both fields sit on the picker's whole-minute grid: the lower bound as picked, the upper
        // one floored into the range it came from.
        let from_query = scope
            .as_ref()
            .and_then(|scope| scope.date_from)
            .map(|secs| secs.div_euclid(date_range::MINUTE) * date_range::MINUTE);
        let to_query = scope
            .as_ref()
            .and_then(|scope| scope.date_to)
            .map(|secs| secs.div_euclid(date_range::MINUTE) * date_range::MINUTE);
        let coin = cx.new(|cx| {
            MoonInputState::new(window, cx)
                .default_value(coin_query.clone())
                .placeholder(t!("report.filter.coin_ph").to_string())
        });
        let from = cx.new(|cx| {
            let mut state = date_range::bound_picker(Bound::From, window, cx);
            if let Some(value) =
                from_query.and_then(|secs| date_range::dt_of_secs(secs, display_zone))
            {
                state.set_value(Some(value), window, cx);
            }
            state
        });
        let to = cx.new(|cx| {
            let mut state = date_range::bound_picker(Bound::To, window, cx);
            if let Some(value) =
                to_query.and_then(|secs| date_range::dt_of_secs(secs, display_zone))
            {
                state.set_value(Some(value), window, cx);
            }
            state
        });
        let selected_strategies = scope.as_ref().map(|scope| HashSet::from([scope.strategy]));
        // Scoped labels are seeded before the first metadata snapshot, so they are display choices
        // but not yet confirmed available choices.
        let available_strategy_keys = HashSet::new();
        let initial_selected_cores = scope
            .as_ref()
            .map(|scope| HashSet::from([scope.strategy.core_uid]))
            .unwrap_or_default();
        let ordered_cores = ordered_strategy_cores(&strategies, &cores, &backend.read(cx).config);
        let groups = strategy_groups(
            &strategies,
            &ordered_cores,
            &t!("report.all_strategies"),
            &t!("analytics.manual_orders"),
        );
        let strategy_select_indices =
            strategy_choice_indices(&groups, selected_strategies.as_ref());
        let strategy_search = ReportStrategyDelegate::search_state();
        let strategy_catalog =
            ReportStrategyDelegate::catalog(groups, available_strategy_keys.clone());
        let strategy_delegate = ReportStrategyDelegate::new(
            strategy_catalog.clone(),
            selected_strategies.as_ref(),
            strategy_search.clone(),
        );
        let strategy_select = cx.new(|cx| {
            MoonComboboxState::new(strategy_delegate, strategy_select_indices, window, cx)
                .multiple(true)
                .searchable(true)
        });
        let scope_owner = cx.entity();
        let scope_control = cx.new(|cx| ReportScopeControl::new(scope_owner.clone(), cx));
        cx.subscribe(
            &strategy_select,
            |panel, _, event: &MoonComboboxEvent<ReportStrategyDelegate>, cx| {
                let MoonComboboxEvent::Change(choices) = event else {
                    return;
                };
                // The widget already owns this selection mutation; only programmatic Report
                // mutations need to synchronize the widget back from panel state.
                panel.set_strategy_choices(choices, cx);
            },
        )
        .detach();
        // Any coin-field change requests a query. Manual input also opens the match popup. An
        // `on_pick` substitution updates `coin_query` first, so either a synchronous or deferred
        // Change event sees the mirror already matched and does not reopen the popup.
        cx.subscribe(&coin, |t, e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = e.read(cx).value().to_string();
                if t.coin_query != value {
                    t.coin_query = value;
                    t.coin_popup_open = !t.coin_query.trim().is_empty();
                    t.request_requery(cx);
                }
            }
        })
        .detach();
        // A non-empty manual date switches to All so a preset cannot silently take precedence.
        cx.subscribe(&from, |t, picker, ev: &MoonDateTimePickerEvent, cx| {
            let MoonDateTimePickerEvent::Change(value) = ev;
            t.apply_bound_change(Bound::From, *value, picker, cx);
        })
        .detach();
        cx.subscribe(&to, |t, picker, ev: &MoonDateTimePickerEvent, cx| {
            let MoonDateTimePickerEvent::Change(value) = ev;
            t.apply_bound_change(Bound::To, *value, picker, cx);
        })
        .detach();
        // A bound composed inside an open popup owes its query on close; the picker repaints then,
        // so the deferred read rides that notify.
        for picker in [&from, &to] {
            cx.observe(picker, |t, picker, cx| {
                if picker.read(cx).is_open() || !t.bounds_pending {
                    return;
                }
                t.bounds_pending = false;
                t.request_requery(cx);
            })
            .detach();
        }
        // The dedicated report-revision channel avoids repainting every shell on each commit.
        let report_revision = backend.read(cx).report_revision.clone();
        cx.observe(&report_revision, |this, _revision, cx| {
            let report_generation = this
                .generation
                .as_ref()
                .map(|generation| generation.load(Ordering::Relaxed))
                .unwrap_or(0);
            let valuation_generation = this
                .valuation_generation
                .as_ref()
                .map(|generation| generation.load(Ordering::Relaxed))
                .unwrap_or(0);
            let current = report_generation.wrapping_add(valuation_generation);
            if current != this.last_gen {
                this.last_gen = current;
                this.requery_on_generation(cx);
            }
            // The valuation mode is application-wide and is edited in Settings, so it changes from
            // outside this panel entirely — nothing here can notice the edit as it happens. It
            // moves no generation — a mode switch changes no rows — so it is compared separately,
            // and it requeries rather than merely repainting: the numbers themselves change.
            let mode = this.backend.read(cx).valuation_mode();
            if mode != this.last_valuation_mode {
                this.last_valuation_mode = mode;
                this.request_requery(cx);
            }
            // Health is polled beside the data generation but never triggers a query: a stall
            // changes no rows, and re-running the report on each visible health transition would
            // turn a broken provider into needless full-table scans.
            let mut last = this.last_status_rev;
            let refreshed = this
                .backend
                .read(cx)
                .valuation
                .as_ref()
                .and_then(|valuation| valuation.status_if_changed(&mut last));
            if let Some(status) = refreshed {
                this.last_status_rev = last;
                this.valuation_status = status;
                cx.notify();
            }
        })
        .detach();

        let workspace_revision = backend.read(cx).workspace_revision();
        cx.observe(&workspace_revision, |this, _revision, cx| {
            if this.standalone {
                return;
            }
            // Never label already-published rows from another core with the new pinned selector.
            this.data = LoadState::default();
            this.selection.clear();
            this.last_strategy_scope = None;
            this.request_requery(cx);
        })
        .detach();

        // Zone identity changes are rare and have their own revision so report commits do not
        // repaint every civil-time consumer. Manual bounds remain absolute instants; only their
        // picker text is rewritten. Presets requery because their civil-day boundaries move.
        let display_time_revision = backend.read(cx).display_time_revision.clone();
        cx.observe(&display_time_revision, |this, _revision, cx| {
            let zone = crate::chrome::clock::resolved_header_clock_zone(
                this.backend.read(cx).header_clock_zone(),
            );
            if zone == this.display_zone {
                return;
            }
            this.display_zone = zone;
            this.display_zone_fields_dirty = true;
            this.natural_widths.clear();
            if this.period == Period::All {
                cx.notify();
            } else {
                // A preset's civil bounds changed. Do not render rows from the old zone under
                // the new preset label while the replacement query is in flight.
                this.data = LoadState::default();
                this.request_requery(cx);
            }
        })
        .detach();

        // Never touched by the user means on: the pane is what makes a long comment readable.
        // Seeded for the docked host; `mark_table_detached` re-reads the detached one.
        let show_comment = true;
        let mut this = Self {
            backend,
            display_zone,
            display_zone_fields_dirty: false,
            group,
            generation,
            valuation_generation,
            valuation_status,
            last_gen,
            last_status_rev,
            last_valuation_mode: seeded_valuation_mode,
            cores,
            strategies,
            available_strategy_keys,
            cols: Rc::new(init_cols),
            data: LoadState::default(),
            sort_key,
            sort_desc,
            sel_cores: initial_selected_cores,
            selected_strategies,
            strategy_select,
            strategy_catalog,
            strategy_search,
            strategy_select_items_dirty: false,
            strategy_select_selection_dirty: false,
            coin,
            coin_query,
            coin_popup_open: false,
            from,
            from_query,
            to,
            to_query,
            bounds_pending: false,
            bound_write_in_progress: false,
            side: scope
                .as_ref()
                .map(|scope| scope.side)
                .unwrap_or(SideFilter::All),
            period: if scope.is_some() {
                Period::All
            } else {
                Period::Today
            },
            kind: scope
                .as_ref()
                .map(|scope| ReportKind::from_filter(scope.emulator))
                .unwrap_or(ReportKind::Real),
            deleted_only: false,
            show_comment,
            comment_metadata_loaded: false,
            preference_revisions: expected_revisions,
            scope_control,
            closed_only: scope.is_some(),
            selection: ReportSelection::default(),
            needs_query: true,
            query_inflight: false,
            query_seq: 0,
            last_query_start: None,
            generation_refresh: query::GenerationRefreshGate::default(),
            last_metadata_at: None,
            last_strategy_scope: None,
            visible,
            table_state,
            widths_id,
            natural_widths: NaturalWidthsCache::default(),
            detached: false,
            standalone: false,
            dock: None,
            focus: cx.focus_handle(),
        };
        this.load_initial_metadata(expected_revisions, allow_app_visible, cx);
        this
    }

    /// Replace the standalone window's exact strategy and inherited Analytics filters.
    ///
    /// Args:
    ///     scope: Replacement exact strategy, date, side, and emulator filters.
    ///     window: Existing standalone Report window.
    ///     cx: Panel context used to update inputs and request one query.
    ///
    /// Returns:
    ///     Nothing; row selection and unrelated coin filters are cleared.
    pub(crate) fn apply_scope(
        &mut self,
        scope: ReportScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        upsert_strategy_choice(&mut self.strategies, scope.strategy, scope.strategy_name);
        self.group = report_group_for_core(&self.backend, scope.strategy.core_uid, cx);
        self.sel_cores = HashSet::from([scope.strategy.core_uid]);
        self.selected_strategies = Some(HashSet::from([scope.strategy]));
        self.queue_strategy_select_sync(true, cx);
        self.side = scope.side;
        self.kind = ReportKind::from_filter(scope.emulator);
        self.period = Period::All;
        self.deleted_only = false;
        self.closed_only = true;
        self.selection.clear();
        self.coin_query.clear();
        self.coin_popup_open = false;
        // The scope's bounds are exact seconds; the fields pick whole minutes, so the lower one is
        // floored and the upper one keeps the minute its inclusive bound ends in.
        self.from_query = scope
            .date_from
            .map(|secs| secs.div_euclid(date_range::MINUTE) * date_range::MINUTE);
        self.to_query = scope
            .date_to
            .map(|secs| secs.div_euclid(date_range::MINUTE) * date_range::MINUTE);
        let from_value = self
            .from_query
            .and_then(|secs| date_range::dt_of_secs(secs, self.display_zone));
        let to_value = self
            .to_query
            .and_then(|secs| date_range::dt_of_secs(secs, self.display_zone));

        self.coin.update(cx, |input, input_cx| {
            input.set_value(String::new(), window, input_cx)
        });
        self.write_bound(Bound::From, from_value, window, cx);
        self.write_bound(Bound::To, to_value, window, cx);
        self.request_requery(cx);
    }

    /// Write one bound into its field, restoring that edge's default clock time when cleared.
    ///
    /// Args:
    ///     bound: Which edge is being written.
    ///     value: New bound, or `None` to empty the field.
    ///     window: Window forwarded to the picker's calendar.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     Nothing; the `Change` this emits is absorbed by `apply_bound_change`, which sees the
    ///     mirror already matching and requests no extra query.
    pub(super) fn write_bound(
        &mut self,
        bound: Bound,
        value: Option<chrono::NaiveDateTime>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let picker = match bound {
            Bound::From => self.from.clone(),
            Bound::To => self.to.clone(),
        };
        self.bound_write_in_progress = true;
        picker.update(cx, |state, state_cx| {
            state.set_value(value, window, state_cx);
            if value.is_none() {
                state.set_time(bound.default_time(), state_cx);
            }
        });
        self.bound_write_in_progress = false;
    }

    /// Mirror a bound field's new value into the filter and requery.
    ///
    /// Args:
    ///     bound: Which edge changed.
    ///     value: The field's new value, or `None` while it is empty.
    ///     picker: The field itself, used to restore its default clock time after a clear.
    ///     cx: Panel context.
    ///
    /// Returns:
    ///     Nothing; an unchanged value requests no query, which is what keeps a programmatic
    ///     write from queueing a second read of the same filter.
    pub(super) fn apply_bound_change(
        &mut self,
        bound: Bound,
        value: Option<chrono::NaiveDateTime>,
        picker: Entity<MoonDateTimePickerState>,
        cx: &mut Context<Self>,
    ) {
        if self.bound_write_in_progress {
            return;
        }
        let secs = value.and_then(|dt| date_range::secs_of_dt(dt, self.display_zone, bound));
        let slot = match bound {
            Bound::From => &mut self.from_query,
            Bound::To => &mut self.to_query,
        };
        if *slot != secs {
            *slot = secs;
            if secs.is_some() {
                self.period = Period::All;
            }
            // While the popup is open the user is still composing the bound; the query waits for
            // the close so a spun drum cannot issue one read per step.
            if picker.read(cx).is_open() {
                self.bounds_pending = true;
                cx.notify();
            } else {
                self.request_requery(cx);
            }
        }
        // Clearing resets the picker's clock to midnight; the "to" field must go back to the END
        // of a day, or the next day picked there would select only its first minute. Guarded on
        // the current time so the `Change` this emits cannot re-enter forever.
        let default = bound.default_time();
        if value.is_none() && picker.read(cx).time() != default {
            picker.update(cx, |state, state_cx| state.set_time(default, state_cx));
        }
    }

    /// Rewrite picker civil values after a selected-zone change without moving their instants.
    ///
    /// Args:
    ///     window: Window required by the retained picker states.
    ///     cx: Panel context used to update picker entities.
    ///
    /// Returns:
    ///     Nothing; absolute query bounds remain unchanged.
    pub(super) fn sync_display_zone_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.display_zone_fields_dirty {
            return;
        }
        self.display_zone_fields_dirty = false;
        let from = self
            .from_query
            .and_then(|secs| date_range::dt_of_secs(secs, self.display_zone));
        let to = self
            .to_query
            .and_then(|secs| date_range::dt_of_secs(secs, self.display_zone));
        self.write_bound(Bound::From, from, window, cx);
        self.write_bound(Bound::To, to, window, cx);
    }

    /// Schedule synchronization of the retained strategy combobox on its next render.
    ///
    /// Args:
    ///     refresh_items: Whether core/strategy metadata changed and groups must be rebuilt.
    ///     cx: Panel context used to request a render with a live window handle.
    ///
    /// Returns:
    ///     Nothing; repeated requests coalesce before element construction.
    pub(super) fn queue_strategy_select_sync(
        &mut self,
        refresh_items: bool,
        cx: &mut Context<Self>,
    ) {
        self.strategy_select_items_dirty |= refresh_items;
        self.strategy_select_selection_dirty = true;
        cx.notify();
    }

    /// Apply queued strategy metadata and selection changes with the owning window available.
    ///
    /// Args:
    ///     window: Owning window required by the MoonUI combobox replacement API.
    ///     cx: Panel render context used to update the retained entity.
    ///
    /// Returns:
    ///     Nothing; selected values outside the retained query remain selected by exact identity.
    pub(super) fn flush_strategy_select_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.strategy_select_items_dirty && !self.strategy_select_selection_dirty {
            return;
        }
        if self.strategy_select_items_dirty {
            let ordered_cores = ordered_strategy_cores(
                &self.strategies,
                &self.cores,
                &self.backend.read(cx).config,
            );
            let groups = strategy_groups(
                &self.strategies,
                &ordered_cores,
                &t!("report.all_strategies"),
                &t!("analytics.manual_orders"),
            );
            self.strategy_catalog =
                ReportStrategyDelegate::catalog(groups, self.available_strategy_keys.clone());
        }
        let selected = self
            .strategy_catalog
            .selected_indices(self.selected_strategies.as_ref());
        let unfiltered = ReportStrategyDelegate::unfiltered(
            self.strategy_catalog.clone(),
            self.selected_strategies.as_ref(),
            self.strategy_search.clone(),
        );
        let filtered = ReportStrategyDelegate::new(
            self.strategy_catalog.clone(),
            self.selected_strategies.as_ref(),
            self.strategy_search.clone(),
        );
        self.strategy_select.update(cx, |select, select_cx| {
            // Synchronize against a full delegate, then restore the retained filtered view. The
            // selection snapshot keeps item values even when their rows are outside the query.
            select.set_items(unfiltered, window, select_cx);
            select.set_selected_indices(selected, window, select_cx);
            select.set_items(filtered, window, select_cx);
        });
        self.strategy_select_items_dirty = false;
        self.strategy_select_selection_dirty = false;
    }

    /// Return retained table state for the detached window's automatic-width reset button.
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Switch a newly created detached panel to the `:win` column-storage context.
    ///
    /// Reload widths and visible columns for detached mode so docked tabs and windows keep separate
    /// layouts, and enable the detached-only manual date controls.
    ///
    /// The comment pane follows the same split: it is a view preference of this table, so a window
    /// keeps its own answer instead of inheriting the docked tab's.
    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.detached = true;
        self.widths_id = crate::persistence::table_persist::ctx_id("report-table-v2", true);
        let mut saved =
            crate::persistence::table_persist::saved(self.backend.read(cx), &self.widths_id);
        complete_widths(&mut saved, &self.cols);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        if self.comment_metadata_loaded {
            self.reload_comment_preference(
                true,
                self.show_comment,
                self.preference_revisions.comment,
                cx,
            );
        }
        cx.notify();
    }

    /// Enable the dedicated Report title bar while retaining detached table controls.
    ///
    /// Args:
    ///     window: Standalone Report window whose geometry is persisted.
    ///     cx: Panel context used to restore detached widths and repaint.
    ///
    /// Returns:
    ///     Nothing.
    pub(crate) fn mark_standalone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mark_table_detached(cx);
        self.standalone = true;
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::window::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |backend, _| {
                if backend
                    .layout
                    .report_window
                    .map(|geometry| (geometry.x, geometry.y, geometry.w, geometry.h))
                    != Some((x, y, w, h))
                {
                    backend.layout.report_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    backend.layout_dirty = true;
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Apply a saved per-context visible-column set for `widths_id`, when non-empty.
    ///
    /// Docked `:dock` and detached `:win` modes have distinct shared-storage entries. An empty set
    /// leaves the current selection intact rather than producing an empty table.
    pub(super) fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) =
            crate::persistence::table_persist::visible(self.backend.read(cx), &self.widths_id)
        {
            let set: HashSet<String> = keys.into_iter().collect();
            if !set.is_empty() {
                self.visible = set;
            }
        }
    }

    /// Save visible columns in runtime table order to per-context storage under `widths_id`.
    ///
    /// Called through [`Self::persist_visible`] after column-menu changes. An empty set is not
    /// written, so an older non-empty per-context entry remains available and can override the empty
    /// `app_meta` set when the panel is recreated.
    pub(super) fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.to_string())
            .collect();
        if !keys.is_empty() {
            crate::persistence::table_persist::set_visible(
                &self.backend,
                &self.widths_id,
                keys,
                cx,
            );
        }
    }

    /// Return visible columns in runtime-schema order for stable rendering and persistence.
    pub(super) fn visible_cols(&self) -> Vec<&str> {
        self.cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.as_str())
            .collect()
    }

    /// Return whether every runtime column is enabled in the Columns menu.
    pub(super) fn all_columns_on(&self) -> bool {
        !self.cols.is_empty() && self.cols.iter().all(|c| self.visible.contains(c.as_str()))
    }

    /// Persist visible columns to `app_meta` and, when non-empty, the dock/window table descriptor.
    ///
    /// `app_meta` records an empty set, but [`Self::save_ctx_columns`] deliberately leaves any older
    /// per-context descriptor unchanged in that case.
    pub(super) fn persist_visible(&mut self, cx: &mut App) {
        self.preference_revisions.visible = self.preference_revisions.visible.wrapping_add(1);
        let visible = self
            .visible_cols()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        schedule_report_preference(cx, ReportPreferenceWrite::Visible(visible));
        self.save_ctx_columns(cx);
    }
}

#[cfg(test)]
mod tests;
