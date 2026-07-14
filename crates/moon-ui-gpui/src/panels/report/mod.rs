//! Панель «Отчёт» — порт egui `src/dock/report_view.rs`. Таблица закрытых сделок
//! (ордеров) из локальной SQLite. Фильтры (ядро/монета/сторона/даты) + выбор колонок
//! сверху, ИТОГО за период снизу, generic-таблица по всем колонкам БД с сортировкой
//! по клику на заголовок. Автообновление по счётчику-генерации writer'а (Backend.reports).
//!
//! По функционалу разнесено: состояние/запросы/жизненный цикл — здесь, поля-списки и
//! меню колонок — [`controls`], форматирование колонок/ячеек/заголовков — [`columns`].

mod columns;
mod controls;
mod export;

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDataCell,
    MoonDataRow, MoonDataTable, MoonDataTableColumn, MoonDataTableState, MoonDropdown, MoonInput,
    MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette, MoonText, MoonTone,
    Panel, PanelEvent, PanelState, StyledExt, h_flex, v_flex,
};
use rusqlite::Connection;
use rusqlite::types::Value;
use rust_i18n::t;

use crate::{Backend, design};
use moon_core::db::{self, ReportFilter, ReportTable, SideFilter};

/// Data cap для отчёта: в таблицу грузим только топ-N под текущий фильтр/сортировку,
/// а «Итого за период» считается ОТДЕЛЬНЫМ SQL-агрегатом по ВСЕМУ фильтру
/// (`query_totals`) — итоги точны при любом N. Каждая запись writer'а перечитывает
/// выборку (generation), поэтому N маленький — иначе фоновые апдейты легаси-строк
/// каждые ~30с гоняли бы стотысячные выборки (CPU впустую).
const MAX_REPORT_ROWS: usize = 100;

/// Пресеты периода, как в отчёте Moonbot. Границы — сутки UTC (даты в БД и в
/// таблице отображаются в UTC — фильтр согласован с тем, что видно в колонках).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    All,
    Today,
    Yesterday,
    Week,
    Month,
    Year,
}

impl Period {
    pub(super) const ALL: [Self; 6] = [
        Self::All,
        Self::Today,
        Self::Yesterday,
        Self::Week,
        Self::Month,
        Self::Year,
    ];

    pub(super) fn label(self) -> String {
        match self {
            Self::All => t!("report.filter.all"),
            Self::Today => t!("report.period.today"),
            Self::Yesterday => t!("report.period.yesterday"),
            Self::Week => t!("report.period.week"),
            Self::Month => t!("report.period.month"),
            Self::Year => t!("report.period.year"),
        }
        .to_string()
    }

    /// (from, to) unix-секунды; None — граница не ограничена.
    fn range(self) -> (Option<i64>, Option<i64>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let day = now - now.rem_euclid(86_400);
        match self {
            Self::All => (None, None),
            Self::Today => (Some(day), None),
            Self::Yesterday => (Some(day - 86_400), Some(day - 1)),
            Self::Week => (Some(day - 6 * 86_400), None),
            Self::Month => (Some(day - 29 * 86_400), None),
            Self::Year => (Some(day - 364 * 86_400), None),
        }
    }
}

/// Колонки, видимые по умолчанию (имена = колонки БД).
const DEFAULT_VISIBLE: &[&str] = &[
    "buydate",
    "closedate",
    "core_name",
    "coin",
    "isshort",
    "quantity",
    "buyprice",
    "sellprice",
    "profitbtc",
    "lev",
    "sellreason",
    "comment",
];

/// Достроить ЧАСТИЧНО сохранённую карту ширин до полной (все текущие колонки,
/// недостающим — дефолт `width_for`). Частичная карта (старые сессии, переименованные
/// при миграции колонки) заставляла движок пере-масштабировать нетронутые колонки при
/// КАЖДОМ драге — соседи «плавали». Полная карта = драг двигает только свою колонку
/// (как в Ордерах после первого ресайза). Пустую не трогаем: полный автофил, снапшот
/// всех ширин движок сделает сам при первом драге.
fn complete_widths(widths: &mut std::collections::HashMap<String, f32>, cols: &[String]) {
    if widths.is_empty() {
        return;
    }
    for c in cols {
        widths
            .entry(c.clone())
            .or_insert_with(|| columns::width_for(c));
    }
}

