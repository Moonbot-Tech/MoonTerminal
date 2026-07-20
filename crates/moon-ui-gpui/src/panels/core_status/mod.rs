//! Панель «Статус ядер»: таблица системных метрик по ядрам охвата (CPU/память + статус
//! подключения). Данные вытащены из строк серверного лога (`CoreData::sys`, парсер
//! `moon_core::session::sys_status`) — moonproto типизированных полей не шлёт.
//!
//! Устройство 1:1 с «Активами» (группа-scope): dock-вкладка окна группы, откреп в своё
//! окно (общий механизм `detached.rs`), персист ширин колонок через [`crate::table_persist`]
//! с контекстом `:dock`/`:win`. Данные/жизненный цикл — здесь, таблица — в [`table`].

mod table;

use std::collections::HashSet;
use std::rc::Rc;

use gpui::*;
use moon_ui::{
    DockArea, MoonDataTableState, MoonPalette, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

use crate::Backend;
use crate::design;
use crate::panels::RenderGate;
use moon_core::feed::ConnStatus;
use moon_core::session::{CoreId, CoreSysStatus};
use rust_i18n::t;

/// Строка таблицы: ядро + его статус подключения + последний снимок системных метрик.
#[derive(Clone)]
pub(super) struct CoreStatusRow {
    pub(super) name: String,
    pub(super) status: ConnStatus,
    pub(super) sys: CoreSysStatus,
}

/// Панель «Статус ядер» (dock-вкладка / откреп-окно; охват = ядра группы).
pub struct CoreStatusView {
    pub(super) backend: Entity<Backend>,
    /// Группа окна: охват = ядра этой группы (как у «Активов» group-scope).
    group: String,
    /// Мультивыбор ядер фильтра (пусто = все ядра группы), как в «Ордерах»/«Активах».
    pub(super) sel_cores: HashSet<CoreId>,
    /// Гейт перерисовки: сигнатура sys_rev/статусов ИЛИ 1 Гц-тик (для «обновлено N с назад»).
    gate: RenderGate,
    cache_sig: Option<u64>,
    cached_rows: Rc<Vec<CoreStatusRow>>,
    table_state: Entity<MoonDataTableState>,
    /// Id хранилища ширин с контекстом (`core-status-table:dock` / `:win`).
    widths_id: String,
    dock: Option<WeakEntity<DockArea>>,
    focus: FocusHandle,
}

impl CoreStatusView {
    fn new(
        backend: Entity<Backend>,
        group: String,
        detached: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Перерисовка по дренажу backend — при изменении метрик/статусов или раз в секунду
        // (чтобы «обновлено N с назад» шло вперёд даже без новых строк лога).
        cx.observe(&backend, |this, backend, cx| {
            let now = moon_chart::paint::now_unix_ms();
            let b = backend.read(cx);
            let sig = this.sys_sig(b);
            let changed = this.cache_sig != Some(sig);
            let due = this.gate.should_notify(sig, now);
            if changed || due {
                this.rebuild_cache(b);
                cx.notify();
            }
        })
        .detach();

        let widths_id = crate::table_persist::ctx_id("core-status-table", detached);
        let saved_widths = crate::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.column_widths = saved_widths;
            s
        });
        cx.observe(&table_state, |this, state, cx| {
            crate::table_persist::persist(&this.backend, &this.widths_id, &state, cx);
        })
        .detach();

        let mut this = Self {
            backend,
            group,
            sel_cores: HashSet::new(),
            gate: RenderGate::default(),
            cache_sig: None,
            cached_rows: Rc::new(Vec::new()),
            table_state,
            widths_id,
            dock: None,
            focus: cx.focus_handle(),
        };
        let b = this.backend.clone();
        this.rebuild_cache(b.read(cx));
        this
    }

    /// Реконструкция dock-вкладки (из `docks.json`) — контекст ширин `:dock`.
    pub fn restored_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, false, window, cx)
    }

    /// Контент откреплённого окна (рамку даёт `DetachedWindow`) — контекст ширин `:win`.
    pub fn detached_group(
        backend: Entity<Backend>,
        group: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(backend, group, true, window, cx)
    }

    /// Доступ к state таблицы для кнопки «⤢ авто» в заголовке откреп-окна.
    pub fn table_state(&self) -> Entity<MoonDataTableState> {
        self.table_state.clone()
    }

    /// Ядра охвата (id, имя): ядра группы панели.
    pub(super) fn scope_cores(&self, b: &Backend) -> Vec<(CoreId, String)> {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| (s.id, s.name.clone()))
            .collect()
    }

    /// Сигнатура: fold sys_rev + дискриминант статуса по всем ядрам охвата — гейт перерисовки.
    fn sys_sig(&self, b: &Backend) -> u64 {
        let store = b.session.store();
        self.scope_cores(b).iter().fold(0u64, |a, (id, _)| {
            let (sys_rev, st) = store
                .core(*id)
                .map(|c| (c.sys_rev, status_ord(&c.status)))
                .unwrap_or((0, 0));
            a.wrapping_mul(31)
                .wrapping_add(sys_rev)
                .wrapping_mul(31)
                .wrapping_add(st)
        })
    }

    fn collect(&self, b: &Backend) -> Vec<CoreStatusRow> {
        let store = b.session.store();
        let mut out = Vec::new();
        for (id, name) in self.scope_cores(b) {
            if !self.sel_cores.is_empty() && !self.sel_cores.contains(&id) {
                continue;
            }
            let (status, sys) = store
                .core(id)
                .map(|c| (c.status.clone(), c.sys.clone()))
                .unwrap_or((ConnStatus::Disconnected, CoreSysStatus::default()));
            out.push(CoreStatusRow { name, status, sys });
        }
        out
    }

    fn rebuild_cache(&mut self, b: &Backend) {
        self.cache_sig = Some(self.sys_sig(b));
        self.cached_rows = Rc::new(self.collect(b));
    }

    /// Тумблер выбранного ядра фильтра (мультивыбор, как в «Активах»).
    pub(super) fn toggle_core(&mut self, id: Option<CoreId>, cx: &mut Context<Self>) {
        let all: HashSet<CoreId> = self
            .scope_cores(self.backend.read(cx))
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        match id {
            None => {
                if !all.is_empty() && self.sel_cores.len() == all.len() {
                    self.sel_cores.clear();
                } else {
                    self.sel_cores = all;
                }
            }
            Some(id) => {
                if !self.sel_cores.remove(&id) {
                    self.sel_cores.insert(id);
                }
            }
        }
        let b = self.backend.clone();
        self.rebuild_cache(b.read(cx));
        cx.notify();
    }
}

