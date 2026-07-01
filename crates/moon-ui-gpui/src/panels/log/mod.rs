//! Панель «Лог» — порт egui `src/dock/log_panel.rs`. Просмотр лога с выбором
//! источника, файла, поиском и фильтром «только ошибки».
//!
//! Источники: «Лог группы» (агрегат живых логов ядер группы), «Локальный» (лог
//! приложения, `applog`-кольцо) и каждое ядро (его серверный лог, кольцо в `CoreData.log`).
//! Для одного ядра/локального можно смотреть Live (текущий) ИЛИ файл с диска
//! (`logs/<дата>_<источник>.log`); агрегат — только Live. Список виртуализирован
//! через `MoonVirtualList`; при появлении новых строк прокрутка держится у хвоста.
//!
//! По функционалу разнесено: состояние/сбор строк/жизненный цикл — здесь, поля-списки
//! источника/файла — [`controls`], сигнатура/агрегат/рендер строки — [`render`].

mod controls;
mod render;

use gpui::*;
use moon_ui::{
    DockArea, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonDropdown,
    MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem, MoonMenuSize, MoonPalette,
    MoonScrollbarVisibility, MoonVirtualList, MoonVirtualListScrollHandle, Panel, PanelEvent,
    PanelState, StyledExt, h_flex, v_flex,
};

use rust_i18n::t;

use crate::Backend;
use moon_core::applog::{self, LogLine};
use moon_core::session::{CoreId, CoreStore};

/// Сколько последних строк держим в поле зрения.
const VIEW_LIMIT: usize = 5000;
/// Сколько строк берём с каждого ядра при сборке агрегата.
const AGG_PER_CORE: usize = 2000;
/// Потолок буфера в режиме паузы (листаем): пока не в Live, старые строки не удаляем, а
/// дописываем новые сверх VIEW_LIMIT — чтобы позиция скролла не «съезжала». До этого
/// потолка. При возврате в Live обрезаем обратно к VIEW_LIMIT.
const PAUSED_CAP: usize = 20_000;

/// Источник лога.
#[derive(Clone, PartialEq)]
pub(super) enum LogSource {
    Aggregate,
    Local,
    Core(CoreId),
}

/// Что показываем: живой лог из памяти или файл с диска.
#[derive(Clone, PartialEq)]
pub(super) enum LogFile {
    Live,
    Named(String),
}

/// Один пункт селектора источника.
pub(super) struct LogSourceItem {
    pub(super) source: LogSource,
    pub(super) display: String,
    pub(super) file_label: String,
}

