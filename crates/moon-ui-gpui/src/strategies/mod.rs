//! Окно «Стратегии» (порт egui `src/strategies/*` + `window/strategies_window.rs`).
//! Отдельное ОС-окно, 4 панели (дерево → секции → параметры): дерево ядро→папка→
//! стратегия с поиском/фильтрами/чекбоксами (стейджинг) и «Применить» (старт/стоп),
//! секции схемы выбранной стратегии (затемнение неактивных), плашки параметров
//! (read-only, YES/NO, «…» для длинных значений). Зависимости полей/разделов — из
//! `assets/param_deps.toml` (hot-reload, [`rules`]). Читает живой `Backend` (store
//! по ядрам), «Применить» шлёт `session.apply_strategies` (синхр. галок + старт/стоп).

mod filter;
mod logic;
mod params;
mod rules;
mod tree;
mod tree_dialogs;
mod tree_dnd;
mod tree_menu;
mod tree_moon;
mod tree_ops;
mod tree_ui;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonTextArea, MoonTextAreaEvent, MoonTextAreaState, MoonTone,
    MoonTreeState, MoonWindowFrame, Root, h_flex, rgba_from, v_flex,
};

use crate::{Backend, design};
use moon_core::feed::{SchemaField, SchemaFieldUi, SchemaSection, StrategyRow};
use moon_core::session::{CoreId, CoreStore};
use rust_i18n::t;

use filter::StrategyFilter;
use logic::*;
use rules::{Rules, Values};

pub type Key = (CoreId, u64);
type FieldEditKey = (CoreId, u64, String);

const STRATEGIES_HEADER_H: f32 = 32.0;

fn moon(hex: u32) -> Hsla {
    rgba_from(hex, 1.0)
}

fn moon_alpha(hex: u32, alpha: f32) -> Hsla {
    rgba_from(hex, alpha)
}

/// Состояние окна «Стратегии» (порт egui `StrategiesState` + рендер 4 панелей).
pub struct StrategiesView {
    backend: Entity<Backend>,
    /// Текстовое поле поиска — значение читаем в фильтр.
    search: Entity<MoonInputState>,
    /// Фильтры дерева (вид/направление/только активные); `search` синхр. из инпута.
    filter: StrategyFilter,
    /// Текущая (первичная) стратегия — источник схемы/секций (ядро, id).
    selected: Option<Key>,
    /// Множественный выбор (ядро, id) — подсветка + объединённый показ параметров.
    sel: HashSet<Key>,
    /// Якорь для range-выбора по Shift.
    anchor: Option<Key>,
    /// Плоский порядок видимых стратегий прошлого кадра — для Shift-диапазона.
    flat_order: Vec<Key>,
    /// Состояние дерева MoonTree (флэттинг/виртуализация/раскрытие/DnD-hitbox). Выбор/стейджинг
    /// остаются в полях выше — `TreeState` лишь рендерит и отдаёт hitbox-и под декораторы.
    tree_state: Entity<MoonTreeState>,
    /// Индекс выбранной секции в схеме её вида. НЕ сбрасывается при смене стратегии,
    /// только клампится при выходе за диапазон.
    selected_section: usize,
    /// Стейджинг чекбоксов: (ядро, id) → желаемый checked. Уходит на сервер по
    /// старт/стоп отмеченных, затем очищается.
    staged: HashMap<Key, bool>,
    /// Draft редактирования полей: (ядро, id, field) → новая строка UI.
    field_edits: HashMap<FieldEditKey, String>,
    /// Живые состояния single-line редакторов видимых/посещённых полей.
    field_inputs: HashMap<String, Entity<MoonInputState>>,
    /// Живые состояния memo/formula редакторов видимых/посещённых полей.
    field_memos: HashMap<String, Entity<MoonTextAreaState>>,
    /// Поле, для которого открыт контекстный helper/autocomplete.
    focused_field: Option<String>,
    /// Раскрытые ядра в дереве.
    expanded_cores: HashSet<CoreId>,
    /// Раскрытые папки в дереве: (ядро, путь).
    expanded_folders: HashSet<(CoreId, String)>,
    /// Правила зависимостей полей (param_deps.toml; hot-reload).
    rules: Rules,
    /// Буфер копирования стратегий/папок (исходные данные — для межъядерной вставки).
    clipboard: Option<Vec<tree_ops::ClipItem>>,
    /// Пустые UI-папки (до наполнения первой стратегией): (ядро, путь через `/`).
    ui_folders: HashSet<(CoreId, String)>,
    /// Активная модалка операции над деревом (создать/переименовать/подтвердить).
    op: Option<tree_ui::TreeOp>,
    /// Ввод модалки создания/переименования — пересоздаётся на каждое открытие, чтобы
    /// модалка всегда стартовала с актуальным начальным значением.
    op_input: Option<Entity<MoonInputState>>,
    /// Начальное значение для `op_input` при следующем создании (render строит инпут).
    op_input_init: String,
    /// Ожидаем появления стратегии (эхо ядра после create/paste): (ядро, имя) — как
    /// придёт, выбираем её в дереве. Очищается после выбора.
    pending_select: Option<(CoreId, String)>,
    /// Сигнатура данных стратегий/схем, которые реально меняют окно.
    last_sig: u64,
    /// Показывать только активные параметры (галка над параметрами).
    only_active_params: bool,
    focus: FocusHandle,
}

