//! Storage tab: what the local databases (`data/*.sqlite`) hold and take on disk, the tape window
//! shared with the station, and the per-core delete tool.
//!
//! Nothing on the tab needs a button: sizes and counts recount whenever the tab opens, compaction
//! and the strategy backup run once a day on their own (`moon_core::backups`), the tape cleanup
//! and the missing-tape autoload at every start. The two tape settings apply at once through the
//! shared station window. SQLite statistics run only on the background executor because counting
//! a large replica on the UI thread would freeze the interface.

mod core_data;
mod trades_cleanup;

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonInputEvent, MoonNumberInput,
    MoonPalette, h_flex, rgba_from, v_flex,
};
use rust_i18n::t;

use super::common::{LabelCol, UsageShare, hint_row_in, num_input, usage_bar};
use super::{SettingsView, StatusMsg, Tab, open_folder};
use crate::design;
use crate::panels::{RadioMark, radio_items};
use moon_core::config::{paths, storage as storage_cfg};
use trades_cleanup::TapeCount;
pub(crate) use trades_cleanup::startup as trades_cleanup_startup;

/// The strategy database's head and history counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StratCounts {
    pub live: i64,
    pub deleted: i64,
    pub versions: i64,
}

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
    /// Strategy counts: `None` without a database file, `Err` when it could not be read.
    pub strat: Option<Result<StratCounts, String>>,
    /// Deals with tape: `None` while the count runs after the sizes, `Err` when it failed.
    pub tape: Option<Result<TapeCount, String>>,
    /// The newest completed strategy backup, UTC milliseconds.
    pub last_backup_ms: Option<i64>,
}

/// Storage-tab state: its configuration and background snapshot.
pub(super) struct StorageEd {
    pub cfg: storage_cfg::StorageCfg,
    pub info: Option<StorageInfo>,
    pub inflight: bool,
    /// Identity shared by the sizes and tape-count stages of the newest refresh.
    generation: u64,
    /// Whether the latest refresh is still counting tape after publishing sizes.
    tape_inflight: bool,
    /// Whether the per-core data deletion is running and its action buttons are disabled.
    pub busy: bool,
    /// The "Data by core" section.
    pub core_data: core_data::CoreDataEd,
}

impl StorageEd {
    /// Invalidate pending background results before a tab entry or completed deletion.
    fn reset_info(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.info = None;
        self.inflight = false;
        self.tape_inflight = false;
    }

    /// Ignore snapshots from a refresh invalidated by a newer tab entry or operation.
    fn receive_info(&mut self, generation: u64, info: StorageInfo) -> bool {
        if generation != self.generation {
            return false;
        }
        self.info = Some(info);
        self.inflight = false;
        true
    }

    /// Preserve externally reconciled window fields when a cached configuration is saved.
    fn sync_shared_window(&mut self) {
        let window = moon_core::market::trade_replay::window();
        self.cfg.trade_replay.margin_s = window.margin_s;
        self.cfg.trade_replay.long_position_min = window.long_position_min;
    }
}

/// One database in the usage bar, in bar order: reports, strategies, klines, trades.
///
/// The four `bytes` sum to the total the header prints. A missing file contributes zero,
/// and a WAL is part of its database.
pub(super) struct UsagePart {
    pub(super) bytes: u64,
    pub(super) color: u32,
    pub(super) label: String,
}

/// Segment colours from the storage mockup. Mid saturation so they read on light, dark and graphite.
const USAGE_COLOR: [u32; 4] = [0x3b82c4, 0x7c3aed, 0x0f9488, 0xb45309];

/// The narrowest content width at which a database row keeps its facts beside its name, unscaled;
/// below it, or wherever the facts would not fit beside the name, they wrap under the name (the
/// mockup's 560 px rule) instead of losing their figures to an ellipsis.
const DB_ROW_WIDE_MIN_W: f32 = 560.0;

/// Width of a database row's name column, unscaled (the mockup's 150 px).
const DB_NAME_W: f32 = 150.0;

/// How often the page redraws while the autoload fills tape, for its count and estimate.
const FILL_REDRAW: Duration = Duration::from_secs(2);

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
///     Reports, strategies, klines, then trades.
pub(super) fn usage_parts(info: &StorageInfo) -> [UsagePart; 4] {
    let bytes = [
        file_bytes(info.reports),
        file_bytes(info.strategies),
        file_bytes(info.klines),
        file_bytes(info.trades),
    ];
    let labels = [
        t!("storage.db.reports.title"),
        t!("storage.db.strategies.title"),
        t!("storage.db.klines.title"),
        t!("storage.db.trades.title"),
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
        generation: 0,
        tape_inflight: false,
        busy: false,
        core_data: core_data::CoreDataEd::default(),
    }
}