/// Дискриминант статуса для сигнатуры (Stage/Failed сравниваем и по тексту — грубо, длиной).
fn status_ord(s: &ConnStatus) -> u64 {
    match s {
        ConnStatus::Connecting => 1,
        ConnStatus::Stage(t) => 100 + t.len() as u64,
        ConnStatus::Ready => 2,
        ConnStatus::Failed(e) => 1000 + e.len() as u64,
        ConnStatus::Disconnected => 3,
    }
}

impl EventEmitter<PanelEvent> for CoreStatusView {}
impl Focusable for CoreStatusView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for CoreStatusView {
    fn panel_name(&self) -> &'static str {
        "CoreStatus"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::panel_meta::tab_label(self.panel_name())
    }
    fn closable(&self, _cx: &App) -> bool {
        true
    }
    fn show_dock_header(&self, _cx: &App) -> bool {
        true
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("CoreStatus", &self.group)
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
        Some(vec![crate::table_persist::reset_button(
            "core-status-reset-widths",
            &self.table_state,
        )])
    }
}

impl Render for CoreStatusView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cores = self.scope_cores(self.backend.read(cx));
        let rows = self.cached_rows.clone();
        let p = MoonPalette::active(cx);
        let count = rows.len();
        let now = moon_chart::paint::now_unix_ms() as i64;

        let core_bar = self.core_bar(&cores, cx);
        let footer = self.footer(count, cx);
        let table = table::core_status_table("core-status-table", rows, now, &self.table_state, cx);

        v_flex()
            .id("core-status-panel")
            .size_full()
            .relative()
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .bg(rgb(p.table_body))
            .child(core_bar)
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(
                v_flex()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(table),
            )
            .child(div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border)))
            .child(footer)
    }
}

impl CoreStatusView {
    /// Верхняя полоса: поле-список выбора ядер (мультивыбор, как в «Активах»).
    fn core_bar(&self, cores: &[(CoreId, String)], cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let combo = crate::controls::core_combo(
            cx,
            "core-status-core",
            cores,
            &self.sel_cores,
            t!("core_status.all_cores").to_string(),
            |n| t!("core_status.cores_n", n = n).to_string(),
            170.0,
            move |id, app| {
                view.update(app, |t, c| t.toggle_core(id, c));
            },
        );
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(combo)
    }

    /// Нижний футер: счётчик ядер (единый визуальный образец футера, как в «Активах»/«Отчёте»).
    fn footer(&self, count: usize, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .items_center()
            .px_2()
            .py_1()
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_soft))
                    .child(t!("core_status.cores").to_string()),
            )
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!("{count}")),
            )
    }
}