pub struct LogPanel {
    pub(super) backend: Entity<Backend>,
    pub(super) group: String,
    pub(super) source: LogSource,
    pub(super) file: LogFile,
    errors_only: bool,
    /// Фильтр по монете (клик по тикеру в строке). None — без фильтра.
    coin_filter: Option<String>,
    query: Entity<MoonInputState>,
    /// Кэш загруженного файла — чтобы не читать диск каждый кадр.
    loaded_name: Option<String>,
    loaded_lines: Vec<LogLine>,
    /// Кэш списка файлов выбранного источника. `render` не должен ходить в FS ради dropdown.
    available_files_label: Option<String>,
    available_files: Vec<String>,
    /// Нефильтрованные строки текущего источника/file. Обновляются вне render.
    raw_lines: Vec<LogLine>,
    /// Отфильтрованные строки текущего кадра (читает рендер списка по индексу).
    /// Классификация/монета посчитаны заранее в `apply_filter`, не на кадре.
    lines: Vec<render::LineView>,
    total: usize,
    /// «Live»: намерение пользователя держаться у хвоста. Ручной выключатель —
    /// отжатая вручную кнопка не вернётся к Live сама (только ручным нажатием).
    live: bool,
    /// Авто-пауза Live: пользователь скроллит. Ставится по колесу, снимается таймером
    /// через 5 c после последнего скролла. Пока true — к низу не прыгаем.
    scroll_pause: bool,
    /// Поколение скролла — гейт для отложенного авто-возврата (новый скролл отменяет
    /// прошлый таймер: 5 c считаются от ПОСЛЕДНЕГО скролла).
    scroll_gen: u64,
    scroll: MoonVirtualListScrollHandle,
    /// Сигнатура лога прошлого кадра — чтобы НЕ пересобирать лог каждые 100мс
    /// (gather клонирует до 5000 строк; на холостом ходу это лишняя нагрузка).
    last_sig: u64,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl LogPanel {
    pub fn new(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query =
            cx.new(|cx| MoonInputState::new(window, cx).placeholder(t!("log.search").to_string()));
        cx.subscribe(&query, |t, _e, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                t.apply_filter(cx);
                cx.notify();
            }
        })
        .detach();
        // Перерисовка — ТОЛЬКО когда реально появились новые строки лога.
        cx.observe(&backend, |this, backend, cx| {
            let sig = render::log_sig(backend.read(cx), &this.group);
            if sig != this.last_sig {
                this.last_sig = sig;
                this.reload_rows(backend.read(cx), cx);
                cx.notify();
            }
        })
        .detach();
        let mut this = Self {
            backend,
            group,
            source: LogSource::Aggregate,
            file: LogFile::Live,
            errors_only: true,
            coin_filter: None,
            query,
            loaded_name: None,
            loaded_lines: Vec::new(),
            available_files_label: None,
            available_files: Vec::new(),
            raw_lines: Vec::new(),
            lines: Vec::new(),
            total: 0,
            live: true,
            scroll_pause: false,
            scroll_gen: 0,
            scroll: MoonVirtualListScrollHandle::new(),
            last_sig: 0,
            dock: None,
            focus: cx.focus_handle(),
        };
        let backend_for_initial_load = this.backend.clone();
        this.reload_rows(backend_for_initial_load.read(cx), cx);
        this
    }

    /// Список источников в области видимости (порт `App::build_log_sources`).
    /// Группа непуста → только её ядра (агрегат = «Лог группы»); пусто → все (детач).
    fn sources(&self, b: &Backend) -> Vec<LogSourceItem> {
        let scoped = !self.group.is_empty();
        let mut v = vec![
            LogSourceItem {
                source: LogSource::Aggregate,
                display: if scoped {
                    t!("log.source.group").to_string()
                } else {
                    t!("log.source.all").to_string()
                },
                file_label: String::new(),
            },
            LogSourceItem {
                source: LogSource::Local,
                display: t!("log.source.local").to_string(),
                file_label: "app".into(),
            },
        ];
        for s in &b.config.servers {
            if scoped && s.group != self.group {
                continue;
            }
            v.push(LogSourceItem {
                source: LogSource::Core(s.id),
                display: s.name.clone(),
                file_label: applog::sanitize_label(&s.name),
            });
        }
        v
    }

    pub(super) fn file_label(&self, sources: &[LogSourceItem]) -> String {
        sources
            .iter()
            .find(|s| s.source == self.source)
            .map(|s| s.file_label.clone())
            .unwrap_or_else(|| "app".into())
    }

    fn refresh_available_files(&mut self, label: &str) {
        if self.available_files_label.as_deref() == Some(label) {
            return;
        }
        self.available_files = applog::list_files(label);
        self.available_files_label = Some(label.to_string());
    }

    /// Строки для текущего выбора (Live — из памяти/агрегат слиянием; Named — из файла).
    fn gather(&mut self, store: &CoreStore, sources: &[LogSourceItem]) -> Vec<LogLine> {
        match &self.file {
            LogFile::Live => {
                self.loaded_name = None;
                match &self.source {
                    LogSource::Local => applog::snapshot(VIEW_LIMIT),
                    LogSource::Core(id) => store
                        .core(*id)
                        .map(|c| c.log_snapshot(VIEW_LIMIT))
                        .unwrap_or_default(),
                    LogSource::Aggregate => render::aggregate(store, sources),
                }
            }
            LogFile::Named(name) => {
                if self.loaded_name.as_deref() != Some(name.as_str()) {
                    self.loaded_lines = applog::read_file(name, VIEW_LIMIT);
                    self.loaded_name = Some(name.clone());
                }
                self.loaded_lines.clone()
            }
        }
    }

    fn apply_filter(&mut self, cx: &App) {
        let query = self.query.read(cx).value().trim().to_lowercase();
        let errors_only = self.errors_only;
        let coin = self.coin_filter.as_deref().map(str::to_lowercase);
        self.total = self.raw_lines.len();
        // Базы монет со всего буфера — чтобы подсветить голые тикеры (`SPK`), встреченные
        // где-то в рыночной форме (`USDT-SPK`). Один проход до сборки строк.
        let known = render::collect_coin_bases(&self.raw_lines);
        // Классификацию/монету считаем ОДИН раз здесь (не на кадре): парсинг текста
        // дорог, а рендер строки вызывается каждый кадр для каждой видимой строки.
        self.lines = self
            .raw_lines
            .iter()
            .filter(|l| {
                query.is_empty()
                    || l.msg.to_lowercase().contains(&query)
                    || l.target.to_lowercase().contains(&query)
            })
            .filter(|l| {
                coin.as_ref()
                    .is_none_or(|c| l.msg.to_lowercase().contains(c))
            })
            .filter_map(|l| {
                let cl = render::classify(l);
                if errors_only && !render::is_error(cl.sev) {
                    return None;
                }
                Some(render::LineView::from_parts(l, cl, &known))
            })
            .collect();
        if self.following() && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    /// Тейлим ли сейчас (эффективный Live): включён и не на скролл-паузе.
    fn following(&self) -> bool {
        self.live && !self.scroll_pause
    }

    /// Вернуться в Live: снять паузу, включить, перечитать свежий хвост из кольца и
    /// прыгнуть вниз (reload_rows → apply_filter сам прыгает, т.к. following() снова true).
    fn resume_live(&mut self, cx: &mut Context<Self>) {
        self.scroll_pause = false;
        self.live = true;
        let backend = self.backend.clone();
        self.last_sig = render::log_sig(backend.read(cx), &self.group);
        self.reload_rows(backend.read(cx), cx);
    }

    /// Пользователь крутанул колесо над списком. В режиме Live это отжимает кнопку
    /// (пауза тейлинга) и заводит таймер авто-возврата на 5 c после ПОСЛЕДНЕГО скролла.
    /// Если Live отжат вручную — скролл ничего не меняет.
    fn on_user_scroll(&mut self, cx: &mut Context<Self>) {
        if !self.live {
            return;
        }
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
        let want_gen = self.scroll_gen;
        if !self.scroll_pause {
            self.scroll_pause = true;
            cx.notify(); // кнопка визуально отжимается
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(std::time::Duration::from_secs(5)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |t, cx| {
                    // Таймер ещё актуален (не было нового скролла) и Live не отжали вручную.
                    if t.scroll_gen == want_gen && t.live && t.scroll_pause {
                        t.resume_live(cx);
                        cx.notify();
                    }
                })
                .ok();
            });
        })
        .detach();
    }

    /// Установить/снять фильтр по монете (клик по тикеру в строке).
    pub(super) fn set_coin_filter(&mut self, coin: Option<String>, cx: &mut Context<Self>) {
        if self.coin_filter != coin {
            self.coin_filter = coin;
            self.apply_filter(cx);
            cx.notify();
        }
    }

    /// ПКМ по монете: открыть её график на Main (через `Backend.open_request`, как Ордера/
    /// Детекты). Ядро строки: источник `Core` → это ядро; `Aggregate` → сервер из `target`;
    /// `Local` → первое ядро группы, где такая монета есть. Базу (`SPK`) резолвим в рыночное
    /// имя через market-поиск ядра (истина), не угадываем суффикс.
    pub(super) fn open_coin_chart(&mut self, base: String, target: String, cx: &mut Context<Self>) {
        let resolved = {
            let b = self.backend.read(cx);
            let ms = b.session.market_source();
            let core = match &self.source {
                LogSource::Core(id) => Some(*id),
                LogSource::Aggregate => b
                    .config
                    .servers
                    .iter()
                    .find(|s| s.name == target)
                    .map(|s| s.id),
                LogSource::Local => None,
            };
            let scoped = !self.group.is_empty();
            let candidates: Vec<CoreId> = match core {
                Some(id) => vec![id],
                None => b
                    .config
                    .servers
                    .iter()
                    .filter(|s| !scoped || s.group == self.group)
                    .map(|s| s.id)
                    .collect(),
            };
            candidates.into_iter().find_map(|id| {
                ms.search_markets(id, &base, 1)
                    .into_iter()
                    .next()
                    .map(|market| (id, market))
            })
        };
        let Some((core, market)) = resolved else {
            return; // рынок для монеты не нашёлся на ядре — молча ничего не делаем
        };
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            // Открыть монету, но окно Main не поднимать (как в Ордерах/Детектах).
            b.open_request_activate = false;
            bcx.notify();
        });
    }

    fn reload_rows(&mut self, b: &Backend, cx: &App) {
        let sources = self.sources(b);
        let is_agg = matches!(self.source, LogSource::Aggregate);
        if !is_agg {
            let label = self.file_label(&sources);
            self.refresh_available_files(&label);
        }
        let fresh = self.gather(b.session.store(), &sources);
        if self.following() {
            // Live: свежий хвост (кольцо уже обрезано до VIEW_LIMIT).
            self.raw_lines = fresh;
        } else {
            // Пауза (листаем): дописываем только новые строки в конец, старые не трогаем —
            // позиция скролла не сдвигается. Лимит VIEW_LIMIT снят до PAUSED_CAP.
            self.merge_paused(fresh);
        }
        self.apply_filter(cx);
    }

    /// Слить свежий снимок в `raw_lines` в режиме паузы: найти в свежем нашу последнюю
    /// строку (по времени+тексту+источнику) и дописать всё, что после неё. Нет совпадения
    /// (паузу держали дольше, чем кольцо, > VIEW_LIMIT новых) → дописываем весь снимок
    /// (возможен разрыв — редкий край). Сверху обрезаем до PAUSED_CAP.
    fn merge_paused(&mut self, fresh: Vec<LogLine>) {
        match self.raw_lines.last() {
            None => self.raw_lines = fresh,
            Some(last) => {
                let boundary = fresh
                    .iter()
                    .rposition(|l| l.ts == last.ts && l.msg == last.msg && l.target == last.target);
                match boundary {
                    Some(pos) => self.raw_lines.extend(fresh.into_iter().skip(pos + 1)),
                    None => self.raw_lines.extend(fresh),
                }
            }
        }
        if self.raw_lines.len() > PAUSED_CAP {
            let drop = self.raw_lines.len() - PAUSED_CAP;
            self.raw_lines.drain(0..drop);
        }
    }

    /// Явный выбор источника/файла → всегда к Live (иначе merge_paused слил бы чужой лог).
    fn reset_to_live(&mut self) {
        self.live = true;
        self.scroll_pause = false;
        self.scroll_gen = self.scroll_gen.wrapping_add(1);
    }

    pub(super) fn set_source(&mut self, s: LogSource, cx: &mut Context<Self>) {
        if self.source != s {
            self.source = s;
            // Смена источника → к Live, сброс кэша файла.
            self.file = LogFile::Live;
            self.loaded_name = None;
            self.available_files_label = None;
            self.available_files.clear();
            self.reset_to_live();
            let backend = self.backend.clone();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
    pub(super) fn set_file(&mut self, f: LogFile, cx: &mut Context<Self>) {
        if self.file != f {
            self.file = f;
            self.reset_to_live();
            let backend = self.backend.clone();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
}

impl EventEmitter<PanelEvent> for LogPanel {}
impl Focusable for LogPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for LogPanel {
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn panel_name(&self) -> &'static str {
        "Log"
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(t!("dock.tab.log").to_string())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Log", &self.group)
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
        Some(vec![crate::panels::detach_button(
            "Log",
            self.group.clone(),
            self.backend.clone(),
            self.dock.clone(),
        )])
    }
}

impl Render for LogPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);

        let sources = self.sources(self.backend.read(cx));
        let is_agg = matches!(self.source, LogSource::Aggregate);
        let total = self.total;

        // ── Панель управления ──
        let mut controls = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2()
            .items_center()
            .px_2()
            .py_1();
        controls = controls.child(self.source_combo(&sources, cx));
        if !is_agg {
            controls = controls
                .child(
                    div()
                        .text_size(crate::design::t_body(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("log.file").to_string()),
                )
                .child(self.file_combo(&self.available_files, cx));
        }
        controls = controls
            .child(
                div().w(px(180.0)).child(
                    MoonInput::new("log-query")
                        .state(&self.query)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(
                MoonCheckbox::new("log-errors-only")
                    .label(t!("log.errors_only").to_string())
                    .checked(self.errors_only)
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        if t.errors_only != *ch {
                            t.errors_only = *ch;
                            t.apply_filter(cx);
                            cx.notify();
                        }
                    })),
            )
            .child(
                MoonCheckbox::new("log-live")
                    .label(t!("log.follow_tail").to_string())
                    .checked(self.following())
                    .size(MoonCheckboxSize::Compact)
                    .on_change(cx.listener(|t, ch: &bool, _, cx| {
                        // Ручное нажатие отменяет отложенный авто-возврат.
                        t.scroll_gen = t.scroll_gen.wrapping_add(1);
                        if *ch {
                            t.resume_live(cx); // вернуться к живому хвосту
                        } else {
                            // Отжали вручную — заморозить, сама к Live не вернётся.
                            t.live = false;
                            t.scroll_pause = false;
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("log.count", shown = self.lines.len(), total = total).to_string()),
            );
        // Чип активного фильтра монеты (клик снимает).
        if let Some(coin) = self.coin_filter.clone() {
            controls = controls.child(
                div()
                    .id("log-coin-chip")
                    .flex_none()
                    .cursor_pointer()
                    .px_1()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(p.blue))
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.blue))
                    .child(format!("{coin} ✕"))
                    .on_click(cx.listener(|t, _, _, cx| t.set_coin_filter(None, cx))),
            );
        }

        // ── Список (виртуализирован, к низу) ──
        let weak = cx.entity().downgrade();
        let body: AnyElement = if self.lines.is_empty() {
            let msg = if total == 0 {
                t!("dock.log.empty").to_string()
            } else {
                t!("log.empty_filtered").to_string()
            };
            div()
                .flex_1()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(p.text_soft))
                .child(msg)
                .into_any_element()
        } else {
            let scroll = self.scroll.clone();
            let query = self.query.read(cx).value().trim().to_lowercase();
            let list_el = MoonVirtualList::new(
                "log-virtual-rows",
                self.lines.len(),
                18.0,
                move |ix, _w, app| {
                    weak.upgrade()
                        .and_then(|e| {
                            e.read(app)
                                .lines
                                .get(ix)
                                .map(|line| render::log_row(line, &query, &weak, p, app))
                        })
                        .unwrap_or_else(|| div().into_any_element())
                },
            )
            .track_scroll(&scroll)
            .surface(false)
            .border(false)
            .radius(0.0)
            .scrollbar_visibility(MoonScrollbarVisibility::Hover);
            div()
                .flex_1()
                .w_full()
                .min_h_0()
                .child(list_el)
                // Скролл колесом над списком → пауза Live + таймер авто-возврата.
                .on_scroll_wheel(cx.listener(|t, _e: &ScrollWheelEvent, _w, cx| {
                    t.on_user_scroll(cx);
                }))
                .into_any_element()
        };

        v_flex()
            .id("log-panel")
            .size_full()
            .track_focus(&self.focus)
            .child(controls)
            .child(div().w_full().h(px(1.0)).bg(rgb(p.border)))
            .child(body)
    }
}