/// Collect storage sizes and row counts on the background executor; the tape count follows
/// separately ([`trades_cleanup::CleanupContext::tape_count`]).
///
/// The reports count preserves `NotReady` for an absent replica and `Failed`
/// for an unreadable replica so both remain distinct from a genuine zero.
fn collect_info() -> StorageInfo {
    use moon_core::db::maint::db_sizes;
    let sized = |p: &std::path::Path| p.exists().then(|| db_sizes(p));
    StorageInfo {
        reports: sized(&paths::reports_db_path()),
        strategies: sized(&paths::strategies_db_path()),
        klines: sized(&paths::klines_db_path()),
        trades: sized(&paths::trades_db_path()),
        report_rows: Some(moon_core::db::report_row_count()),
        strat: strat_counts(),
        tape: None,
        last_backup_ms: moon_core::strat_db::backup::latest_snapshot_ms(),
    }
}

/// The strategy database's counts, or `None` while it has no file yet.
fn strat_counts() -> Option<Result<StratCounts, String>> {
    if !paths::strategies_db_path().exists() {
        return None;
    }
    let Some(conn) = moon_core::strat_db::open_reader() else {
        return Some(Err(t!("common.db_read_failed_short").to_string()));
    };
    let count = |sql: &str| {
        conn.query_row(sql, [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())
    };
    Some((|| {
        Ok(StratCounts {
            live: count("SELECT COUNT(*) FROM strategies WHERE deleted=0")?,
            deleted: count("SELECT COUNT(*) FROM strategies WHERE deleted=1")?,
            versions: count("SELECT COUNT(*) FROM strategy_versions")?,
        })
    })())
}

impl SettingsView {
    /// Collects storage sizes and row counts in the background, then the deals with tape.
    ///
    /// Only the sizes stage blocks a new collection; stale stages discard their results.
    pub(super) fn storage_refresh(&mut self, cx: &mut Context<Self>) {
        if self.storage.inflight {
            return;
        }
        self.storage.inflight = true;
        self.storage.generation = self.storage.generation.wrapping_add(1);
        self.storage.tape_inflight = true;
        let generation = self.storage.generation;
        // The core table is always on the page, so its counts move with every snapshot.
        self.core_data_refresh(cx);
        let tape = trades_cleanup::CleanupContext::of(self.backend.read(cx));
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let info = executor.spawn(async move { collect_info() }).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.storage.receive_info(generation, info) {
                        cx.notify();
                    }
                });
            });
            // The count walks the replica against the file: after the sizes, so they never wait.
            let counted = executor
                .spawn(async move { tape.tape_count().map_err(|e| format!("{e:#}")) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.storage.generation != generation || !this.storage.tape_inflight {
                        return;
                    }
                    this.storage.tape_inflight = false;
                    if let Some(info) = &mut this.storage.info {
                        info.tape = Some(counted);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Runs a maintenance operation on the background executor, then refreshes data and status.
    /// A job may answer with a detail line for the status (`Some`), such as what a deletion
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
                    // Invalidate sizes and counts after every maintenance attempt.
                    this.storage.reset_info();
                    this.storage_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Sets the prints kept around a trade, snapped onto `TRADE_MARGIN_STEPS_S`, and writes
    /// storage.toml through the shared station window.
    pub(in crate::settings) fn set_trades_margin_s(&mut self, secs: u32, cx: &mut Context<Self>) {
        let old = moon_core::market::trade_replay::window();
        self.backend.update(cx, |backend, cx| {
            backend.tape_window_set(
                moon_core::station_api::TapeWindow {
                    margin_s: secs,
                    ..old
                },
                cx,
            );
        });
        self.storage.sync_shared_window();
        if moon_core::market::trade_replay::window() != old {
            cx.notify();
        }
    }

    /// Sets the minutes a position must be held to count as long, clamped to
    /// `LONG_POSITION_MIN_RANGE`.
    pub(in crate::settings) fn set_long_position_min(
        &mut self,
        minutes: u32,
        cx: &mut Context<Self>,
    ) {
        let old = moon_core::market::trade_replay::window();
        self.backend.update(cx, |backend, cx| {
            backend.tape_window_set(
                moon_core::station_api::TapeWindow {
                    long_position_min: minutes,
                    ..old
                },
                cx,
            );
        });
        self.storage.sync_shared_window();
        if moon_core::market::trade_replay::window() != old {
            cx.notify();
        }
    }

    /// Refresh the once-seeded input before either settings page renders.
    pub(in crate::settings) fn storage_input_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.storage.sync_shared_window();
        self.storage_ui.sync_long_input(
            self.storage.cfg.trade_replay.long_position_min,
            window,
            cx,
        );
    }

    /// Note the page the window shows: leaving Storage makes its next opening recount.
    pub(in crate::settings) fn storage_note_tab(&mut self, active: Tab) {
        if active != Tab::Storage {
            self.storage_ui.shown = false;
        }
    }

    /// Subscribe the number field once. Each edit saves storage.toml immediately.
    fn wire_storage_numbers(&mut self, cx: &mut Context<Self>) {
        if self.storage_ui.wired {
            return;
        }
        self.storage_ui.wired = true;
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
                if !this.storage_ui.long_edit(minutes) {
                    return;
                }
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

    /// While the autoload fills tape, keep one redraw queued so its count and estimate move.
    fn storage_fill_redraw(&mut self, cx: &mut Context<Self>) {
        if self.storage_ui.fill_redraw {
            return;
        }
        self.storage_ui.fill_redraw = true;
        cx.spawn(async move |this, cx| {
            let timer = cx.update(|cx| cx.background_executor().timer(FILL_REDRAW));
            timer.await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.storage_ui.fill_redraw = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }
}

impl SettingsView {
    /// The Storage tab: recounts on opening, gathers the page; the layout is
    /// [`storage_body_el`]'s.
    pub(super) fn storage_tab(&mut self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        self.storage.sync_shared_window();
        if !self.storage_ui.shown {
            self.storage_ui.shown = true;
            self.storage.reset_info();
        }
        if self.storage.info.is_none() && !self.storage.inflight {
            self.storage_refresh(cx);
        }
        self.wire_storage_numbers(cx);
        let pending = crate::analytics::tape_autoload::pending_rows().filter(|&n| n > 0);
        let fill = pending.map(|n| (n, self.storage_ui.fill_eta(n, Instant::now())));
        if pending.is_some() {
            self.storage_fill_redraw(cx);
        } else {
            self.storage_ui.fill_sample = None;
        }
        let core = self.core_data_section(cx);
        let view = StorageView {
            loading: self.storage.info.is_none(),
            info: self.storage.info.clone().unwrap_or_default(),
            cfg: self.storage.cfg.clone(),
            long_min: self.storage_ui.long_min.clone(),
            station: crate::backend::station::known_cached(),
            fill,
            core,
        };
        let open = self.more_open.clone();
        let ctx = StorageCtx {
            weak: cx.entity().downgrade(),
            open: &open,
        };
        storage_body_el(view, &ctx, width, cx)
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
    /// No snapshot yet: the database rows are dimmed under the counting line.
    pub(super) loading: bool,
    pub(super) cfg: storage_cfg::StorageCfg,
    pub(super) long_min: Entity<moon_ui::MoonInputState>,
    /// A station is set up: the tape window is shared with it.
    pub(super) station: bool,
    /// While the autoload fills tape: the deals left and, after two samples, the seconds left.
    pub(super) fill: Option<(usize, Option<u64>)>,
    /// The "Data by core" section.
    pub(super) core: AnyElement,
}

/// The Storage page in one column: the disk card, the tape card, then the core table's card.
///
/// Args:
///     view: What to draw.
///     s: Where edits go.
///     width: The page body's content width, for the database rows' narrow layout.
///     cx: Application context.
pub(super) fn storage_body_el(
    view: StorageView,
    s: &StorageCtx,
    width: f32,
    cx: &App,
) -> AnyElement {
    let StorageView {
        info,
        loading,
        cfg,
        long_min,
        station,
        fill,
        core,
    } = view;
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, CARD_GAP))
        .child(total_card(&info, loading, width, cx))
        .child(tape_card(s, &cfg, &long_min, station, fill, cx))
        .child(card("storage-core-card", cx).child(core))
        .into_any_element()
}

/// Vertical gap between the page's cards, unscaled.
const CARD_GAP: f32 = 12.0;

/// A card's horizontal padding, unscaled.
const CARD_PAD_X: f32 = 14.0;

/// One card of the page: border, rounded corners, the raised shell, the mockup's padding.
fn card(id: &'static str, cx: &App) -> Stateful<Div> {
    let p = MoonPalette::active(cx);
    v_flex()
        .id(id)
        .debug_selector(move || id.into())
        .w_full()
        // Prose in the UI face; figures set the mono face themselves.
        .font_family(design::ui_font())
        .text_size(design::t_body(cx))
        .px(design::ui_px(cx, CARD_PAD_X))
        .py(design::ui_px(cx, 12.0))
        .gap(design::ui_px(cx, 8.0))
        .border_1()
        .border_color(rgba_from(p.border, 1.0))
        .rounded(design::r_container(cx))
        .bg(rgba_from(p.shell_high, 1.0))
}

/// A card's header line: the title, then `rest` (figures, a tag, a right-hand action).
fn card_head(id: &'static str, title: String, cx: &App) -> Div {
    h_flex()
        .w_full()
        .flex_wrap()
        .items_center()
        .gap(design::ui_px(cx, 10.0))
        .child(
            div()
                .debug_selector(move || format!("{id}-title"))
                .flex_none()
                .whitespace_nowrap()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
}

/// A muted caption line led by a green dot: something that runs on its own.
fn auto_line(id: &'static str, text: String, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    h_flex()
        .debug_selector(move || id.into())
        .w_full()
        .items_start()
        .gap(design::ui_px(cx, 6.0))
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(p.text_muted, 1.0))
        .child(
            div()
                .flex_none()
                .mt(design::ui_px(cx, 5.0))
                .size(design::ui_px(cx, 6.0))
                .rounded_full()
                .bg(rgba_from(p.green, 1.0)),
        )
        .child(
            super::common::prose(format!("{id}-text"), text, true, cx)
                .flex_1()
                .text_size(design::t_caption(cx)),
        )
        .into_any_element()
}

/// A caption status line in `color`: counting and filling amber, a failure red.
fn status_line(id: &'static str, text: String, color: u32, cx: &App) -> AnyElement {
    div()
        .debug_selector(move || id.into())
        .w_full()
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(color, 1.0))
        .child(text)
        .into_any_element()
}

/// Card "Всего на диске": the total, the usage bar and legend, one row per database, the counting
/// or failure line, and the daily maintenance line.
fn total_card(info: &StorageInfo, loading: bool, width: f32, cx: &App) -> AnyElement {
    let p = MoonPalette::active(cx);
    let parts = usage_parts(info);
    let total: u64 = parts.iter().map(|part| part.bytes).sum();
    let open = MoonButton::new("storage-open")
        .variant(MoonButtonVariant::Neutral)
        .label(format!("  {}  ", t!("storage.open_folder")))
        .on_click(|_, _, _| open_folder(&paths::db_dir()))
        .render()
        .into_any_element();
    let head = card_head("storage-total", t!("storage.usage.total").to_string(), cx)
        .child(
            div()
                .debug_selector(|| "storage-total-size".into())
                .whitespace_nowrap()
                .font_family(design::mono())
                .child(moon_tg::size_text(total)),
        )
        .child(div().flex_1())
        .child(open);
    let shares: Vec<UsageShare> = parts
        .iter()
        .map(|part| UsageShare {
            value: part.bytes as f64,
            color: part.color,
            label: format!("{} {}", part.label, moon_tg::size_text(part.bytes)),
        })
        .collect();
    let mut legend = h_flex()
        .w_full()
        .flex_wrap()
        .gap_x(design::ui_px(cx, 16.0))
        .gap_y(design::ui_px(cx, 6.0))
        .text_size(design::t_caption(cx));
    for part in &parts {
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
                        .child(moon_tg::size_text(part.bytes)),
                ),
        );
    }
    let rows = db_rows(info, width, cx);
    let mut out = card("storage-total", cx)
        .child(head)
        .child(usage_bar("storage-usage-bar", shares, total as f64))
        .child(legend)
        .child(
            div()
                .w_full()
                .when(loading, |el| el.opacity(0.45))
                .child(rows),
        );
    if loading {
        out = out.child(status_line(
            "storage-loading",
            t!("storage.counting").to_string(),
            p.amber,
            cx,
        ));
    } else if let Some((db, err)) = first_failure(info) {
        out = out.child(status_line(
            "storage-error",
            t!("storage.count_failed", db = db, err = err).to_string(),
            p.red,
            cx,
        ));
    }
    out.child(auto_line(
        "storage-auto",
        maint_text(info.last_backup_ms),
        cx,
    ))
    .into_any_element()
}