struct ReportQueryResult {
    cores: Vec<(u64, String)>,
    table: ReportTable,
    totals: (f64, i64),
}

fn empty_report_query_result() -> ReportQueryResult {
    ReportQueryResult {
        cores: Vec::new(),
        table: ReportTable {
            cols: Vec::new(),
            rows: Vec::new(),
            core_uids: Vec::new(),
        },
        totals: (0.0, 0),
    }
}

/// `with_cores` — тянуть ли список ядер (GROUP BY по всей БД — дорого на сотнях МБ;
/// панель обновляет его не чаще раза в минуту, состав ядер меняется редко).
fn run_report_query(
    filter: ReportFilter,
    sort_key: String,
    sort_desc: bool,
    with_cores: bool,
) -> ReportQueryResult {
    let started = std::time::Instant::now();
    let Some(conn) = db::open_reader() else {
        return empty_report_query_result();
    };
    let out = ReportQueryResult {
        cores: if with_cores {
            db::distinct_cores(&conn)
        } else {
            Vec::new()
        },
        table: db::query_reports(&conn, &filter, &sort_key, sort_desc, MAX_REPORT_ROWS),
        totals: db::query_totals(&conn, &filter),
    };
    // Замер вместо гаданий: медленный requery виден в логе (частоту задаёт троттл панели).
    let ms = started.elapsed().as_millis();
    if ms > 250 {
        log::warn!(
            "отчёты: медленный query {ms}ms (rows={} cores={} filter: dates={:?}/{:?})",
            out.table.rows.len(),
            with_cores,
            filter.date_from,
            filter.date_to,
        );
    } else {
        log::debug!("отчёты: query {ms}ms (rows={})", out.table.rows.len());
    }
    out
}

