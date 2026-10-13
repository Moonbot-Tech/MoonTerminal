//! Storage tab for local databases (`data/*.sqlite`): main/WAL sizes, row counts, maintenance
//! actions (compact/backup, the trade-tape cleanup of [`trades_cleanup`]), and
//! strategy-database settings.
//!
//! Changes apply immediately to `cfg/storage.toml` and live `strat_db` atomics, unlike draft-backed
//! tabs: Storage has its own file, and its recording toggle must take effect without Save. SQLite
//! statistics and maintenance run only on the background executor because counting a large replica
//! on the UI thread would freeze the interface.

mod core_data;
mod trades_cleanup;

use std::collections::HashSet;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonDropdown, MoonInputEvent,
    MoonNumberInput, MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::common::{LabelCol, UsageShare, hint_row_in, label_col, num_input, usage_bar};
use super::{SettingsView, StatusMsg, open_folder};
use crate::design;
use crate::panels::{RadioMark, radio_items};
use moon_core::config::{paths, storage as storage_cfg};
use trades_cleanup::CleanupPreview;
pub(crate) use trades_cleanup::startup as trades_cleanup_startup;

/// Snapshot of storage state collected in the background.
#[derive(Clone, Default)]
pub(super) struct StorageInfo {
    /// Main-file and WAL sizes per database, or `None` when the file does not exist.
    pub reports: Option<(u64, u64)>,
    pub strategies: Option<(u64, u64)>,
    pub klines: Option<(u64, u64)>,
    pub trades: Option<(u64, u64)>,
    /// Replica row count from the shared fallible read path.
    ///
    /// `None` means the background snapshot has not completed,
    /// `Err(ReadFail::NotReady)` means the replica is absent, and
    /// `Err(ReadFail::Failed { .. })` means the count could not be read. Neither
    /// non-data case may render as zero because zero asserts an empty replica.
    pub report_rows: Option<moon_core::db::ReadResult<i64>>,
    pub strat_live: i64,
    pub strat_deleted: i64,
    pub strat_versions: i64,
}

/// Storage-tab state: its configuration and background snapshot.
pub(super) struct StorageEd {
    pub cfg: storage_cfg::StorageCfg,
    pub info: Option<StorageInfo>,
    pub inflight: bool,
    /// Whether a maintenance operation is running and action buttons must be disabled.
    pub busy: bool,
    /// What the trade-tape cleanup would remove, or why it could not be counted; `None` while
    /// a count is pending.
    pub cleanup: Option<Result<CleanupPreview, String>>,
    /// Whether a count is running; a second ask meanwhile sets `cleanup_dirty` instead.
    pub cleanup_inflight: bool,
    /// The margin moved while a count was running: count again when it lands.
    pub cleanup_dirty: bool,
    /// The "Data by core" section.
    pub core_data: core_data::CoreDataEd,
}

/// One database in the usage bar, in bar order: reports, strategies, trades, klines.
///
/// The four `bytes` sum to the total the header prints. A missing file contributes zero,
/// and a WAL is part of its database.
pub(super) struct UsagePart {
    pub(super) bytes: u64,
    pub(super) color: u32,
    pub(super) label: String,
}

/// A database card's narrowest width, unscaled; two of them side by side or one column.
const CARD_MIN_W: f32 = 300.0;

/// Segment colours from the storage mockup. Mid saturation so they read on light, dark and graphite.
const USAGE_COLOR: [u32; 4] = [0x3b82c4, 0x7c3aed, 0x0f9488, 0xb45309];

fn file_bytes(size: Option<(u64, u64)>) -> u64 {
    size.map(|(main, wal)| main.saturating_add(wal))
        .unwrap_or(0)
}

/// The four database shares of `info`.
///
/// Args:
///     info: The latest size snapshot. A missing snapshot counts as four empty files.
///
/// Returns:
///     Reports, strategies, trades, then klines.
pub(super) fn usage_parts(info: &StorageInfo) -> [UsagePart; 4] {
    let bytes = [
        file_bytes(info.reports),
        file_bytes(info.strategies),
        file_bytes(info.trades),
        file_bytes(info.klines),
    ];
    let labels = [
        t!("storage.db.reports.title"),
        t!("storage.db.strategies.title"),
        t!("storage.db.trades.title"),
        t!("storage.db.klines.title"),
    ];
    std::array::from_fn(|i| UsagePart {
        bytes: bytes[i],
        color: USAGE_COLOR[i],
        label: labels[i].to_string(),
    })
}

pub(super) fn build() -> StorageEd {
    StorageEd {
        cfg: storage_cfg::load(),
        info: None,
        inflight: false,
        busy: false,
        cleanup: None,
        cleanup_inflight: false,
        cleanup_dirty: false,
        core_data: core_data::CoreDataEd::default(),
    }
}