/// The first database whose facts could not be read, by title, and why.
fn first_failure(info: &StorageInfo) -> Option<(String, String)> {
    if let Some(Err(moon_core::db::ReadFail::Failed { .. })) = &info.report_rows {
        return Some((
            t!("storage.db.reports.title").to_string(),
            t!("common.db_read_failed_short").to_string(),
        ));
    }
    if let Some(Err(e)) = &info.strat {
        return Some((t!("storage.db.strategies.title").to_string(), e.clone()));
    }
    if let Some(Err(e)) = &info.tape {
        return Some((t!("storage.db.trades.title").to_string(), e.clone()));
    }
    None
}

/// "Сжатие и бэкап — сами, раз в сутки в 14:00. Последний бэкап: …": the coordinator's noon-UTC
/// slot in the local clock, and the newest completed backup.
fn maint_text(last_backup_ms: Option<i64>) -> String {
    use chrono::{Local, TimeZone, Utc};
    let noon = Utc
        .from_utc_datetime(
            &Utc::now()
                .date_naive()
                .and_hms_opt(12, 0, 0)
                .unwrap_or_default(),
        )
        .with_timezone(&Local);
    let auto = t!(
        "storage.auto_maint",
        time = noon.format("%H:%M").to_string()
    );
    let last = match last_backup_ms.and_then(|ms| Local.timestamp_millis_opt(ms).single()) {
        Some(at) if at.date_naive() == Local::now().date_naive() => t!(
            "storage.last_backup",
            when = t!("storage.today", time = at.format("%H:%M").to_string())
        ),
        Some(at) => t!(
            "storage.last_backup",
            when = at.format("%d.%m.%Y %H:%M").to_string()
        ),
        None => t!("storage.no_backup_yet"),
    };
    format!("{auto} {last}")
}

