//! Вкладка «Хранилище» — локальные БД (`data/*.sqlite`): размеры (файл + WAL),
//! счётчики строк, обслуживание (Сжать/Бэкап), настройки БД стратегий.
//!
//! Правки применяются СРАЗУ (пишутся в `cfg/storage.toml` и в живые атомики
//! `strat_db`), в отличие от draft-вкладок: у хранилища свой файл, а тогл записи
//! должен действовать без «Сохранить». Сборы статистики и VACUUM ходят в SQLite —
//! только на background executor (COUNT по большой реплике фризил бы UI).

use gpui::*;
use moon_ui::{MoonButton, MoonCheckboxSize, MoonPalette, StyledExt, h_flex, rgba_from, v_flex};
use rust_i18n::t;

use super::{SettingsView, StatusMsg, section, separator};
use crate::design;
use moon_core::config::{paths, storage as storage_cfg};

/// Снимок состояния хранилища (собирается фоном).
#[derive(Clone, Default)]
pub(super) struct StorageInfo {
    /// (размер файла, размер -wal) по БД; None = файла нет.
    pub reports: Option<(u64, u64)>,
    pub strategies: Option<(u64, u64)>,
    pub klines: Option<(u64, u64)>,
    pub report_rows: i64,
    pub strat_live: i64,
    pub strat_deleted: i64,
    pub strat_versions: i64,
}

/// Состояние вкладки: конфиг хранилища + фоновый снимок.
pub(super) struct StorageEd {
    pub cfg: storage_cfg::StorageCfg,
    pub info: Option<StorageInfo>,
    pub inflight: bool,
    /// Идущая операция обслуживания (кнопки задизейблены).
    pub busy: bool,
}

pub(super) fn build() -> StorageEd {
    StorageEd {
        cfg: storage_cfg::load(),
        info: None,
        inflight: false,
        busy: false,
    }
}

/// Собрать снимок хранилища (вызывается на background executor).
fn collect_info() -> StorageInfo {
    use moon_core::db::maint::db_sizes;
    let sized = |p: &std::path::Path| p.exists().then(|| db_sizes(p));
    let mut out = StorageInfo {
        reports: sized(&paths::reports_db_path()),
        strategies: sized(&paths::strategies_db_path()),
        klines: sized(&paths::klines_db_path()),
        ..Default::default()
    };
    if let Some(conn) = moon_core::db::open_reader() {
        out.report_rows = conn
            .query_row("SELECT COUNT(*) FROM orders_rep", [], |r| r.get(0))
            .unwrap_or(0);
    }
    if let Some(conn) = moon_core::strat_db::open_reader() {
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0);
        out.strat_live = count("SELECT COUNT(*) FROM strategies WHERE deleted=0");
        out.strat_deleted = count("SELECT COUNT(*) FROM strategies WHERE deleted=1");
        out.strat_versions = count("SELECT COUNT(*) FROM strategy_versions");
    }
    out
}

/// Человекочитаемый размер (КБ/МБ/ГБ).
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

