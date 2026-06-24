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
    lines: Vec<LogLine>,
    total: usize,
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
            query,
            loaded_name: None,
            loaded_lines: Vec::new(),
            available_files_label: None,
            available_files: Vec::new(),
            raw_lines: Vec::new(),
            lines: Vec::new(),
            total: 0,
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
        self.total = self.raw_lines.len();
        let previous_len = self.lines.len();
        self.lines = self
            .raw_lines
            .iter()
            .filter(|l| !errors_only || l.is_errorish())
            .filter(|l| query.is_empty() || l.msg.to_lowercase().contains(&query))
            .cloned()
            .collect();
        if previous_len != self.lines.len() && !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    fn reload_rows(&mut self, b: &Backend, cx: &App) {
        let sources = self.sources(b);
        let is_agg = matches!(self.source, LogSource::Aggregate);
        if !is_agg {
            let label = self.file_label(&sources);
            self.refresh_available_files(&label);
        }
        self.raw_lines = self.gather(b.session.store(), &sources);
        self.apply_filter(cx);
    }

    pub(super) fn set_source(&mut self, s: LogSource, cx: &mut Context<Self>) {
        if self.source != s {
            self.source = s;
            // Смена источника → к Live, сброс кэша файла.
            self.file = LogFile::Live;
            self.loaded_name = None;
            self.available_files_label = None;
            self.available_files.clear();
            let backend = self.backend.clone();
            self.reload_rows(backend.read(cx), cx);
            cx.notify();
        }
    }
    pub(super) fn set_file(&mut self, f: LogFile, cx: &mut Context<Self>) {
        if self.file != f {
            self.file = f;
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
                div()
                    .text_size(crate::design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(t!("log.count", shown = self.lines.len(), total = total).to_string()),
            );

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
                                .map(|line| render::log_row(line, p, app))
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