/// A count in its plural form of `base` (`<base>_one|_few|_many`).
fn plural(base: &str, n: i64) -> String {
    let n = n.max(0) as u64;
    let key = crate::display_text::plural_key(base, n);
    t!(key.as_str(), n = n).to_string()
}

/// The four database rows: name, figures, size.
fn db_rows(info: &StorageInfo, width: f32, cx: &App) -> AnyElement {
    let counting = || t!("storage.counting").to_string();
    let reports = match &info.report_rows {
        Some(Ok(rows)) => plural("storage.facts.reports", *rows),
        Some(Err(_)) => "—".to_string(),
        None => counting(),
    };
    let strategies = match &info.strat {
        Some(Ok(c)) => [
            plural("storage.facts.strategies", c.live),
            plural("storage.facts.deleted", c.deleted),
            plural("storage.facts.versions", c.versions),
        ]
        .join(" \u{b7} "),
        Some(Err(_)) | None => "—".to_string(),
    };
    let trades = match &info.tape {
        Some(Ok(c)) => format!(
            "{} \u{b7} {}",
            if c.incomplete_markets > 0 {
                t!(
                    "storage.facts.lower_bound",
                    value = plural("storage.facts.trades", c.with_tape as i64)
                )
                .to_string()
            } else {
                plural("storage.facts.trades", c.with_tape as i64)
            },
            if c.incomplete_markets > 0 {
                t!(
                    "storage.facts.lower_bound",
                    value = t!("storage.facts.trades_long", n = c.long)
                )
                .to_string()
            } else {
                t!("storage.facts.trades_long", n = c.long).to_string()
            }
        ),
        Some(Err(_)) => "—".to_string(),
        None => counting(),
    };
    let rows = [
        ("reports", "storage.db.reports.title", reports, info.reports),
        (
            "strategies",
            "storage.db.strategies.title",
            strategies,
            info.strategies,
        ),
        (
            "klines",
            "storage.db.klines.title",
            t!("storage.facts.klines").to_string(),
            info.klines,
        ),
        ("trades", "storage.db.trades.title", trades, info.trades),
    ];
    // The card's content box: the page width less the card's padding and border.
    let room = width - design::ui_value(cx, 2.0 * CARD_PAD_X + 2.0);
    let floor = width < design::ui_value(cx, DB_ROW_WIDE_MIN_W);
    v_flex()
        .debug_selector(|| "storage-dbs".into())
        .w_full()
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(i, (key, title, facts, size))| {
                    db_row(
                        key,
                        t!(title).to_string(),
                        facts,
                        size,
                        i == 0,
                        if floor { 0.0 } else { room },
                        cx,
                    )
                }),
        )
        .into_any_element()
}