pub struct ReportPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    generation: Option<Arc<AtomicU64>>,
    last_gen: u64,

    conn: Option<Connection>,
    pub(super) cores: Vec<(u64, String)>,
    pub(super) table: Rc<ReportTable>,
    totals: (f64, i64),

    sort_key: String,
    sort_desc: bool,

    /// Выбранные ядра (мультивыбор по uid, как выбор колонок); пусто = все.
    pub(super) sel_cores: HashSet<u64>,
    coin: Entity<MoonInputState>,
    from: Entity<MoonInputState>,
    to: Entity<MoonInputState>,
    pub(super) side: SideFilter,
    /// Пресет периода (дефолт «Сегодня», как MB). Ручные даты С:/По: действуют
    /// только при пресете «Все»; правка дат сбрасывает пресет на «Все».
    pub(super) period: Period,
    /// Галка «Эмулятор»: выкл (дефолт) — только реальные, вкл — только эмуляторные.
    pub(super) emu: bool,
    needs_query: bool,
    query_inflight: bool,
    query_seq: u64,
    /// Старт последнего запроса — троттл generation-requery (писатель бампает
    /// generation на КАЖДУЮ запись, легаси-апдейты открытых позиций идут каждые
    /// ~15с с ядра — без троттла БД в сотни МБ сканировалась бы с той же частотой).
    last_query_start: Option<std::time::Instant>,
    /// Отложенный generation-requery уже взведён (таймер досыпает остаток троттла).
    throttle_armed: bool,
    /// Когда последний раз тянули список ядер (дорогой GROUP BY — не чаще 1/мин).
    last_cores_at: Option<std::time::Instant>,

    /// Видимые колонки — по ИМЕНАМ (а не индексам), чтобы авто-добавленные поля
    /// ядра не сбивали соответствие. Новая колонка по умолчанию скрыта.
    pub(super) visible: HashSet<String>,
    table_state: Entity<MoonDataTableState>,
    /// Id хранилища ширин колонок с контекстом (`report-table:dock`/`:win`) — свои ширины на режим.
    widths_id: String,
    /// Панель открыта в откреплённом окне (`mark_table_detached`): в докнутой вкладке
    /// показываем компактный набор фильтров (без ручных дат С:/По: — там есть пресеты).
    detached: bool,
    /// Снимок ширин с прошлого observe-тика — детект «какая колонка тянется» для
    /// бюджет-клампа [`Self::clamp_table_widths`].
    last_widths: std::collections::HashMap<String, f32>,
    // (table_state отдаётся наружу через `table_state()` — кнопка «⤢» откреп-окна.)
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl ReportPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let generation = backend
            .read(cx)
            .reports
            .as_ref()
            .map(|h| h.generation.clone());
        let conn = db::open_reader();
        let cores = conn.as_ref().map(db::distinct_cores).unwrap_or_default();
        let last_gen = generation
            .as_ref()
            .map(|g| g.load(Ordering::Relaxed))
            .unwrap_or(0);
        // Видимость колонок: восстанавливаем сохранённый набор имён (app_meta), иначе дефолт.
        let visible: HashSet<String> = conn
            .as_ref()
            .and_then(db::load_visible)
            .map(|saved| saved.into_iter().collect())
            .unwrap_or_else(|| DEFAULT_VISIBLE.iter().map(|c| c.to_string()).collect());
        // Стартовый список колонок — сразу из БД, чтобы первая отрисовка/меню были полными.
        let init_cols = conn.as_ref().map(db::display_columns).unwrap_or_default();
        let (sort_key, sort_desc) = conn
            .as_ref()
            .and_then(db::load_sort)
            .unwrap_or_else(|| ("buydate".to_string(), true));
        let widths_id = crate::table_persist::ctx_id("report-table", false);
        let mut saved_widths = crate::table_persist::saved(backend.read(cx), &widths_id);
        complete_widths(&mut saved_widths, &init_cols);
        let table_state = cx.new(|_| MoonDataTableState::new());
        table_state.update(cx, |state, _| {
            state.set_sort(sort_key.clone(), !sort_desc);
            state.column_widths = saved_widths;
        });
        // Ресайз колонки мутирует state → бюджет-кламп (сумма ширин ≤ вьюпорт, см.
        // clamp_table_widths) и сохранение ширин (универсальный сейвер).
        cx.observe(&table_state, |this, state, cx| {
            this.clamp_table_widths(&state, cx);
            crate::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let coin = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.coin_ph").to_string())
        });
        let from = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        let to = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("report.filter.date_ph").to_string())
        });
        cx.subscribe(&coin, |t, _e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                t.request_requery(cx);
            }
        })
        .detach();
        // Ручной ввод дат переводит период на «Все» (иначе пресет молча перекрыл бы даты).
        for st in [&from, &to] {
            cx.subscribe(st, |t, e, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    if !e.read(cx).value().trim().is_empty() {
                        t.period = Period::All;
                    }
                    t.request_requery(cx);
                }
            })
            .detach();
        }
        // Перерисовка — ТОЛЬКО когда writer записал новый отчёт (сменился generation);
        // иначе таблицу не перестраиваем каждые 100мс. Правки фильтров нотифаят сами
        // (без троттла — реакция на клик мгновенная); generation — через троттл.
        cx.observe(&backend, |this, _b, cx| {
            if let Some(g) = &this.generation {
                let v = g.load(Ordering::Relaxed);
                if v != this.last_gen {
                    this.last_gen = v;
                    this.requery_on_generation(cx);
                }
            }
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            generation,
            last_gen,
            conn,
            cores,
            table: Rc::new(ReportTable {
                cols: init_cols,
                rows: Vec::new(),
                core_uids: Vec::new(),
            }),
            totals: (0.0, 0),
            sort_key,
            sort_desc,
            sel_cores: HashSet::new(),
            coin,
            from,
            to,
            side: SideFilter::All,
            period: Period::Today,
            emu: false,
            needs_query: true,
            query_inflight: false,
            query_seq: 0,
            last_query_start: None,
            throttle_armed: false,
            last_cores_at: None,
            visible,
            table_state,
            widths_id,
            detached: false,
            last_widths: std::collections::HashMap::new(),
            dock: None,
            focus: cx.focus_handle(),
        };
        // Per-контекст набор полей (единый дескриптор) перекрывает загруженный из app_meta набор,
        // если сохранён для этого режима; иначе app_meta-набор остаётся как миграционный сид.
        this.apply_ctx_columns(cx);
        this.schedule_requery(cx);
        this
    }

    /// Пометить панель как открытую в ОТКРЕПЛЁННОМ окне (из `detached::spawn`): контекст ширин
    /// → `:win`, пере-засев из набора этого режима. Своя раскладка ширин у вкладки и у окна.
    /// Стейт таблицы — для кнопки «⤢ авто» в заголовке откреп-окна (detached.rs).
    pub(crate) fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Бюджет-кламп ширин: сумма ширин ВИДИМЫХ колонок не должна превышать вьюпорт
    /// таблицы. Пока превышает, движок пропорционально ужимает ВСЕ колонки на каждом
    /// рендере (`downscale_columns_to_available` форка, без опт-аута) — драг одной
    /// колонки менял масштаб всех, «соседи плыли». Держим стор в бюджете: перелив от
    /// одиночного драга снимаем с тянутой колонки (она упирается в край), перелив
    /// засева/сжатия панели — пропорционально со всех видимых. Ширины сверх вьюпорта
    /// движок всё равно не отрисовал бы (нет h-скролла) — мы ничего не теряем.
    fn clamp_table_widths(&mut self, state: &Entity<MoonDataTableState>, cx: &mut App) {
        let (cur, viewport) = {
            let s = state.read(cx);
            (s.column_widths.clone(), s.viewport_width())
        };
        let prev = std::mem::replace(&mut self.last_widths, cur.clone());
        if cur.is_empty() || viewport <= 80.0 {
            return;
        }
        // Сумма как у движка: видимые колонки, сток из карты либо дефолт колонки.
        let visible: Vec<&String> = self
            .table
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .collect();
        let sum: f32 = visible
            .iter()
            .map(|c| {
                cur.get(c.as_str())
                    .copied()
                    .unwrap_or_else(|| columns::width_for(c))
            })
            .sum();
        let overflow = sum - viewport;
        if overflow <= 0.5 {
            return;
        }
        // Одна изменившаяся колонка при том же составе карты = живой драг.
        let changed: Vec<String> = cur
            .iter()
            .filter(|(k, v)| prev.get(*k).is_none_or(|p| (**v - p).abs() > 0.5))
            .map(|(k, _)| k.clone())
            .collect();
        let single_drag = changed.len() == 1 && prev.len() == cur.len();
        state.update(cx, |s, c| {
            if single_drag {
                if let Some(w) = s.column_widths.get_mut(&changed[0]) {
                    *w = (*w - overflow).max(40.0);
                }
            } else {
                let scale = viewport / sum;
                for col in &visible {
                    if let Some(w) = s.column_widths.get_mut(col.as_str()) {
                        *w = (*w * scale).max(40.0);
                    }
                }
            }
            c.notify();
        });
        self.last_widths = state.read(cx).column_widths.clone();
    }

    pub(crate) fn mark_table_detached(&mut self, cx: &mut Context<Self>) {
        self.detached = true;
        self.widths_id = crate::table_persist::ctx_id("report-table", true);
        let mut saved = crate::table_persist::saved(self.backend.read(cx), &self.widths_id);
        complete_widths(&mut saved, &self.table.cols);
        self.table_state.update(cx, |s, c| {
            s.column_widths = saved;
            c.notify();
        });
        self.apply_ctx_columns(cx);
        cx.notify();
    }

    /// Применить сохранённый per-контекст набор видимых колонок (`table_visible_columns` по
    /// `widths_id`), если он есть. Единый дескриптор полей: набор различается на режим (`:dock`
    /// vs `:win`). Пустой набор → оставляем текущий (иначе показали бы пустую таблицу).
    fn apply_ctx_columns(&mut self, cx: &App) {
        if let Some(keys) = crate::table_persist::visible(self.backend.read(cx), &self.widths_id) {
            let set: HashSet<String> = keys.into_iter().collect();
            if !set.is_empty() {
                self.visible = set;
            }
        }
    }

    /// Сохранить текущий набор видимых колонок в per-контекст хранилище (`widths_id`) в порядке
    /// колонок таблицы. Зовётся из [`Self::toggle_column`].
    fn save_ctx_columns(&self, cx: &mut App) {
        let keys: Vec<String> = self
            .table
            .cols
            .iter()
            .filter(|c| self.visible.contains(c.as_str()))
            .map(|c| c.to_string())
            .collect();
        if !keys.is_empty() {
            crate::table_persist::set_visible(&self.backend, &self.widths_id, keys, cx);
        }
    }

    fn filter(&self, cx: &App) -> ReportFilter {
        // Пресет периода перекрывает ручные даты; «Все» отдаёт даты полям С:/По:.
        let (pfrom, pto) = self.period.range();
        let date_from = pfrom.or_else(|| db::parse_ymd(&self.from.read(cx).value()));
        let date_to =
            pto.or_else(|| db::parse_ymd(&self.to.read(cx).value()).map(|d| d + 86_399));
        ReportFilter {
            core_uids: self.sel_cores.iter().copied().collect(),
            date_from,
            date_to,
            coin: self.coin.read(cx).value().to_string(),
            side: self.side,
            emulator: self.emu,
        }
    }

    fn request_requery(&mut self, cx: &mut Context<Self>) {
        self.needs_query = true;
        self.schedule_requery(cx);
        cx.notify();
    }

    /// Requery по записи writer'а: не чаще раза в 5с (трейлинг-таймер досыпает остаток —
    /// последняя запись не теряется). Пользовательские правки фильтров идут мимо троттла.
    fn requery_on_generation(&mut self, cx: &mut Context<Self>) {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
        let since = self
            .last_query_start
            .map(|t| t.elapsed())
            .unwrap_or(MIN_INTERVAL);
        if since >= MIN_INTERVAL {
            self.request_requery(cx);
            return;
        }
        if self.throttle_armed {
            return;
        }
        self.throttle_armed = true;
        let wait = MIN_INTERVAL - since;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(wait).await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.throttle_armed = false;
                    this.request_requery(cx);
                });
            });
        })
        .detach();
    }

    fn schedule_requery(&mut self, cx: &mut Context<Self>) {
        if !self.needs_query || self.query_inflight {
            return;
        }
        self.needs_query = false;
        self.query_inflight = true;
        self.query_seq = self.query_seq.wrapping_add(1);
        self.last_query_start = Some(std::time::Instant::now());
        // Список ядер — не чаще раза в минуту (дорогой GROUP BY по всей БД).
        let with_cores = self
            .last_cores_at
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(60))
            .unwrap_or(true);
        if with_cores {
            self.last_cores_at = Some(std::time::Instant::now());
        }

        let request_id = self.query_seq;
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;

        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move { run_report_query(filter, sort_key, sort_desc, with_cores) })
                .await;

            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.query_seq != request_id {
                        return;
                    }
                    this.query_inflight = false;
                    if this.needs_query {
                        this.schedule_requery(cx);
                        return;
                    }

                    let cols_changed = this.table.cols != result.table.cols;
                    if !result.cores.is_empty() {
                        this.cores = result.cores;
                    }
                    this.table = Rc::new(result.table);
                    this.totals = result.totals;
                    // Состав колонок вырос (пришла схема ядра / появилось новое поле) —
                    // достраиваем карту ширин, чтобы новые колонки не «плавали» при
                    // ресайзе соседей. Только на смену состава: одиночный сброс колонки
                    // дабл-кликом (движок убирает её ключ) не перетирается.
                    if cols_changed {
                        let cols = this.table.cols.clone();
                        this.table_state.update(cx, |s, c| {
                            if !s.column_widths.is_empty() {
                                complete_widths(&mut s.column_widths, &cols);
                                c.notify();
                            }
                        });
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Тогл ядра в мультивыборе; `None` — «Все»-тумблер: ставит галки на все ядра,
    /// повторный клик по полному набору — снимает все (пусто = фильтра нет).
    pub(super) fn toggle_core(&mut self, uid: Option<u64>, cx: &mut Context<Self>) {
        match uid {
            None => {
                let all: HashSet<u64> = self.cores.iter().map(|(u, _)| *u).collect();
                if !all.is_empty() && self.sel_cores.len() == all.len() {
                    self.sel_cores.clear();
                } else {
                    self.sel_cores = all;
                }
            }
            Some(uid) => {
                if !self.sel_cores.remove(&uid) {
                    self.sel_cores.insert(uid);
                }
            }
        }
        self.request_requery(cx);
    }

    /// «Все»-тумблер колонок: включает все колонки; повторный клик по полному набору
    /// оставляет только первую (все скрыть нельзя — таблица опустеет).
    pub(super) fn toggle_all_columns(&mut self, cx: &mut Context<Self>) {
        let all_on = !self.table.cols.is_empty()
            && self.table.cols.iter().all(|c| self.visible.contains(c.as_str()));
        if all_on {
            self.visible = self.table.cols.first().cloned().into_iter().collect();
        } else {
            self.visible = self.table.cols.iter().cloned().collect();
        }
        if let Some(conn) = &self.conn {
            let cols: Vec<&str> = self
                .table
                .cols
                .iter()
                .filter(|c| self.visible.contains(c.as_str()))
                .map(|c| c.as_str())
                .collect();
            db::save_visible(conn, &cols);
        }
        self.save_ctx_columns(cx);
        cx.notify();
    }
    pub(super) fn set_side(&mut self, s: SideFilter, cx: &mut Context<Self>) {
        if self.side != s {
            self.side = s;
            self.request_requery(cx);
        }
    }
    pub(super) fn set_period(&mut self, p: Period, cx: &mut Context<Self>) {
        if self.period != p {
            self.period = p;
            self.request_requery(cx);
        }
    }
    pub(super) fn set_emu(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.emu != on {
            self.emu = on;
            self.request_requery(cx);
        }
    }
    /// Переключить видимость колонки по ИМЕНИ и СОХРАНИТЬ набор (app_meta) — переживает рестарт.
    pub(super) fn toggle_column(&mut self, name: String, cx: &mut Context<Self>) {
        if self.visible.contains(name.as_str()) {
            self.visible.remove(&name);
        } else {
            self.visible.insert(name);
        }
        if let Some(conn) = &self.conn {
            // Сохраняем в порядке колонок таблицы (стабильно для UI).
            let cols: Vec<&str> = self
                .table
                .cols
                .iter()
                .filter(|c| self.visible.contains(c.as_str()))
                .map(|c| c.as_str())
                .collect();
            db::save_visible(conn, &cols);
        }
        // Единый per-контекст дескриптор полей (свой набор на :dock/:win).
        self.save_ctx_columns(cx);
        cx.notify();
    }
    /// Экспорт отчёта в файл (пункты меню «Сохранить»): диалог пути → фоновый запрос
    /// БД по ТЕКУЩЕМУ фильтру (период/даты/ядра/монета/сторона/эмулятор, сортировка
    /// как в таблице) → запись CSV/XLSX. `all_cols` — все колонки БД, иначе видимые.
    fn export_report(&mut self, fmt: export::Format, all_cols: bool, cx: &mut Context<Self>) {
        let cols: Vec<String> = if all_cols {
            self.table.cols.clone()
        } else {
            self.table
                .cols
                .iter()
                .filter(|c| self.visible.contains(c.as_str()))
                .cloned()
                .collect()
        };
        if cols.is_empty() {
            log::warn!("отчёт: экспорт без колонок (таблица пуста?)");
            return;
        }
        let filter = self.filter(cx);
        let sort_key = self.sort_key.clone();
        let sort_desc = self.sort_desc;
        let suggested = export::suggested_name(&filter, fmt, all_cols);
        let rx = cx.prompt_for_new_path(&export::default_dir(), Some(&suggested));
        cx.spawn(async move |_this, cx| {
            // Диалог отменён/закрыт — тихо выходим.
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let executor = cx.update(|cx| cx.background_executor().clone());
            let result = executor
                .spawn(async move {
                    export::run(&path, fmt, &cols, &filter, &sort_key, sort_desc)
                        .map(|n| (n, path))
                })
                .await;
            match result {
                Ok((n, path)) => log::info!("отчёт: экспортировано {n} строк → {}", path.display()),
                Err(e) => log::warn!("отчёт: экспорт не удался: {e:#}"),
            }
        })
        .detach();
    }

    fn set_report_sort(&mut self, col: &str, sort_desc: bool, cx: &mut Context<Self>) {
        if self.sort_key == col && self.sort_desc == sort_desc {
            return;
        }
        self.sort_key = col.to_string();
        self.sort_desc = sort_desc;
        self.table_state.update(cx, |state, _| {
            state.set_sort(col.to_string(), !sort_desc);
        });
        if let Some(conn) = &self.conn {
            db::save_sort(conn, &self.sort_key, self.sort_desc);
        }
        self.request_requery(cx);
    }
}

