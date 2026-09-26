//! Storage tab for local databases (`data/*.sqlite`): main/WAL sizes, row counts, maintenance
//! actions (compact/backup, the trade-tape cleanup of [`trades_cleanup`]), and
//! strategy-database settings.
//!
//! Changes apply immediately to `cfg/storage.toml` and live `strat_db` atomics, unlike draft-backed
//! tabs: Storage has its own file, and its recording toggle must take effect without Save. SQLite
//! statistics and maintenance run only on the background executor because counting a large replica
//! on the UI thread would freeze the interface.

mod trades_cleanup;

use gpui::*;
use moon_ui::{MoonButton, MoonPalette, StyledExt, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::{SettingsView, StatusMsg, open_folder, section, separator};
use crate::design;
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

/// Formats a byte count for display in KB, MB, or GB.
fn fmt_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.2} ГБ", b / KB / KB / KB)
    } else if b >= KB * KB {
        format!("{:.1} МБ", b / KB / KB)
    } else {
        format!("{:.0} КБ", b / KB)
    }
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
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let info = executor.spawn(async move { collect_info() }).await;
            let _ = cx.update(|cx| {
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
            let _ = cx.update(|cx| {
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

    /// Adjusts the version limit, clamps it to `0..=10000`, and updates live state and storage.toml.
    fn adjust_version_limit(&mut self, delta: i32, cx: &mut Context<Self>) {
        let v = (self.storage.cfg.strategies.version_limit as i32 + delta).clamp(0, 10_000) as u32;
        if self.storage.cfg.strategies.version_limit != v {
            self.storage.cfg.strategies.version_limit = v;
            moon_core::strat_db::set_version_limit(v);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    /// Adjusts the `trades.sqlite` ceiling in megabytes, clamps it to `0..=100_000` (zero is no
    /// ceiling), and updates live state and storage.toml.
    fn adjust_trades_max_mb(&mut self, delta: i32, cx: &mut Context<Self>) {
        let v = (self.storage.cfg.trade_replay.max_mb as i32 + delta).clamp(0, 100_000) as u32;
        if self.storage.cfg.trade_replay.max_mb != v {
            self.storage.cfg.trade_replay.max_mb = v;
            moon_core::market::trade_replay::trade_cache::set_max_mb(v);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    /// Moves the prints kept around a trade, per end, `delta` steps along
    /// `TRADE_MARGIN_STEPS_S` (30 s … 120 min, including 65 s, not a fixed amount), and updates
    /// live state and storage.toml.
    fn adjust_trades_margin_step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let v = storage_cfg::step_trade_margin_s(self.storage.cfg.trade_replay.margin_s, delta);
        if self.storage.cfg.trade_replay.margin_s != v {
            self.storage.cfg.trade_replay.margin_s = v;
            moon_core::market::trade_replay::set_margin_s(v);
            storage_cfg::save(&self.storage.cfg);
            // The excess is measured against the margin: the counts under the buttons move
            // with it.
            self.storage_cleanup_refresh(cx);
            cx.notify();
        }
    }

    /// Moves the minutes a position must be held to count as long, clamped to
    /// `LONG_POSITION_MIN_RANGE`, and updates live state and storage.toml. The cleanup's count
    /// moves with it: a long position claims its two ends, a short one its whole length.
    fn adjust_long_position_min(&mut self, delta: i32, cx: &mut Context<Self>) {
        let current = self.storage.cfg.trade_replay.long_position_min as i32;
        let v = storage_cfg::clamp_long_position_min((current + delta).max(0) as u32);
        if self.storage.cfg.trade_replay.long_position_min != v {
            self.storage.cfg.trade_replay.long_position_min = v;
            moon_core::market::trade_replay::set_long_position_min(v);
            storage_cfg::save(&self.storage.cfg);
            self.storage_cleanup_refresh(cx);
            cx.notify();
        }
    }

    /// The stepper's label for a margin: a whole number of minutes when the step divides by
    /// 60, seconds otherwise. 65 s is a step and must not read as "1 min", which is what 60 s
    /// already says.
    fn trades_margin_label(secs: u32) -> String {
        if secs % 60 == 0 {
            t!("storage.trades_min", min = secs / 60).to_string()
        } else {
            t!("storage.trades_sec", s = secs).to_string()
        }
    }

    /// Render storage controls with the version-limit stepper wrapping below its label when needed.
    pub(super) fn storage_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Start background snapshot collection when the tab is first shown.
        if self.storage.info.is_none() && !self.storage.inflight {
            self.storage_refresh(cx);
        }
        let p = MoonPalette::active(cx);
        let muted = rgba_from(p.text_muted, 1.0);
        let hint = |s: String| div().text_color(muted).child(s);
        let busy = self.storage.busy;
        let info = self.storage.info.clone().unwrap_or_default();
        let enabled = self.storage.cfg.strategies.enabled;
        let limit = self.storage.cfg.strategies.version_limit;
        let persist_trades = self.storage.cfg.trade_replay.persist_trades;
        let trades_max_mb = self.storage.cfg.trade_replay.max_mb;
        let trades_margin_s = self.storage.cfg.trade_replay.margin_s;
        let long_position_min = self.storage.cfg.trade_replay.long_position_min;
        let cleanup_at_startup = self.storage.cfg.trade_replay.cleanup_at_startup;

        let size_line = |sz: Option<(u64, u64)>| -> String {
            match sz {
                None => t!("storage.no_file").to_string(),
                Some((main, 0)) => t!("storage.size", size = fmt_size(main)).to_string(),
                Some((main, wal)) => t!(
                    "storage.size_wal",
                    size = fmt_size(main),
                    wal = fmt_size(wal)
                )
                .to_string(),
            }
        };
        let total: u64 = [info.reports, info.strategies, info.klines, info.trades]
            .iter()
            .flatten()
            .map(|(m, w)| m + w)
            .sum();

        let tool_btn = |id: &'static str, label: String, disabled: bool| {
            // Spaces around the label work around the fork's `MoonButton` `pad_x=0` bug, which
            // otherwise places text against the outline.
            MoonButton::new(id)
                .outline()
                .label(format!("  {label}  "))
                .disabled(disabled)
        };

        v_flex()
            .w_full()
            .gap_1()
            // ── General: data directory ─────────────────────────────────────
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(div().font_bold().child(t!("storage.data_dir").to_string()))
                    .child(
                        div()
                            .font_family(design::mono())
                            .text_color(muted)
                            .child(paths::db_dir().display().to_string()),
                    )
                    .child(
                        tool_btn("storage-open", t!("storage.open_folder").to_string(), false)
                            .on_click(cx.listener(|_, _, _, _| open_folder(&paths::db_dir())))
                            .render(),
                    )
                    .child(
                        tool_btn("storage-refresh", t!("storage.refresh").to_string(), false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage.info = None;
                                this.storage_refresh(cx);
                            }))
                            .render(),
                    ),
            )
            // MIXED NODE: `storage.total_size` combines the localized "Total:" label with the
            // figure in one text node (locales/storage.yml:22-25) — cannot style half of it, so
            // it stays mono.
            .child(
                hint(t!("storage.total_size", size = fmt_size(total)).to_string())
                    .font_family(design::mono()),
            )
            .child(separator(p, cx))
            // ── Reports ─────────────────────────────────────────────────────
            .child(section(&t!("storage.reports_title"), p, cx))
            // MIXED NODE: `size_line` and `storage.reports_rows` each combine a localized label
            // with a figure in one text node (locales/storage.yml:22-25, 43-46) — stays mono.
            .child(
                hint(format!(
                    "{} · {}",
                    size_line(info.reports),
                    match &info.report_rows {
                        Some(Ok(rows)) => t!("storage.reports_rows", rows = rows).to_string(),
                        // A missing replica and a pending snapshot are non-errors,
                        // but neither is evidence of zero rows.
                        Some(Err(moon_core::db::ReadFail::NotReady)) | None => "—".to_string(),
                        Some(Err(_)) => t!("common.db_read_failed_short").to_string(),
                    }
                ))
                .font_family(design::mono()),
            )
            .child(
                h_flex().child(
                    tool_btn("reports-compact", t!("storage.compact").to_string(), busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_op(cx, "storage.op_compact", || {
                                moon_core::db::maint::compact_db(&paths::reports_db_path())
                                    .map(|()| None)
                            });
                        }))
                        .render(),
                ),
            )
            .child(hint(t!("storage.reports_hint").to_string()))
            .child(separator(p, cx))
            // ── Strategies ──────────────────────────────────────────────────
            .child(section(&t!("storage.strategies_title"), p, cx))
            .child(
                moon_ui::MoonCheckbox::new("strat-db-enabled")
                    .checked(enabled)
                    .label(t!("storage.strategies_enabled").to_string())
                    .description(t!("storage.strategies_enabled_hint").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        if this.storage.cfg.strategies.enabled != v {
                            this.storage.cfg.strategies.enabled = v;
                            moon_core::strat_db::set_enabled(v);
                            storage_cfg::save(&this.storage.cfg);
                            cx.notify();
                        }
                    })),
            )
            // MIXED NODE: same as the reports readout above — stays mono.
            .child(
                hint(format!(
                    "{} · {}",
                    size_line(info.strategies),
                    t!(
                        "storage.strategies_rows",
                        live = info.strat_live,
                        deleted = info.strat_deleted,
                        versions = info.strat_versions
                    )
                ))
                .font_family(design::mono()),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(rgba_from(p.text, 1.0))
                            .child(t!("storage.version_limit").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "verlim",
                        true,
                        if limit == 0 {
                            t!("storage.version_limit_off").to_string()
                        } else {
                            limit.to_string()
                        },
                        1,
                        50,
                        Self::adjust_version_limit,
                    )),
            )
            .child(hint(t!("storage.version_limit_hint").to_string()))
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 8.0))
                    .child(
                        tool_btn("strat-compact", t!("storage.compact").to_string(), busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_op(cx, "storage.op_compact", || {
                                    moon_core::db::maint::compact_db(&paths::strategies_db_path())
                                        .map(|()| None)
                                });
                            }))
                            .render(),
                    )
                    .child(
                        tool_btn("strat-backup", t!("storage.backup").to_string(), busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_op(cx, "storage.op_backup", || {
                                    moon_core::strat_db::backup::backup_now().map(|_| None)
                                });
                            }))
                            .render(),
                    ),
            )
            .child(hint(t!("storage.strategies_hint").to_string()))
            .child(separator(p, cx))
            // ── Kline cache ─────────────────────────────────────────────────
            .child(section(&t!("storage.klines_title"), p, cx))
            // MIXED NODE: `size_line` combines label and figure — stays mono.
            .child(hint(size_line(info.klines)).font_family(design::mono()))
            .child(hint(t!("storage.klines_hint").to_string()))
            .child(separator(p, cx))
            // ── Trade prints ────────────────────────────────────────────────
            .child(section(&t!("storage.trades_title"), p, cx))
            .child(
                moon_ui::MoonCheckbox::new("trades-db-enabled")
                    .checked(persist_trades)
                    .label(t!("storage.trades_enabled").to_string())
                    .description(t!("storage.trades_enabled_hint").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        if this.storage.cfg.trade_replay.persist_trades != v {
                            this.storage.cfg.trade_replay.persist_trades = v;
                            moon_core::market::trade_replay::trade_cache::set_enabled(v);
                            storage_cfg::save(&this.storage.cfg);
                            cx.notify();
                        }
                    })),
            )
            // MIXED NODE: `size_line` combines label and figure — stays mono.
            .child(hint(size_line(info.trades)).font_family(design::mono()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(rgba_from(p.text, 1.0))
                            .child(t!("storage.trades_max_mb").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "trades-max-mb",
                        persist_trades,
                        if trades_max_mb == 0 {
                            t!("storage.version_limit_off").to_string()
                        } else {
                            t!("storage.trades_mb", mb = trades_max_mb).to_string()
                        },
                        64,
                        1024,
                        Self::adjust_trades_max_mb,
                    )),
            )
            .child(hint(t!("storage.trades_max_mb_hint").to_string()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(rgba_from(p.text, 1.0))
                            .child(t!("storage.trades_margin").to_string()),
                    )
                    // Enabled whether or not the file is on: the margin sizes what a window
                    // fetches and what a close copies, not only what the file keeps.
                    .child(self.stepper_controls(
                        cx,
                        "trades-margin-s",
                        true,
                        Self::trades_margin_label(trades_margin_s),
                        1,
                        3,
                        Self::adjust_trades_margin_step,
                    )),
            )
            .child(hint(t!("storage.trades_margin_hint").to_string()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(design::ui_px(cx, 8.0))
                    .items_center()
                    .child(
                        div()
                            .text_color(rgba_from(p.text, 1.0))
                            .child(t!("storage.trades_long_position").to_string()),
                    )
                    .child(self.stepper_controls(
                        cx,
                        "trades-long-position-min",
                        true,
                        t!("storage.trades_min", min = long_position_min).to_string(),
                        1,
                        5,
                        Self::adjust_long_position_min,
                    )),
            )
            .child(hint(t!("storage.trades_long_position_hint").to_string()))
            // The startup cleanup: read once per launch by the coordination tick, so the flip
            // takes effect at the next launch — which is what "at startup" says.
            .child(
                moon_ui::MoonCheckbox::new("trades-cleanup-at-startup")
                    .checked(cleanup_at_startup)
                    .label(t!("storage.trades_cleanup_at_startup").to_string())
                    .description(t!("storage.trades_cleanup_at_startup_hint").to_string())
                    .on_change(cx.listener(|this, v: &bool, _, cx| {
                        let v = *v;
                        if this.storage.cfg.trade_replay.cleanup_at_startup != v {
                            this.storage.cfg.trade_replay.cleanup_at_startup = v;
                            moon_core::market::trade_replay::set_cleanup_at_startup(v);
                            storage_cfg::save(&this.storage.cfg);
                            cx.notify();
                        }
                    })),
            )
            .child(self.trades_cleanup_controls(cx, p, busy))
            .child(
                h_flex().child(
                    tool_btn("trades-compact", t!("storage.compact").to_string(), busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_op(cx, "storage.op_compact", || {
                                moon_core::db::maint::compact_db(&paths::trades_db_path())
                                    .map(|()| None)
                            });
                        }))
                        .render(),
                ),
            )
            .child(hint(t!("storage.trades_hint").to_string()))
    }
}