impl StrategiesView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx
            .new(|cx| MoonInputState::new(window, cx).placeholder(t!("strat.search").to_string()));
        // Печать в поиске → обновить фильтр и перерисовать. Render не должен читать input
        // как event source.
        cx.subscribe(&search, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.filter.search != value {
                    this.filter.search = value;
                    cx.notify();
                }
            }
        })
        .detach();

        let initial_sig = strategies_sig(backend.read(cx));

        // Новые снимки стратегий/схемы → перерисовка. Hot-reload правил живёт на
        // отдельном file-mtime таймере ниже: backend data observe не должен быть
        // суррогатным polling loop для файловой системы.
        cx.observe(&backend, |this, backend, cx| {
            let sig = strategies_sig(backend.read(cx));
            if sig != this.last_sig {
                this.last_sig = sig;
                this.sync_pending_select(cx);
                this.clamp_selected_section(cx);
                cx.notify();
            }
        })
        .detach();

        // Сохранять положение/размер окна «Стратегии» в layout — чтобы открывалось на прежнем
        // месте. Дебаунс-сейв делает дренаж по `layout_dirty` (как у окон групп).
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.strategies_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.strategies_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        if std::env::var_os("MOON_STRATEGY_RULES_HOT_RELOAD").is_some() {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                loop {
                    executor.timer(Duration::from_secs(1)).await;
                    let alive = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            if this.rules.reload_if_changed() {
                                cx.notify();
                            }
                        })
                        .is_ok()
                    });
                    if !alive {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            backend,
            search,
            filter: StrategyFilter::default(),
            selected: None,
            sel: HashSet::new(),
            anchor: None,
            flat_order: Vec::new(),
            tree_state: cx.new(|cx| MoonTreeState::new(cx)),
            selected_section: 0,
            staged: HashMap::new(),
            field_edits: HashMap::new(),
            field_inputs: HashMap::new(),
            field_memos: HashMap::new(),
            focused_field: None,
            expanded_cores: HashSet::new(),
            expanded_folders: HashSet::new(),
            rules: Rules::load(),
            clipboard: None,
            ui_folders: HashSet::new(),
            op: None,
            op_input: None,
            op_input_init: String::new(),
            pending_select: None,
            last_sig: initial_sig,
            // По умолчанию неактивные параметры скрыты (галка включена).
            only_active_params: true,
            focus: cx.focus_handle(),
        }
    }

    // ── Выбор ───────────────────────────────────────────────────────────────

    /// Клик по стратегии с учётом модификаторов: Shift — диапазон от якоря (по
    /// `order`), Ctrl/Cmd — добавить/убрать по одной, без модификатора — выбрать одну.
    fn apply_click(&mut self, key: Key, order: &[Key], shift: bool, command: bool) -> bool {
        let before_selected = self.selected;
        let before_anchor = self.anchor;
        let before_sel = self.sel.clone();
        if shift {
            if let Some(a) = self.anchor {
                let ia = order.iter().position(|k| *k == a);
                let ib = order.iter().position(|k| *k == key);
                if let (Some(ia), Some(ib)) = (ia, ib) {
                    let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
                    self.sel = order[lo..=hi].iter().copied().collect();
                } else {
                    self.sel = std::iter::once(key).collect();
                }
            } else {
                self.sel = std::iter::once(key).collect();
                self.anchor = Some(key);
            }
        } else if command {
            if !self.sel.remove(&key) {
                self.sel.insert(key);
            }
            self.anchor = Some(key);
        } else {
            self.sel.clear();
            self.sel.insert(key);
            self.anchor = Some(key);
        }
        // Первичная (источник схемы/секций) — всегда кликнутая. Раздел не сбрасываем.
        self.selected = Some(key);
        before_selected != self.selected || before_anchor != self.anchor || before_sel != self.sel
    }

    fn sync_pending_select(&mut self, cx: &App) -> bool {
        let Some((core, name)) = self.pending_select.clone() else {
            return false;
        };
        let key = {
            let store = self.backend.read(cx).session.store();
            store.core(core).and_then(|cd| {
                cd.strategies
                    .iter()
                    .find(|row| row.name == name)
                    .map(|row| (core, row.id))
            })
        };
        let Some(key) = key else {
            return false;
        };
        self.selected = Some(key);
        self.sel.clear();
        self.sel.insert(key);
        self.pending_select = None;
        self.clamp_selected_section(cx);
        true
    }

    fn clamp_selected_section(&mut self, cx: &App) -> bool {
        let store = self.backend.read(cx).session.store();
        let Some(sections) = selected_sections(self, store) else {
            return false;
        };
        if self.selected_section < sections.len() {
            return false;
        }
        self.selected_section = 0;
        true
    }

    // ── Действия (старт/стоп отмеченных) ─────────────────────────────────────

    /// «Старт/стоп отмеченных»: на ядро — изменённые галки (diff стейджинга против
    /// серверного checked) + команда старт/стоп, если у ядра есть отмеченная стратегия
    /// или есть правки галок. Шлёт через `session.apply_strategies`, чистит стейджинг.
    fn apply_start_stop(
        &mut self,
        cores: &[(CoreId, String)],
        start: bool,
        cx: &mut Context<Self>,
    ) {
        // Собрать действия (читаем store), затем применить (повторный borrow backend).
        let mut actions: Vec<(CoreId, Vec<(u64, bool)>, bool)> = Vec::new();
        {
            let b = self.backend.read(cx);
            let store = b.session.store();
            for (core, _) in cores {
                let Some(cd) = store.core(*core) else {
                    continue;
                };
                let mut checks = Vec::new();
                let mut has_checked = false;
                for r in &cd.strategies {
                    let eff = self
                        .staged
                        .get(&(*core, r.id))
                        .copied()
                        .unwrap_or(r.checked);
                    if eff != r.checked {
                        checks.push((r.id, eff));
                    }
                    if eff {
                        has_checked = true;
                    }
                }
                if !checks.is_empty() || has_checked {
                    actions.push((*core, checks, start));
                }
            }
        }
        if actions.is_empty() {
            return;
        }
        let b = self.backend.read(cx);
        for (core, checks, st) in actions {
            if let Err(error) = b.session.apply_strategies(core, checks, Some(st)) {
                log::warn!("apply strategies failed: {error}");
                return;
            }
        }
        self.staged.clear();
        cx.notify();
    }

    fn stage_field_value(
        &mut self,
        keys: &[Key],
        field: &str,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        self.focused_field = Some(field.to_string());
        for (core, id) in keys {
            self.field_edits
                .insert((*core, *id, field.to_string()), value.clone());
        }
        cx.notify();
    }

    fn apply_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        // Группируем по ЯДРУ → внутри по стратегии. На ядро уходит ОДНА команда со всеми его
        // правками: иначе при нескольких выбранных стратегиях одного ядра раздельные
        // `sync_local_strategies` перетирали бы друг друга (применялось бы к одной).
        let mut per_core: HashMap<CoreId, HashMap<u64, Vec<(String, String)>>> = HashMap::new();
        for ((core, id, field), value) in &self.field_edits {
            per_core
                .entry(*core)
                .or_default()
                .entry(*id)
                .or_default()
                .push((field.clone(), value.clone()));
        }
        let b = self.backend.read(cx);
        for (core, strat_edits) in per_core {
            let edits: Vec<(u64, Vec<(String, String)>)> = strat_edits.into_iter().collect();
            if let Err(error) = b.session.edit_strategies(core, edits) {
                log::warn!("edit strategies failed: {error}");
                return;
            }
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn discard_field_edits(&mut self, cx: &mut Context<Self>) {
        if self.field_edits.is_empty() {
            return;
        }
        self.clear_field_draft();
        cx.notify();
    }

    fn clear_field_draft(&mut self) {
        self.field_edits.clear();
        self.field_inputs.clear();
        self.field_memos.clear();
        self.focused_field = None;
    }

    /// Поставить фильтр поиска по точному имени стратегии и сфокусировать поле — пункт
    /// меню «Найти все с этим именем». Имя пишем и в фильтр, и в сам инпут (`set_value`
    /// не эмитит Change, поэтому фильтр выставляем вручную).
    pub(super) fn search_by_name(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.filter.search = name.clone();
        self.search.update(cx, |st, cx| {
            st.set_value(name, window, cx);
            st.focus(window, cx);
        });
        cx.notify();
    }

    fn field_input_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        if let Some(state) = self.field_inputs.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // Кэш мог устареть: значение в сторе изменилось (эхо сервера / правка в другом
            // выборе), а `value` уже актуально (учитывает черновик). Синхронизируем тихо:
            // `sync_value` не эмитит Change, поэтому не зацикливает staged edits.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonInputState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_inputs.insert(id, state.clone());
        state
    }

    fn field_memo_state(
        &mut self,
        id: String,
        value: String,
        keys: Arc<Vec<Key>>,
        field: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonTextAreaState> {
        if let Some(state) = self.field_memos.get(&id) {
            let cur = state.read(cx).value().to_string();
            if cur == value {
                return state.clone();
            }
            // См. field_input_state: тихо синхронизируем с актуальным значением.
            let state = state.clone();
            state.update(cx, |s, cx| s.sync_value(value.clone(), cx));
            return state;
        }
        let state = cx.new(|cx| MoonTextAreaState::new(window, cx).default_value(value));
        cx.subscribe(&state, move |this, state, ev: &MoonTextAreaEvent, cx| {
            if matches!(ev, MoonTextAreaEvent::Change) {
                let value = state.read(cx).value().to_string();
                this.stage_field_value(keys.as_ref(), &field, value, cx);
            }
        })
        .detach();
        self.field_memos.insert(id, state.clone());
        state
    }

    fn append_formula_snippet(&mut self, field: &str, snippet: &str, cx: &mut Context<Self>) {
        let needle = field_id(field);
        let state = self
            .field_memos
            .iter()
            .find_map(|(id, state)| id.contains(&needle).then_some(state.clone()));
        let current = state
            .as_ref()
            .map(|state| state.read(cx).value().to_string())
            .unwrap_or_default();
        let next = append_snippet(&current, snippet);
        if let Some(state) = state {
            state.update(cx, |state, cx| state.sync_value(next, cx));
        }
    }

    /// Раскрыть каждый уровень пути (накопительные префиксы) в `expanded_folders`.
    /// Единый помощник раскрытия цепочки папок (используется при «развернуть всё» и при
    /// создании папки, чтобы новая была сразу видна).
    pub(super) fn expand_path<'a>(
        &mut self,
        core: CoreId,
        segments: impl Iterator<Item = &'a str>,
    ) {
        let mut acc = String::new();
        for part in segments {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.expanded_folders.insert((core, acc.clone()));
        }
    }

    /// Развернуть все узлы (если `collapsed`) или свернуть все (иначе).
    fn expand_collapse_toggle(
        &mut self,
        cores: &[(CoreId, String)],
        store: &CoreStore,
        collapsed: bool,
    ) {
        if !collapsed {
            self.expanded_cores.clear();
            self.expanded_folders.clear();
            return;
        }
        for (c, _) in cores {
            self.expanded_cores.insert(*c);
            let Some(cd) = store.core(*c) else { continue };
            let paths: Vec<String> = cd
                .strategies
                .iter()
                .map(|r| r.folder_path.clone())
                .collect();
            for path in paths {
                self.expand_path(*c, tree_ops::path_segments(&path));
            }
        }
    }

    // ── Панель 1: дерево ──────────────────────────────────────────────────────

    fn sections_panel(&self, store: &CoreStore, cx: &Context<Self>) -> AnyElement {
        let p = MoonPalette::active(cx);
        let border = moon(p.border);
        let mut col = v_flex()
            .w(px(264.0))
            .h_full()
            .bg(moon(p.shell_high))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .border_r_1()
            .border_color(border)
            .px(design::ui_px(cx, 10.0))
            .py(design::ui_px(cx, 12.0))
            .gap(design::ui_px(cx, 7.0))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t!("strat.sections").to_string()),
            )
            .child(div().w_full().h(px(1.0)).bg(border));

        let Some(sections) = selected_sections(self, store) else {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.no_selection").to_string()),
                )
                .into_any_element();
        };
        if sections.is_empty() {
            return col
                .child(
                    div()
                        .mt_2()
                        .text_color(moon(p.text_muted))
                        .child(t!("strat.no_schema").to_string()),
                )
                .into_any_element();
        }
        let values = selected_values(self, store);

        // Порядок: сначала активные, потом неактивные; внутри групп — порядок схемы.
        let mut order: Vec<(usize, bool)> = sections
            .iter()
            .enumerate()
            .map(|(i, sec)| (i, section_active(&self.rules, &values, sec)))
            .collect();
        order.sort_by_key(|(_, active)| !active);

        let mut list = v_flex().w_full().gap_0();
        for (i, active) in order {
            let sec = &sections[i];
            let on = self.selected_section == i;
            let tcol = if !active { p.text_muted } else { p.text };
            let mut row = div()
                .id(SharedString::from(format!("sec-{i}")))
                .w_full()
                .h(design::fit_h_px(cx, 24.0, 14.0, 5.0))
                .px(design::ui_px(cx, 6.0))
                .rounded(design::ui_px(cx, 3.0))
                .border_1()
                .border_color(moon_alpha(p.border, 0.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(moon(tcol))
                .child(sec.title.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.selected_section != i {
                        this.selected_section = i;
                        cx.notify();
                    }
                }));
            if on {
                row = row
                    .bg(moon_alpha(p.amber, 0.16))
                    .border_color(moon_alpha(p.amber, 0.55));
            } else {
                row = row.hover(move |s| s.bg(moon_alpha(p.panel, 0.74)));
            }
            list = list.child(row);
        }
        col = col.child(
            div()
                .id("strat-sections-scroll")
                .flex_1()
                .w_full()
                .overflow_y_scroll()
                .child(list),
        );
        col.into_any_element()
    }

    // ── Панель 3: параметры выбранной секции ────────────────────────────────
}