/// One database row, selector `storage-db-<key>`: name, mono facts, size on one line when `room`
/// holds all three; otherwise the facts wrap under the name with the size kept on its line.
fn db_row(
    key: &'static str,
    name: String,
    facts: String,
    size: Option<(u64, u64)>,
    first: bool,
    room: f32,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let id = format!("storage-db-{key}");
    let sel = |part: &str| {
        let s = format!("{id}{part}");
        move || s.clone()
    };
    let size = size
        .map(|(main, wal)| moon_tg::size_text(main.saturating_add(wal)))
        .unwrap_or_else(|| t!("storage.no_file").to_string());
    let gap = design::ui_value(cx, 12.0);
    let one_line = design::ui_value(cx, DB_NAME_W)
        .max(design::ui_body_text_width(cx, &name, 600.0))
        + design::mono_caption_text_width(cx, &facts, 400.0)
        + design::mono_caption_text_width(cx, &size, 400.0)
        + 2.0 * gap;
    let narrow = one_line > room;
    let name_el = div()
        .debug_selector(sel("-name"))
        .flex_none()
        .min_w(design::ui_px(cx, DB_NAME_W))
        .whitespace_nowrap()
        .font_weight(FontWeight::SEMIBOLD)
        .child(name);
    let size_el = div()
        .debug_selector(sel("-size"))
        .flex_none()
        .whitespace_nowrap()
        .font_family(design::mono())
        .text_size(design::t_caption(cx))
        .child(size);
    let facts_el = div()
        .debug_selector(sel("-facts"))
        .min_w_0()
        .font_family(design::mono())
        .text_size(design::t_caption(cx))
        .text_color(rgba_from(p.text_muted, 1.0));
    let row = div()
        .debug_selector(sel(""))
        .w_full()
        .py(design::ui_px(cx, 8.0))
        .when(!first, |el| {
            el.border_t_1().border_color(rgba_from(p.border, 1.0))
        });
    if narrow {
        row.child(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 4.0))
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap(design::ui_px(cx, 12.0))
                        .child(name_el.flex_1().min_w_0().whitespace_normal())
                        .child(size_el),
                )
                .child(facts_el.w_full().child(facts)),
        )
        .into_any_element()
    } else {
        // The facts are the width-owning row's own child, truncating only by a rounding hair: a
        // flex box between them collapses the text to its ellipsis.
        row.child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(name_el)
                .child(facts_el.flex_1().truncate().child(facts))
                .child(size_el),
        )
        .into_any_element()
    }
}