/// Collect storage sizes and row counts on the background executor.
///
/// The reports count preserves `NotReady` for an absent replica and `Failed`
/// for an unreadable replica so both remain distinct from a genuine zero.
fn collect_info() -> StorageInfo {
    use moon_core::db::maint::db_sizes;
    let sized = |p: &std::path::Path| p.exists().then(|| db_sizes(p));
    let mut out = StorageInfo {
        reports: sized(&paths::reports_db_path()),
        strategies: sized(&paths::strategies_db_path()),
        klines: sized(&paths::klines_db_path()),
        trades: sized(&paths::trades_db_path()),
        ..Default::default()
    };
    out.report_rows = Some(moon_core::db::report_row_count());
    if let Some(conn) = moon_core::strat_db::open_reader() {
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0);
        out.strat_live = count("SELECT COUNT(*) FROM strategies WHERE deleted=0");
        out.strat_deleted = count("SELECT COUNT(*) FROM strategies WHERE deleted=1");
        out.strat_versions = count("SELECT COUNT(*) FROM strategy_versions");
    }
    out
}

impl SettingsView {
    /// Collects storage sizes and row counts in the background.
    ///
    /// Only one collection may run at a time; calls made while one is in flight are ignored.
    pub(super) fn storage_refresh(&mut self, cx: &mut Context<Self>) {
        if self.storage.inflight {
            return;
        }
        self.storage.inflight = true;
        self.storage_cleanup_refresh(cx);
        // The core table is always on the page, so its counts move with every snapshot.
        self.core_data_refresh(cx);
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let info = executor.spawn(async move { collect_info() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.storage.inflight = false;
                    this.storage.info = Some(info);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Runs a maintenance operation on the background executor, then refreshes data and status.
    /// A job may answer with a detail line for the status (`Some`), such as what a cleanup
    /// removed; `None` reports plain success.
    fn storage_op(
        &mut self,
        cx: &mut Context<Self>,
        op_key: &'static str,
        job: impl FnOnce() -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        if self.storage.busy {
            return;
        }
        self.storage.busy = true;
        self.status = Some((
            StatusMsg::Text(t!("storage.busy", op = t!(op_key)).to_string()),
            false,
        ));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor.spawn(async move { job() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.storage.busy = false;
                    this.status = Some(match result {
                        Ok(None) => (
                            StatusMsg::Text(t!("storage.op_done", op = t!(op_key)).to_string()),
                            false,
                        ),
                        Ok(Some(detail)) => (
                            StatusMsg::Text(
                                t!("storage.op_done_with", op = t!(op_key), detail = detail)
                                    .to_string(),
                            ),
                            false,
                        ),
                        Err(e) => (
                            StatusMsg::Text(
                                t!("storage.op_failed", op = t!(op_key), err = e.to_string())
                                    .to_string(),
                            ),
                            true,
                        ),
                    });
                    // Invalidate the snapshot and re-count after every maintenance attempt: a
                    // compaction moves the sizes, a cleanup moves the counts too. The count is
                    // asked on its own — `storage_refresh` skips everything while a snapshot is
                    // still in flight, and the counts must not stay armed on pre-op numbers.
                    this.storage.info = None;
                    this.storage_refresh(cx);
                    this.storage_cleanup_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Sets the version limit, already clamped to `0..=10000`, and writes storage.toml.
    fn set_version_limit(&mut self, v: u32, cx: &mut Context<Self>) {
        if self.storage.cfg.strategies.version_limit != v {
            self.storage.cfg.strategies.version_limit = v;
            moon_core::strat_db::set_version_limit(v);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    /// Sets the `trades.sqlite` ceiling in megabytes. Zero is no ceiling.
    fn set_trades_max_mb(&mut self, v: u32, cx: &mut Context<Self>) {
        if self.storage.cfg.trade_replay.max_mb != v {
            self.storage.cfg.trade_replay.max_mb = v;
            moon_core::market::trade_replay::trade_cache::set_max_mb(v);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    /// Sets the prints kept around a trade, snapped onto `TRADE_MARGIN_STEPS_S`, and writes
    /// storage.toml. The cleanup count is measured against the margin, so it is taken again.
    fn set_trades_margin_s(&mut self, secs: u32, cx: &mut Context<Self>) {
        let v = storage_cfg::snap_trade_margin_s(secs);
        if self.storage.cfg.trade_replay.margin_s != v {
            self.storage.cfg.trade_replay.margin_s = v;
            moon_core::market::trade_replay::set_margin_s(v);
            storage_cfg::save(&self.storage.cfg);
            self.storage_cleanup_refresh(cx);
            cx.notify();
        }
    }

    /// Sets the minutes a position must be held to count as long, clamped to
    /// `LONG_POSITION_MIN_RANGE`. The cleanup's count moves with it.
    fn set_long_position_min(&mut self, minutes: u32, cx: &mut Context<Self>) {
        let v = storage_cfg::clamp_long_position_min(minutes);
        if self.storage.cfg.trade_replay.long_position_min != v {
            self.storage.cfg.trade_replay.long_position_min = v;
            moon_core::market::trade_replay::set_long_position_min(v);
            storage_cfg::save(&self.storage.cfg);
            self.storage_cleanup_refresh(cx);
            cx.notify();
        }
    }

    /// Subscribe the number fields once. Each edit saves storage.toml immediately.
    fn wire_storage_numbers(&mut self, cx: &mut Context<Self>) {
        if self.storage_ui.wired {
            return;
        }
        self.storage_ui.wired = true;
        cx.subscribe(
            &self.storage_ui.version_limit,
            |this, state, ev: &MoonInputEvent, cx| {
                if !matches!(ev, MoonInputEvent::Change) {
                    return;
                }
                let Some(v) = MoonNumberInput::read_value(&state, cx) else {
                    return;
                };
                this.set_version_limit(v.round().clamp(0.0, 10_000.0) as u32, cx);
            },
        )
        .detach();
        cx.subscribe(
            &self.storage_ui.max_mb,
            |this, state, ev: &MoonInputEvent, cx| {
                if !matches!(ev, MoonInputEvent::Change) {
                    return;
                }
                let Some(v) = MoonNumberInput::read_value(&state, cx) else {
                    return;
                };
                this.set_trades_max_mb(v.round().clamp(0.0, 100_000.0) as u32, cx);
            },
        )
        .detach();
        cx.subscribe(
            &self.storage_ui.long_min,
            |this, state, ev: &MoonInputEvent, cx| {
                if !matches!(ev, MoonInputEvent::Change) {
                    return;
                }
                let Some(v) = MoonNumberInput::read_value(&state, cx) else {
                    return;
                };
                let minutes = v.round().max(0.0) as u32;
                this.set_long_position_min(minutes, cx);
            },
        )
        .detach();
    }

    /// The stepper's label for a margin: a whole number of minutes when the step divides by
    /// 60, seconds otherwise. A step that is not a whole minute must not read as the minute
    /// beside it — 60 s already says "1 min".
    pub(super) fn trades_margin_label(secs: u32) -> String {
        if secs.is_multiple_of(60) {
            t!("storage.trades_min", min = secs / 60).to_string()
        } else {
            t!("storage.trades_sec", s = secs).to_string()
        }
    }

    fn set_strategies_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.storage.cfg.strategies.enabled != on {
            self.storage.cfg.strategies.enabled = on;
            moon_core::strat_db::set_enabled(on);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    fn set_persist_trades(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.storage.cfg.trade_replay.persist_trades != on {
            self.storage.cfg.trade_replay.persist_trades = on;
            moon_core::market::trade_replay::trade_cache::set_enabled(on);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    fn set_autoload_cores(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.storage.cfg.trade_replay.autoload_cores != on {
            self.storage.cfg.trade_replay.autoload_cores = on;
            moon_core::market::trade_replay::set_tape_autoload_cores(on);
            if on {
                crate::analytics::tape_autoload::switched_on();
            } else {
                crate::analytics::tape_autoload::cores_switched_off();
            }
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    fn set_autoload_missing(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.storage.cfg.trade_replay.autoload_missing != on {
            self.storage.cfg.trade_replay.autoload_missing = on;
            moon_core::market::trade_replay::set_tape_autoload(on);
            if on {
                crate::analytics::tape_autoload::switched_on();
            } else {
                crate::analytics::tape_autoload::switched_off();
            }
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    fn set_cleanup_at_startup(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.storage.cfg.trade_replay.cleanup_at_startup != on {
            self.storage.cfg.trade_replay.cleanup_at_startup = on;
            moon_core::market::trade_replay::set_cleanup_at_startup(on);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }
}

impl SettingsView {
    /// The Storage tab: refreshes what is missing and gathers the page; the layout is
    /// [`storage_body_el`]'s.
    pub(super) fn storage_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.storage.info.is_none() && !self.storage.inflight {
            self.storage_refresh(cx);
        }
        self.wire_storage_numbers(cx);
        let core = self.core_data_section(cx);
        let view = StorageView {
            info: self.storage.info.clone().unwrap_or_default(),
            busy: self.storage.busy,
            path: paths::db_dir().display().to_string(),
            cfg: self.storage.cfg.clone(),
            version_limit: self.storage_ui.version_limit.clone(),
            max_mb: self.storage_ui.max_mb.clone(),
            long_min: self.storage_ui.long_min.clone(),
            cleanup: self.storage.cleanup.clone(),
            core,
        };
        let open = self.more_open.clone();
        let ctx = StorageCtx {
            weak: cx.entity().downgrade(),
            open: &open,
        };
        storage_body_el(view, &ctx, cx)
    }
}

/// What a storage builder needs of the view: where its controls send their edits, and which
/// rows have their long help open.
pub(super) struct StorageCtx<'a> {
    pub(super) weak: WeakEntity<SettingsView>,
    pub(super) open: &'a HashSet<&'static str>,
}

impl StorageCtx<'_> {
    /// A click handler that runs `f` on the view; a dropped view ignores it.
    fn click(
        &self,
        f: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let weak = self.weak.clone();
        move |_, window, app| {
            let _ = weak.update(app, |this, cx| f(this, window, cx));
        }
    }
}

/// What the Storage page draws, gathered by [`SettingsView::storage_tab`].
pub(super) struct StorageView {
    pub(super) info: StorageInfo,
    /// A maintenance operation runs: action buttons are disabled.
    pub(super) busy: bool,
    /// The data folder as shown.
    pub(super) path: String,
    pub(super) cfg: storage_cfg::StorageCfg,
    pub(super) version_limit: Entity<moon_ui::MoonInputState>,
    pub(super) max_mb: Entity<moon_ui::MoonInputState>,
    pub(super) long_min: Entity<moon_ui::MoonInputState>,
    /// The trade-tape cleanup's count; `None` while pending.
    pub(super) cleanup: Option<Result<CleanupPreview, String>>,
    /// The "Data by core" section.
    pub(super) core: AnyElement,
}

/// The Storage page: the usage panel, the four database cards in two columns, then the per-core
/// table.
pub(super) fn storage_body_el(view: StorageView, s: &StorageCtx, cx: &App) -> AnyElement {
    let StorageView {
        info,
        busy,
        path,
        cfg,
        version_limit,
        max_mb,
        long_min,
        cleanup,
        core,
    } = view;
    let parts = usage_parts(&info);
    let refresh = tool_btn("storage-refresh", t!("storage.refresh").to_string(), false)
        .on_click(s.click(|this, _, cx| {
            this.storage.info = None;
            this.storage_refresh(cx);
        }))
        .render()
        .into_any_element();
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 12.0))
        .child(usage_panel(&parts, path, refresh, cx))
        // Two columns of equal height, as the mockup's two-wide grid: the three light cards
        // stacked on the left beside the heavy trades card, so no light card stands as an empty
        // box next to it. Below two 300-wide columns they wrap into one.
        .child(
            h_flex()
                .debug_selector(|| "storage-cards".into())
                .w_full()
                .flex_wrap()
                .items_stretch()
                .gap(design::ui_px(cx, 10.0))
                .child(
                    v_flex()
                        .debug_selector(|| "storage-col-light".into())
                        .flex_1()
                        .min_w(design::ui_px(cx, CARD_MIN_W))
                        .gap(design::ui_px(cx, 10.0))
                        .child(reports_card(s, &info, busy, cx))
                        .child(strategies_card(s, &info, &cfg, &version_limit, busy, cx))
                        .child(klines_card(s, &info, cx)),
                )
                .child(
                    v_flex()
                        .debug_selector(|| "storage-col-trades".into())
                        .flex_1()
                        .min_w(design::ui_px(cx, CARD_MIN_W))
                        .child(trades_card(
                            s, &info, &cfg, &max_mb, &long_min, cleanup, busy, cx,
                        )),
                ),
        )
        .child(core)
        .into_any_element()
}

/// Outline tool button. The spaces around the label work around `MoonButton`'s `pad_x = 0`.
fn tool_btn(id: &'static str, label: String, disabled: bool) -> MoonButton {
    MoonButton::new(id)
        .variant(moon_ui::MoonButtonVariant::Neutral)
        .label(format!("  {label}  "))
        .disabled(disabled)
}

/// Compact one database. `path` is owned so the click can run after this render returns.
fn compact_btn(
    s: &StorageCtx,
    id: &'static str,
    path: std::path::PathBuf,
    busy: bool,
) -> AnyElement {
    tool_btn(id, t!("storage.compact").to_string(), busy)
        .on_click(s.click(move |this, _, cx| {
            let path = path.clone();
            this.storage_op(cx, "storage.op_compact", move || {
                moon_core::db::maint::compact_db(&path).map(|()| None)
            });
        }))
        .render()
        .into_any_element()
}

/// A checkbox row. The long explanation sits behind "подробнее".
#[allow(clippy::too_many_arguments)]
fn flag_row(
    s: &StorageCtx,
    col: LabelCol,
    id: &'static str,
    label: String,
    more: String,
    checked: bool,
    change: fn(&mut SettingsView, bool, &mut Context<SettingsView>),
    cx: &App,
) -> AnyElement {
    let weak = s.weak.clone();
    let check = MoonCheckbox::new(SharedString::from(format!("{id}-check")))
        .checked(checked)
        .on_change(move |v: &bool, _, app| {
            let _ = weak.update(app, |this, cx| change(this, *v, cx));
        })
        .into_any_element();
    setting_row(s, col, id, label, None, more, check, cx)
}

/// A labelled row with its control on the right and longer help behind "подробнее"; `col` is
/// its card's label column.
#[allow(clippy::too_many_arguments)]
fn setting_row(
    s: &StorageCtx,
    col: LabelCol,
    id: &'static str,
    label: String,
    hint: Option<String>,
    more: String,
    control: AnyElement,
    cx: &App,
) -> AnyElement {
    let open = s.open.contains(id);
    let weak = s.weak.clone();
    let toggle: super::common::Action = Rc::new(move |_, app| {
        let _ = weak.update(app, |this, cx| {
            if !this.more_open.remove(id) {
                this.more_open.insert(id);
            }
            cx.notify();
        });
    });
    hint_row_in(col, id, label, hint, Some(more), control, open, toggle, cx)
}

fn reports_card(s: &StorageCtx, info: &StorageInfo, busy: bool, cx: &App) -> AnyElement {
    let rows = match &info.report_rows {
        Some(Ok(rows)) => t!("storage.reports_rows", rows = rows).to_string(),
        Some(Err(moon_core::db::ReadFail::NotReady)) | None => "—".to_string(),
        Some(Err(_)) => t!("common.db_read_failed_short").to_string(),
    };
    let compact = compact_btn(s, "reports-compact", paths::reports_db_path(), busy);
    let readout = setting_row(
        s,
        LabelCol::Full,
        "storage-reports-rows",
        rows,
        Some(t!("storage.db.reports.note").to_string()),
        t!("storage.reports_hint").to_string(),
        div().into_any_element(),
        cx,
    );
    settings_card(
        "storage-card-reports",
        t!("storage.db.reports.title").to_string(),
        size_line(info.reports),
        vec![compact],
        readout,
        cx,
    )
}

fn strategies_card(
    s: &StorageCtx,
    info: &StorageInfo,
    cfg: &storage_cfg::StorageCfg,
    version_limit: &Entity<moon_ui::MoonInputState>,
    busy: bool,
    cx: &App,
) -> AnyElement {
    let enabled = cfg.strategies.enabled;
    let limit = cfg.strategies.version_limit;
    let rows = t!(
        "storage.strategies_rows",
        live = info.strat_live,
        deleted = info.strat_deleted,
        versions = info.strat_versions
    )
    .to_string();
    let field = no_limit_note(
        "storage-version-limit-off",
        num_input(
            "storage-version-limit-n",
            version_limit,
            0.0..=10_000.0,
            1.0,
            None,
        ),
        limit == 0,
        cx,
    );
    // One label column for the card's labelled rows; its control-less note takes the full width.
    let col = label_col(
        &[
            t!("storage.strategies_enabled"),
            t!("storage.version_limit"),
        ],
        cx,
    );
    let body = v_flex()
        .w_full()
        .child(flag_row(
            s,
            col,
            "storage-strat-enabled",
            t!("storage.strategies_enabled").to_string(),
            t!("storage.strategies_enabled_hint").to_string(),
            enabled,
            SettingsView::set_strategies_enabled,
            cx,
        ))
        .child(mono_readout("storage-strat-rows", rows, cx))
        .child(setting_row(
            s,
            col,
            "storage-version-limit",
            t!("storage.version_limit").to_string(),
            None,
            t!("storage.version_limit_hint").to_string(),
            field,
            cx,
        ))
        .child(setting_row(
            s,
            LabelCol::Full,
            "storage-strategies-more",
            t!("storage.db.strategies.note").to_string(),
            None,
            t!("storage.strategies_hint").to_string(),
            div().into_any_element(),
            cx,
        ));
    let compact = compact_btn(s, "strat-compact", paths::strategies_db_path(), busy);
    let backup = tool_btn("strat-backup", t!("storage.backup").to_string(), busy)
        .on_click(s.click(|this, _, cx| {
            this.storage_op(cx, "storage.op_backup", || {
                let strategies = moon_core::strat_db::backup::backup_now();
                let traces = moon_core::db::order_traces::backup::backup_now();
                strategies.and(traces).map(|_| None)
            });
        }))
        .render()
        .into_any_element();
    settings_card(
        "storage-card-strategies",
        t!("storage.db.strategies.title").to_string(),
        size_line(info.strategies),
        vec![compact, backup],
        body,
        cx,
    )
}

#[allow(clippy::too_many_arguments)] // one card's inputs, all from `StorageView`
fn trades_card(
    s: &StorageCtx,
    info: &StorageInfo,
    cfg: &storage_cfg::StorageCfg,
    max_mb_state: &Entity<moon_ui::MoonInputState>,
    long_min_state: &Entity<moon_ui::MoonInputState>,
    cleanup: Option<Result<CleanupPreview, String>>,
    busy: bool,
    cx: &App,
) -> AnyElement {
    let cfg = &cfg.trade_replay;
    let max_field = no_limit_note(
        "storage-trades-max-off",
        num_input(
            "storage-trades-max-n",
            max_mb_state,
            0.0..=100_000.0,
            1.0,
            Some(t!("storage.unit_mb").to_string()),
        ),
        cfg.max_mb == 0,
        cx,
    );
    let long_field = num_input(
        "storage-trades-long-n",
        long_min_state,
        1.0..=120.0,
        1.0,
        Some(t!("storage.unit_min").to_string()),
    );
    // One label column for the card's labelled rows; its control-less note takes the full width.
    let col = label_col(
        &[
            t!("storage.trades_enabled"),
            t!("storage.trades_margin"),
            t!("storage.trades_max_mb"),
            t!("storage.trades_long_position"),
            t!("storage.trades_autoload_cores"),
            t!("storage.trades_autoload"),
            t!("storage.trades_cleanup_at_startup"),
            t!("storage.trades_cleanup"),
        ],
        cx,
    );
    let body = v_flex()
        .w_full()
        .child(flag_row(
            s,
            col,
            "storage-trades-enabled",
            t!("storage.trades_enabled").to_string(),
            t!("storage.trades_enabled_hint").to_string(),
            cfg.persist_trades,
            SettingsView::set_persist_trades,
            cx,
        ))
        .child(setting_row(
            s,
            col,
            "storage-trades-margin",
            t!("storage.trades_margin").to_string(),
            None,
            t!("storage.trades_margin_hint").to_string(),
            margin_picker(s, cfg.margin_s, cx).into_any_element(),
            cx,
        ))
        .child(setting_row(
            s,
            col,
            "storage-trades-max",
            t!("storage.trades_max_mb").to_string(),
            None,
            t!("storage.trades_max_mb_hint").to_string(),
            max_field,
            cx,
        ))
        .child(setting_row(
            s,
            col,
            "storage-trades-long",
            t!("storage.trades_long_position").to_string(),
            None,
            t!("storage.trades_long_position_hint").to_string(),
            long_field,
            cx,
        ))
        .child(flag_row(
            s,
            col,
            "storage-trades-autoload-cores",
            t!("storage.trades_autoload_cores").to_string(),
            t!("storage.trades_autoload_cores_hint").to_string(),
            cfg.autoload_cores,
            SettingsView::set_autoload_cores,
            cx,
        ))
        .child(flag_row(
            s,
            col,
            "storage-trades-autoload",
            t!("storage.trades_autoload").to_string(),
            t!("storage.trades_autoload_hint").to_string(),
            cfg.autoload_missing,
            SettingsView::set_autoload_missing,
            cx,
        ))
        .child(flag_row(
            s,
            col,
            "storage-trades-cleanup-startup",
            t!("storage.trades_cleanup_at_startup").to_string(),
            t!("storage.trades_cleanup_at_startup_hint").to_string(),
            cfg.cleanup_at_startup,
            SettingsView::set_cleanup_at_startup,
            cx,
        ))
        .child(trades_cleanup::cleanup_el(s, col, cleanup, busy, cx))
        .child(setting_row(
            s,
            LabelCol::Full,
            "storage-trades-more",
            t!("storage.db.trades.note").to_string(),
            None,
            t!("storage.trades_hint").to_string(),
            div().into_any_element(),
            cx,
        ));
    let compact = compact_btn(s, "trades-compact", paths::trades_db_path(), busy);
    settings_card(
        "storage-card-trades",
        t!("storage.db.trades.title").to_string(),
        size_line(info.trades),
        vec![compact],
        body,
        cx,
    )
}

/// A limit field followed by «без лимита» while its value is the zero that means no limit.
/// The note sits after the field, not in it: a long translation would squeeze the steppers.
fn no_limit_note(id: &'static str, field: AnyElement, off: bool, cx: &App) -> AnyElement {
    if !off {
        return field;
    }
    h_flex()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 8.0))
        .child(field)
        .child(
            div()
                .debug_selector(move || id.into())
                .whitespace_nowrap()
                .text_color(rgba_from(MoonPalette::active(cx).text_muted, 1.0))
                .child(t!("storage.version_limit_off").to_string()),
        )
        .into_any_element()
}

/// The margin is one of [`storage_cfg::TRADE_MARGIN_STEPS_S`], not a uniform increment.
fn margin_picker(s: &StorageCtx, secs: u32, cx: &App) -> MoonDropdown {
    let steps = storage_cfg::TRADE_MARGIN_STEPS_S;
    let current = storage_cfg::snap_trade_margin_s(secs);
    let mut options = Vec::with_capacity(steps.len());
    let mut selected = 0usize;
    for (i, step) in steps.iter().copied().enumerate() {
        if step == current {
            selected = i;
        }
        options.push((
            i,
            SharedString::from(format!("storage-margin-{step}")),
            SharedString::from(SettingsView::trades_margin_label(step)),
        ));
    }
    let trigger = SharedString::from(SettingsView::trades_margin_label(current));
    let view = s.weak.clone();
    let items = radio_items(
        options,
        selected,
        RadioMark::Check,
        move |app, row: usize| {
            let secs = storage_cfg::TRADE_MARGIN_STEPS_S
                .get(row)
                .copied()
                .unwrap_or(current);
            let _ = view.update(app, |this, cx| this.set_trades_margin_s(secs, cx));
        },
    );
    MoonDropdown::new("storage-margin")
        .label(trigger)
        .trigger_caret(true)
        // Label left, chevron at the right edge, as the page's selects read.
        .trigger_caret_end(true)
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::density(cx))
        .trigger_width(design::font_w(cx, 120.0))
        .menu_width_scaled(140.0)
        .items(items)
}

fn klines_card(s: &StorageCtx, info: &StorageInfo, cx: &App) -> AnyElement {
    let more = setting_row(
        s,
        LabelCol::Full,
        "storage-klines-more",
        t!("storage.db.klines.note").to_string(),
        None,
        t!("storage.klines_hint").to_string(),
        div().into_any_element(),
        cx,
    );
    settings_card(
        "storage-card-klines",
        t!("storage.db.klines.title").to_string(),
        size_line(info.klines),
        Vec::new(),
        more,
        cx,
    )
}

impl SettingsView {
    /// The Storage page. Opening the data folder is the header action; edits save at once.
    pub(super) fn storage_page(&mut self, cx: &mut Context<Self>) -> super::frame::PageParts {
        let open = MoonButton::new("storage-open")
            .variant(moon_ui::MoonButtonVariant::Neutral)
            .label(t!("storage.open_folder").to_string())
            .on_click(cx.listener(|_, _, _, _| open_folder(&paths::db_dir())))
            .render()
            .into_any_element();
        let body = self.storage_tab(cx).into_any_element();
        self.page_parts(super::Tab::Storage, body, vec![open], cx)
    }
}

/// Main file plus WAL, or the "no file yet" line.
fn size_line(size: Option<(u64, u64)>) -> String {
    match size {
        None => t!("storage.no_file").to_string(),
        Some((main, 0)) => t!("storage.size", size = moon_tg::size_text(main)).to_string(),
        Some((main, wal)) => t!(
            "storage.size_wal",
            size = moon_tg::size_text(main),
            wal = moon_tg::size_text(wal)
        )
        .to_string(),
    }
}

/// A muted mono readout of figures as separate stat items: it wraps only between
/// [`stat_items`], so a figure never breaks mid-number or away from its label.
fn mono_readout(id: &'static str, text: String, cx: &App) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .debug_selector(move || id.into())
        .w_full()
        .flex_wrap()
        .py(design::ui_px(cx, 4.0))
        .gap_x(design::ui_px(cx, 14.0))
        .font_family(design::mono())
        .text_color(rgba_from(p.text_muted, 1.0))
        .children(stat_items(&text).into_iter().enumerate().map(|(i, chunk)| {
            div()
                .debug_selector(move || format!("{id}-{i}"))
                .flex_none()
                .whitespace_nowrap()
                .child(chunk)
        }))
}

/// Split a figures line into the pieces it may wrap between: before each ` (` and each ` · `.
/// `"Strategies: 12 (deleted: 3) · Versions: 40"` gives `"Strategies: 12"`, `"(deleted: 3)"` and
/// `"· Versions: 40"`, so every figure stays on the line of its label.
///
/// Args:
///     text: The localized line.
///
/// Returns:
///     The pieces in order; the whole text when it has no break point.
pub(super) fn figure_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while let Some(at) = [" (", " \u{b7} "]
        .iter()
        .filter_map(|sep| rest.find(sep))
        .min()
    {
        chunks.push(rest[..at].to_string());
        rest = &rest[at + 1..];
    }
    chunks.push(rest.to_string());
    chunks.retain(|chunk| !chunk.is_empty());
    chunks
}

/// The figures of a readout line as stat items: its [`figure_chunks`] without the separators a
/// single line needs, `"Strategies: 12 (deleted: 3) · Versions: 40"` giving `"Strategies: 12"`,
/// `"deleted: 3"` and `"Versions: 40"`.
///
/// Args:
///     text: The localized line.
///
/// Returns:
///     The items in order.
pub(super) fn stat_items(text: &str) -> Vec<String> {
    figure_chunks(text)
        .into_iter()
        .map(|chunk| {
            let chunk = chunk.trim_start_matches('\u{b7}').trim();
            chunk
                .strip_prefix('(')
                .and_then(|inner| inner.strip_suffix(')'))
                .unwrap_or(chunk)
                .to_string()
        })
        .filter(|item| !item.is_empty())
        .collect()
}

/// The usage overview: total, path, stacked bar, legend.
fn usage_panel(parts: &[UsagePart], path: String, refresh: AnyElement, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let total: u64 = parts.iter().map(|part| part.bytes).sum();
    let shares: Vec<UsageShare> = parts
        .iter()
        .map(|part| UsageShare {
            value: part.bytes as f64,
            color: part.color,
            label: format!("{} {}", part.label, moon_tg::size_text(part.bytes)),
        })
        .collect();
    let mut legend = h_flex().w_full().flex_wrap().gap(design::ui_px(cx, 14.0));
    for part in parts {
        legend = legend.child(
            h_flex()
                .gap(design::ui_px(cx, 5.0))
                .items_center()
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .flex_none()
                        .rounded(px(2.0))
                        .bg(rgb(part.color)),
                )
                .child(part.label.clone())
                .child(
                    div()
                        .font_family(design::mono())
                        .text_color(muted)
                        .child(moon_tg::size_text(part.bytes)),
                ),
        );
    }
    v_flex()
        .id("storage-usage")
        .debug_selector(|| "storage-usage".into())
        .w_full()
        .gap(design::ui_px(cx, 8.0))
        .p(design::ui_px(cx, 12.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.shell_high, 1.0))
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .flex_wrap()
                .items_center()
                .gap(design::ui_px(cx, 8.0))
                .child(
                    h_flex()
                        .gap(design::ui_px(cx, 8.0))
                        .items_center()
                        .child(t!("storage.usage.total").to_string())
                        .child(
                            div()
                                .font_family(design::mono())
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(moon_tg::size_text(total)),
                        ),
                )
                // The basis is a floor: a narrow card wraps the path onto its own line rather
                // than collapsing it to one glyph per line beside the total and the button.
                .child(
                    div()
                        .flex_1()
                        .flex_basis(design::ui_px(cx, 200.0))
                        .min_w_0()
                        .font_family(design::mono())
                        .text_size(design::t_caption(cx))
                        .text_color(muted)
                        .child(path),
                )
                .child(refresh),
        )
        .child(usage_bar("storage-usage-bar", shares, total as f64))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(muted)
                .child(t!("storage.usage.legend").to_string()),
        )
        .child(legend)
        .into_any_element()
}

/// One database card: a header with the title and size and the actions at its top right, then
/// the card's settings.
fn settings_card(
    id: &'static str,
    title: String,
    size: String,
    actions: Vec<AnyElement>,
    body: impl IntoElement,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let muted = rgba_from(p.text_muted, 1.0);
    let mut head = h_flex()
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 8.0))
        .child(
            h_flex()
                .debug_selector(move || format!("{id}-head"))
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .gap_x(design::ui_px(cx, 8.0))
                .items_center()
                .child(
                    div()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_x(design::ui_px(cx, 6.0))
                        .font_family(design::mono())
                        .text_color(muted)
                        .children(
                            figure_chunks(&size)
                                .into_iter()
                                .map(|chunk| div().whitespace_nowrap().child(chunk)),
                        ),
                ),
        );
    if !actions.is_empty() {
        head = head.child(
            h_flex()
                .flex_none()
                .flex_wrap()
                .justify_end()
                .gap(design::ui_px(cx, 6.0))
                .children(actions),
        );
    }
    v_flex()
        .id(SharedString::from(id))
        .debug_selector(move || id.into())
        // Grows in its column: the cards of the shorter column share what the other is taller by,
        // so both columns end on one line.
        .flex_1()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .p(design::ui_px(cx, 12.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.shell_high, 1.0))
        .child(head)
        .child(body)
        .into_any_element()
}

/// UI state of the Storage page beyond `StorageEd`. Branch-owned: fields are added here, never on
/// `SettingsView`.
pub(super) struct StorageUi {
    /// Number field for `strategies.version_limit`. Zero means unlimited.
    pub(super) version_limit: Entity<moon_ui::MoonInputState>,
    /// Number field for `trade_replay.max_mb`. Zero means no ceiling.
    pub(super) max_mb: Entity<moon_ui::MoonInputState>,
    /// Number field for `trade_replay.long_position_min`, in minutes.
    pub(super) long_min: Entity<moon_ui::MoonInputState>,
    /// The number-field subscriptions are installed on the first render.
    wired: bool,
}

impl StorageUi {
    /// Fields seeded from the loaded storage config.
    pub(super) fn init(cfg: &storage_cfg::StorageCfg, window: &mut Window, cx: &mut App) -> Self {
        Self {
            version_limit: super::common::number_state(cfg.strategies.version_limit, window, cx),
            max_mb: super::common::number_state(cfg.trade_replay.max_mb, window, cx),
            long_min: super::common::number_state(cfg.trade_replay.long_position_min, window, cx),
            wired: false,
        }
    }
}

#[cfg(test)]
mod tests;