fn strategies_sig(b: &Backend) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| {
            a.wrapping_mul(31)
                .wrapping_add(c.strategies_rev)
                .wrapping_mul(31)
                .wrapping_add(c.schema_rev)
        })
}

impl EventEmitter<()> for StrategiesView {}
impl Focusable for StrategiesView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StrategiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Список ядер (id, имя) — все подключённые, как egui (session.sessions()).
        let cores: Vec<(CoreId, String)> = {
            let b = self.backend.read(cx);
            b.session
                .sessions()
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect()
        };

        // Адаптер MoonTree (owned, без заимствования стора наружу), затем синк состояния дерева.
        let build = {
            let store = self.backend.read(cx).session.store();
            tree_moon::build(self, store, &cores)
        };
        self.flat_order = build.flat;
        let searching = build.searching;
        self.tree_state.update(cx, |st, c| {
            st.set_items(build.items, c);
            st.set_force_expanded(searching, c);
            st.set_expanded(build.expanded_ids, c);
        });
        let node_data = std::rc::Rc::new(build.node_data);

        let (tree, sections, params_model) = {
            let store = self.backend.read(cx).session.store();
            (
                self.tree_panel(store, &cores, node_data, cx),
                self.sections_panel(store, cx),
                self.params_model(store),
            )
        };
        let params = self.params_panel(params_model, window, cx);

        let p = MoonPalette::active(cx);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.handle_tree_key(ev, window, cx);
            }))
            .child(strategies_header(p, cx))
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .child(tree)
                    .child(sections)
                    .child(params),
            );
        root = root.child(
            MoonWindowFrame::tool("strategies-window-frame-hit", chrome_width)
                .header_height(STRATEGIES_HEADER_H)
                .leading_inset(design::titlebar_leading_inset())
                .show_controls(design::show_custom_window_controls())
                .hit_overlay(),
        );
        root
    }
}