/// Card "Лента сделок": whom the window is shared with, its two settings with a one-line hint
/// each, the fill line while the autoload runs, and what runs on its own.
fn tape_card(
    s: &StorageCtx,
    cfg: &storage_cfg::StorageCfg,
    long_min_state: &Entity<moon_ui::MoonInputState>,
    station: bool,
    fill: Option<(usize, Option<u64>)>,
    cx: &App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    let tag = if station {
        t!("storage.tape.shared")
    } else {
        t!("storage.tape.local_only")
    };
    let head = card_head(
        "storage-tape",
        t!("storage.db.trades.title").to_string(),
        cx,
    )
    .child(
        div()
            .debug_selector(|| "storage-tape-tag".into())
            .min_w_0()
            .text_size(design::t_caption(cx))
            .text_color(rgba_from(p.text_muted, 1.0))
            .child(tag.to_string()),
    );
    let long_field = num_input(
        "storage-trades-long-n",
        long_min_state,
        1.0..=120.0,
        1.0,
        Some(t!("storage.unit_min").to_string()),
    );
    let noop: super::common::Action = Rc::new(|_, _| {});
    let row = |id: &'static str, label: String, hint: String, control: AnyElement| {
        hint_row_in(
            LabelCol::Full,
            id,
            label,
            Some(hint),
            None,
            control,
            false,
            noop.clone(),
            cx,
        )
    };
    let margin = row(
        "storage-trades-margin",
        t!("storage.trades_margin").to_string(),
        t!("storage.trades_margin_hint").to_string(),
        margin_picker(s, cfg.trade_replay.margin_s, cx).into_any_element(),
    );
    let long = row(
        "storage-trades-long",
        t!("storage.trades_long_position").to_string(),
        t!("storage.trades_long_position_hint").to_string(),
        long_field,
    );
    let mut out = card("storage-tape", cx).child(head).child(margin).child(
        div()
            .w_full()
            .border_t_1()
            .border_color(rgba_from(p.border, 1.0))
            .child(long),
    );
    if let Some((n, eta)) = fill {
        let text = match eta {
            Some(secs) => t!(
                "storage.tape.fill_eta",
                n = n,
                eta = crate::display_text::fmt_duration_short(secs as f64)
            ),
            None => t!("storage.tape.fill", n = n),
        };
        out = out.child(status_line(
            "storage-tape-fill",
            text.to_string(),
            p.amber,
            cx,
        ));
    }
    let auto = if station {
        t!("storage.tape.auto")
    } else {
        t!("storage.tape.auto_local")
    };
    out.child(auto_line("storage-tape-auto", auto.to_string(), cx))
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
        .fit_menu_width(140.0, 560.0)
        .items(items)
}