impl EventEmitter<PanelEvent> for ReportPanel {}
impl Focusable for ReportPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for ReportPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Report"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("dock.tab.report").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Report", &self.group)
    }
    fn on_added_to(
        &mut self,
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.dock = Some(dock_area);
    }
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Vec<AnyElement>> {
        Some(vec![
            crate::table_persist::reset_button("report-reset-widths", &self.table_state),
            crate::panels::detach_button(
                "Report",
                self.group.clone(),
                self.backend.clone(),
                self.dock.clone(),
            ),
        ])
    }
}

impl Render for ReportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let border = rgb(p.border);

        // ── Фильтры ──
        let filters = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.filter.core").to_string()),
            )
            .child(self.core_combo(cx))
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.filter.coin").to_string()),
            )
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("rep-coin")
                        .state(&self.coin)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.filter.side").to_string()),
            )
            .child(self.side_combo(cx))
            .child(self.period_combo(cx))
            .child({
                let view = cx.entity();
                MoonCheckbox::new("rep-emu")
                    .checked(self.emu)
                    .size(MoonCheckboxSize::Compact)
                    .label(t!("report.filter.emu").to_string())
                    .on_change(move |on: &bool, _w, app| {
                        let on = *on;
                        view.update(app, |t, c| t.set_emu(on, c));
                    })
            })
            // Ручные даты С:/По: — только в откреплённом окне; в докнутой вкладке
            // хватает пресетов периода (компактная строка: ядро/монета/сторона/
            // период/эмулятор/колонки).
            .when(self.detached, |f| {
                f.child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.from").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-from").state(&self.from).small()),
                )
                .child(
                    div()
                        .text_size(design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("report.filter.to").to_string()),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(MoonInput::new("rep-to").state(&self.to).small()),
                )
            })
            .child(self.export_menu(cx))
            .child(self.columns_menu(cx));

        // ── Таблица ──
        let vis: Vec<usize> = self
            .table
            .cols
            .iter()
            .enumerate()
            .filter(|(_, c)| self.visible.contains(c.as_str()))
            .map(|(i, _)| i)
            .collect();
        let table_el: AnyElement = if vis.is_empty() {
            div()
                .p_3()
                .text_color(rgb(p.text_soft))
                .child(t!("report.all_cols_hidden").to_string())
                .into_any_element()
        } else {
            let table = self.table.clone();
            let visible = Rc::new(vis.clone());
            let row_count = table.rows.len();
            let view = cx.entity();
            let backend = self.backend.clone();
            let table_state = self.table_state.clone();
            let cols = columns::report_columns(&self.table, &vis);
            // Общий хост докнутых таблиц (как Orders/Assets): overflow_hidden не даёт
            // раздвинутым колонкам распирать layout панели, «пусто» — оверлеем.
            crate::panels::common::data_table_host(
                "rep-table-host",
                row_count == 0,
                t!("report.empty").to_string(),
                p,
                cx,
                MoonDataTable::new("report-table", row_count, move |ri, _window, _app| {
                    columns::report_data_row(ri, &table, &visible, &backend, p)
                })
                .state(&table_state)
                .columns(cols)
                .header_height(design::TABLE_HEAD_H)
                .row_height(design::TABLE_ROW_H)
                .on_sort(move |key, ascending, _window, app| {
                    let key = key.to_string();
                    view.update(app, |t, cx| t.set_report_sort(&key, !ascending, cx));
                }),
            )
            .into_any_element()
        };

        // ── ИТОГО ──
        let (sum, count) = self.totals;
        let sum_col = if sum > 0.0 {
            p.green
        } else if sum < 0.0 {
            p.red
        } else {
            p.text_soft
        };
        let totals = h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.totals").to_string()),
            )
            .child(
                // Профит в долларах: 2 знака + «$» (без «BTC» — сумма в котировке пар,
                // обычно usdt; 6 знаков «тысячных» были лишними).
                div()
                    .font_bold()
                    .text_color(rgb(sum_col))
                    .child(format!("{sum:+.2} $")),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.orders_count", count = count).to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("report.shown_top", n = self.table.rows.len()).to_string()),
            );

        v_flex()
            .id("report-panel")
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(p.table_body))
            .child(filters)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(table_el)
            .child(div().w_full().h(px(1.0)).bg(border))
            .child(totals)
    }
}