fn strategies_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("strategies-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, STRATEGIES_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("strategies-titlebar-title", 0.0)
                .title_cluster(t!("strat.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("strategies-window-frame-visual", 0.0)
                    .header_height(STRATEGIES_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Открыть окно «Стратегии» (tool/secondary окно). Дедуп окон — в `Backend`.
pub fn open(backend: Entity<Backend>, owner: Option<AnyWindowHandle>, cx: &mut App) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).strategies_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // Tool-окно: визуально и поведенчески это часть терминала, а не отдельное приложение
    // в taskbar. Геометрию восстанавливаем из layout (её сохраняет StrategiesView).
    let saved = backend.read(cx).layout.strategies_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(120.0), px(90.0)),
            size: size(px(1180.0), px(680.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Мультимонитор: без display_id окно создаётся на primary и при bounds вне него gpui
    // откатывается на дефолт — ищем монитор, содержащий сохранённую точку.
    let display_id = saved.and_then(|g| {
        let origin = point(px(g.x as f32), px(g.y as f32));
        cx.displays()
            .into_iter()
            .find(|d| d.bounds().contains(&origin))
            .map(|d| d.id())
    });
    let mut opts = crate::windowing::tool_window_options(
        t!("strat.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(920.0), px(560.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| StrategiesView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.strategies_window = Some(handle));
    }
}