/// Открыть папку в системном файловом менеджере.
fn open_folder(path: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

impl SettingsView {
    /// Фоновый сбор снимка хранилища (размеры + COUNT'ы). Дедуп: не чаще одного
    /// в полёте; повторный вызов при inflight игнорируется.
    pub(super) fn storage_refresh(&mut self, cx: &mut Context<Self>) {
        if self.storage.inflight {
            return;
        }
        self.storage.inflight = true;
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

    /// Операция обслуживания на background executor + рефреш и статус по концу.
    fn storage_op(
        &mut self,
        cx: &mut Context<Self>,
        op_key: &'static str,
        job: impl FnOnce() -> anyhow::Result<()> + Send + 'static,
    ) {
        if self.storage.busy {
            return;
        }
        self.storage.busy = true;
        self.status = Some((StatusMsg::Text(t!("storage.busy", op = t!(op_key)).to_string()), false));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor.spawn(async move { job() }).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.storage.busy = false;
                    this.status = Some(match result {
                        Ok(()) => (
                            StatusMsg::Text(t!("storage.op_done", op = t!(op_key)).to_string()),
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
                    this.storage.info = None; // размеры изменились — пересобрать
                    this.storage_refresh(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Изменить лимит версий (клампим 0..=10000): живой атомик + storage.toml.
    fn adjust_version_limit(&mut self, delta: i32, cx: &mut Context<Self>) {
        let v = (self.storage.cfg.strategies.version_limit as i32 + delta).clamp(0, 10_000) as u32;
        if self.storage.cfg.strategies.version_limit != v {
            self.storage.cfg.strategies.version_limit = v;
            moon_core::strat_db::set_version_limit(v);
            storage_cfg::save(&self.storage.cfg);
            cx.notify();
        }
    }

    pub(super) fn storage_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Первый показ вкладки — запускаем фоновый сбор снимка.
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

        let size_line = |sz: Option<(u64, u64)>| -> String {
            match sz {
                None => t!("storage.no_file").to_string(),
                Some((main, 0)) => t!("storage.size", size = fmt_size(main)).to_string(),
                Some((main, wal)) => {
                    t!("storage.size_wal", size = fmt_size(main), wal = fmt_size(wal)).to_string()
                }
            }
        };
        let total: u64 = [info.reports, info.strategies, info.klines]
            .iter()
            .flatten()
            .map(|(m, w)| m + w)
            .sum();

        let tool_btn = |id: &'static str, label: String, disabled: bool| {
            MoonButton::new(id)
                .outline()
                .small()
                .label(label)
                .disabled(disabled)
        };

        v_flex()
            .w_full()
            .gap_1()
            // ── Общий блок: папка данных ────────────────────────────────────
            .child(
                h_flex()
                    .gap(design::ui_px(cx, 10.0))
                    .items_center()
                    .child(div().font_bold().child(t!("storage.data_dir").to_string()))
                    .child(
                        div()
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
            .child(hint(t!("storage.total_size", size = fmt_size(total)).to_string()))
            .child(separator(p, cx))
            // ── Отчёты ──────────────────────────────────────────────────────
            .child(section(&t!("storage.reports_title"), p, cx))
            .child(hint(format!(
                "{} · {}",
                size_line(info.reports),
                t!("storage.reports_rows", rows = info.report_rows)
            )))
            .child(
                h_flex().child(
                    tool_btn("reports-compact", t!("storage.compact").to_string(), busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.storage_op(cx, "storage.op_compact", || {
                                moon_core::db::maint::compact_db(&paths::reports_db_path())
                            });
                        }))
                        .render(),
                ),
            )
            .child(hint(t!("storage.reports_hint").to_string()))
            .child(separator(p, cx))
            // ── Стратегии ───────────────────────────────────────────────────
            .child(section(&t!("storage.strategies_title"), p, cx))
            .child(
                moon_ui::MoonCheckbox::new("strat-db-enabled")
                    .checked(enabled)
                    .label(t!("storage.strategies_enabled").to_string())
                    .size(MoonCheckboxSize::Normal)
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
            .child(hint(t!("storage.strategies_enabled_hint").to_string()))
            .child(hint(format!(
                "{} · {}",
                size_line(info.strategies),
                t!(
                    "storage.strategies_rows",
                    live = info.strat_live,
                    deleted = info.strat_deleted,
                    versions = info.strat_versions
                )
            )))
            .child(
                h_flex()
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
                                });
                            }))
                            .render(),
                    )
                    .child(
                        tool_btn("strat-backup", t!("storage.backup").to_string(), busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.storage_op(cx, "storage.op_backup", || {
                                    let src = paths::strategies_db_path();
                                    anyhow::ensure!(src.exists(), "БД стратегий ещё не создана");
                                    let stamp = chrono_like_stamp();
                                    let dst = paths::db_dir()
                                        .join(format!("strategies-backup-{stamp}.sqlite"));
                                    moon_core::db::maint::backup_db(&src, &dst)
                                });
                            }))
                            .render(),
                    ),
            )
            .child(hint(t!("storage.strategies_hint").to_string()))
            .child(separator(p, cx))
            // ── Кэш свечей ──────────────────────────────────────────────────
            .child(section(&t!("storage.klines_title"), p, cx))
            .child(hint(size_line(info.klines)))
            .child(hint(t!("storage.klines_hint").to_string()))
    }
}

/// Штамп для имени бэкапа: локальное время `ГГГГММДД-ЧЧММСС` без зависимостей.
fn chrono_like_stamp() -> String {
    let ms = moon_core::util::now_unix_ms_i64();
    let secs = ms / 1000;
    // Дни с эпохи → Г/М/Д (алгоритм civil_from_days, UTC — для имени файла хватит).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}