impl SettingsView {
    /// The Storage page. Opening the data folder sits in the disk card; edits save at once.
    pub(super) fn storage_page(
        &mut self,
        body_width: f32,
        cx: &mut Context<Self>,
    ) -> super::frame::PageParts {
        let width = (body_width - 2.0 * design::ui_value(cx, super::render::BODY_PAD)).max(0.0);
        let body = self.storage_tab(width, cx).into_any_element();
        self.page_parts(Tab::Storage, body, Vec::new(), cx)
    }
}

/// UI state of the Storage page beyond `StorageEd`. Branch-owned: fields are added here, never on
/// `SettingsView`.
pub(super) struct StorageUi {
    /// Number field for `trade_replay.long_position_min`, in minutes.
    pub(super) long_min: Entity<moon_ui::MoonInputState>,
    /// Last displayed threshold; programmatic Change events must not save it back.
    long_shown: Option<u32>,
    /// The number-field subscription is installed on the first render.
    wired: bool,
    /// The page is on screen; cleared by [`SettingsView::storage_note_tab`] when another shows.
    shown: bool,
    /// The fill's first sample: when it was taken and how many deals were left then.
    fill_sample: Option<(Instant, usize)>,
    /// A redraw for the fill line is queued.
    fill_redraw: bool,
}

impl StorageUi {
    /// Reject Change emitted by a programmatic refresh, retaining clamping writeback for typing.
    fn long_edit(&mut self, typed: u32) -> bool {
        if self.long_shown == Some(typed) {
            return false;
        }
        self.long_shown = (typed == storage_cfg::clamp_long_position_min(typed)).then_some(typed);
        true
    }

    /// Show an external threshold before its queued Change event can be observed.
    fn sync_long_input(&mut self, value: u32, window: &mut Window, cx: &mut App) {
        if self.long_shown != Some(value) {
            self.long_shown = Some(value);
            self.long_min.update(cx, |state, cx| {
                state.set_value(value.to_string(), window, cx)
            });
        }
    }

    /// Seconds the fill still needs at the rate seen since its first sample, or `None` until a
    /// second sample shows deals done. A count that grows starts the sampling over.
    fn fill_eta(&mut self, left: usize, now: Instant) -> Option<u64> {
        match self.fill_sample {
            Some((at, first)) if left < first => {
                let rate = (first - left) as f64 / now.duration_since(at).as_secs_f64();
                (rate.is_finite() && rate > 0.0).then(|| (left as f64 / rate).ceil() as u64)
            }
            Some((_, first)) if left == first => None,
            _ => {
                self.fill_sample = Some((now, left));
                None
            }
        }
    }

    /// Fields seeded from the loaded storage config.
    pub(super) fn init(cfg: &storage_cfg::StorageCfg, window: &mut Window, cx: &mut App) -> Self {
        Self {
            long_min: super::common::number_state(cfg.trade_replay.long_position_min, window, cx),
            long_shown: Some(cfg.trade_replay.long_position_min),
            wired: false,
            shown: false,
            fill_sample: None,
            fill_redraw: false,
        }
    }
}

#[cfg(test)]
mod tests;
