//! Состояние и окно «Скринер»: `ScreenerView` (фильтры/сортировка/пересборка строк),
//! рендер окна, шапка и `open()`. Схема колонок и рендер строк — в [`super::table`].

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonBackgroundPolicy, MoonButtonSize, MoonButtonVariant, MoonDataTable, MoonDataTableColumn,
    MoonDataTableState, MoonDropdown, MoonInput, MoonInputEvent, MoonInputState, MoonMenuItem,
    MoonMenuSize, MoonPalette, MoonWindowFrame, Root, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::session::CoreId;

use crate::panels::{RadioMark, RenderGate, data_table_host, radio_items};
use crate::{Backend, design};

use super::table::{COLS, ColDef, Entry, moon, moon_alpha, parse_vol, screener_row, sort_entries};

const SCREENER_HEADER_H: f32 = 32.0;

/// Фильтр по ядру (как источник в Ордерах): все ядра или одно конкретное.
#[derive(Clone, Copy, PartialEq)]
enum ScrSource {
    All,
    Core(CoreId),
}

/// Состояние окна «Скринер».
pub struct ScreenerView {
    pub(super) backend: Entity<Backend>,
    /// Отфильтрованные и отсортированные строки текущего кадра.
    rows: Rc<Vec<Entry>>,
    /// Всего строк до фильтра (для счётчика внизу).
    total: usize,
    /// Фильтр по монете (подстрока, без регистра).
    coin_input: Entity<MoonInputState>,
    /// Минимальный 24ч объём (поддерживает суффиксы k/m).
    dvol_input: Entity<MoonInputState>,
    /// Ключ сортировки (из `COLS`) + направление.
    sort_key: String,
    sort_desc: bool,
    /// Фильтр по ядру (не персистится — сессионное, как в Ордерах).
    source: ScrSource,
    /// Видимые колонки (ключи из `COLS`). Персистится в layout (`screener_columns`).
    visible_cols: HashSet<String>,
    /// Состояние таблицы (перетаскивание/ширины колонок + индикатор сортировки).
    table_state: Entity<MoonDataTableState>,
    gate: RenderGate,
    focus: FocusHandle,
}

impl ScreenerView {
    fn new(backend: Entity<Backend>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let coin_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("BTC"));
        let dvol_input = cx.new(|cx| MoonInputState::new(window, cx).placeholder("500k"));
        for input in [&coin_input, &dvol_input] {
            cx.subscribe(input, |this: &mut Self, _, ev: &MoonInputEvent, cx| {
                if matches!(ev, MoonInputEvent::Change) {
                    this.rebuild(cx);
                    cx.notify();
                }
            })
            .detach();
        }

        // Скринер всегда отдельное окно → контекст ширин фиксированный `:win`.
        let widths_id = crate::table_persist::ctx_id("screener-table", true);
        let saved_widths = crate::table_persist::saved(backend.read(cx), &widths_id);
        let table_state = cx.new(|_| {
            let mut s = MoonDataTableState::new();
            s.sort_column = Some("vol24".into());
            s.sort_ascending = false;
            s.column_widths = saved_widths;
            s
        });
        // Ресайз колонки мутирует state → сохраняем ширины (универсальный сейвер).
        let widths_id_obs = widths_id.clone();
        cx.observe(&table_state, move |this, state, cx| {
            crate::table_persist::persist(&this.backend, &widths_id_obs, &state, cx);
        })
        .detach();

        // Живые данные: рынки обновляются пакетами каждую секунду, гейт держит
        // перестройку на 1 Гц (секундное ведро; сигнатура не нужна — таблица
        // живая всегда, пока есть ядра).
        cx.observe(&backend, |this, _, cx| {
            let now = moon_chart::paint::now_unix_ms();
            if this.gate.should_notify(0, now) {
                this.rebuild(cx);
                cx.notify();
            }
        })
        .detach();

        // Сохранять положение/размер окна в layout — открытие на прежнем месте.
        cx.observe_window_bounds(window, |this, window, cx| {
            let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
                return;
            };
            this.backend.update(cx, |b, _| {
                if b.layout.screener_window.map(|g| (g.x, g.y, g.w, g.h)) != Some((x, y, w, h)) {
                    b.layout.screener_window =
                        Some(moon_core::config::layout::GeomRect { x, y, w, h });
                    b.layout_dirty = true;
                }
            });
        })
        .detach();

        // Видимые колонки: единый per-контекст дескриптор (`table_visible_columns` по
        // `screener-table:win`) — приоритетный источник; при его отсутствии мигрируем со старого
        // `screener_columns`. Неизвестные ключи отбрасываем (колонку могли переименовать/удалить).
        let saved_cols = crate::table_persist::visible(backend.read(cx), &widths_id)
            .or_else(|| backend.read(cx).layout.screener_columns.clone());
        let visible_cols: HashSet<String> = match saved_cols {
            Some(list) => {
                let set: HashSet<String> = list
                    .iter()
                    .filter(|k| COLS.iter().any(|c| c.0 == k.as_str()))
                    .cloned()
                    .collect();
                if set.is_empty() {
                    COLS.iter().map(|c| c.0.to_string()).collect()
                } else {
                    set
                }
            }
            None => COLS.iter().map(|c| c.0.to_string()).collect(),
        };

        let mut this = Self {
            backend,
            rows: Rc::new(Vec::new()),
            total: 0,
            coin_input,
            dvol_input,
            sort_key: "vol24".to_string(),
            sort_desc: true,
            source: ScrSource::All,
            visible_cols,
            table_state,
            gate: RenderGate::default(),
            focus: cx.focus_handle(),
        };
        this.rebuild(cx);
        this
    }

    /// Пересобрать строки: сбор по группам ядер → оверлей ордеров → фильтры → сортировка.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        crate::diag::bump(&crate::diag::SCREENER_REBUILD);
        let coin_filter = self.coin_input.read(cx).value().trim().to_uppercase();
        let min_vol = parse_vol(self.dvol_input.read(cx).value().trim());

        let b = self.backend.read(cx);
        let source = b.session.market_source();
        // Группы дедупа: провайдер → все ядра его биржи (порядок стабилен по конфигу).
        // При фильтре по ядру остаётся одна группа из него самого (аккаунтные
        // колонки — только его), чарт открывается на нём же.
        let mut groups: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
        let mut names: HashMap<CoreId, SharedString> = HashMap::new();
        for s in b.session.sessions() {
            names.insert(s.id, SharedString::from(s.name.clone()));
            if let ScrSource::Core(only) = self.source {
                if s.id != only {
                    continue;
                }
            }
            let Some(provider) = source.provider_of(s.id) else {
                continue;
            };
            match groups.iter_mut().find(|(p, _)| *p == provider) {
                Some((_, members)) => members.push(s.id),
                None => groups.push((provider, vec![s.id])),
            }
        }

        let store = b.session.store();
        let mut entries: Vec<Entry> = Vec::new();
        for (provider, members) in &groups {
            let mut rows = source.screener_rows(*provider, members);
            // Оверлей открытых ордеров по монете (сумма по ядрам группы).
            let mut order_counts: HashMap<&str, u32> = HashMap::new();
            let member_data: Vec<_> = members.iter().filter_map(|&m| store.core(m)).collect();
            for cd in &member_data {
                for o in &cd.orders {
                    *order_counts.entry(o.market.as_str()).or_default() += 1;
                }
            }
            for row in &mut rows {
                row.orders = order_counts.get(row.market.as_str()).copied().unwrap_or(0);
            }
            // Ядро для колонки Core и открытия чарта: при фильтре — выбранное
            // ядро (его ордера/линии на чарте), иначе провайдер группы.
            let open_core = match self.source {
                ScrSource::Core(id) => id,
                ScrSource::All => *provider,
            };
            let core_name = names
                .get(&open_core)
                .cloned()
                .unwrap_or_else(|| SharedString::from(format!("#{open_core}")));
            entries.extend(rows.into_iter().map(|row| Entry {
                row,
                core_name: core_name.clone(),
                open_core,
            }));
        }

        self.total = entries.len();
        if !coin_filter.is_empty() {
            entries.retain(|e| {
                e.row.market.to_uppercase().contains(&coin_filter)
                    || e.row.coin.to_uppercase().contains(&coin_filter)
            });
        }
        if min_vol > 0.0 {
            entries.retain(|e| e.row.vol_24h >= min_vol);
        }
        sort_entries(&mut entries, &self.sort_key, self.sort_desc);
        self.rows = Rc::new(entries);
    }

    fn set_sort(&mut self, key: &str, desc: bool, cx: &mut Context<Self>) {
        self.sort_key = key.to_string();
        self.sort_desc = desc;
        self.rebuild(cx);
        cx.notify();
    }

    fn set_source(&mut self, source: ScrSource, cx: &mut Context<Self>) {
        if self.source != source {
            self.source = source;
            self.rebuild(cx);
            cx.notify();
        }
    }

    /// Тогл видимости колонки + персист списка в layout (порядок каноничный из
    /// `COLS`). Последнюю видимую колонку скрыть нельзя (таблица опустеет).
    fn toggle_col(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.visible_cols.contains(key) {
            if self.visible_cols.len() == 1 {
                return;
            }
            self.visible_cols.remove(key);
        } else {
            self.visible_cols.insert(key.to_string());
        }
        self.persist_visible_cols(cx);
    }

    /// «Все»-тумблер колонок: включить все / повторно — оставить одну первую
    /// (все скрыть нельзя — таблица опустеет).
    fn toggle_all_cols(&mut self, cx: &mut Context<Self>) {
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        self.visible_cols = if all_on {
            std::iter::once(COLS[0].0.to_string()).collect()
        } else {
            COLS.iter().map(|c| c.0.to_string()).collect()
        };
        self.persist_visible_cols(cx);
    }

    /// Персист списка видимых колонок (порядок каноничный из `COLS`).
    fn persist_visible_cols(&self, cx: &mut Context<Self>) {
        let list: Vec<String> = COLS
            .iter()
            .filter(|c| self.visible_cols.contains(c.0))
            .map(|c| c.0.to_string())
            .collect();
        // Единый per-контекст дескриптор полей (`screener-table:win`). Скринер всегда окно,
        // потому контекст один; храним явный список видимых ключей (в т.ч. когда видимы все —
        // список полный), устаревший `screener_columns` больше не пишем (мигрируем при чтении).
        let id = crate::table_persist::ctx_id("screener-table", true);
        crate::table_persist::set_visible(&self.backend, &id, list, cx);
        cx.notify();
    }

    /// Открыть монету на Main-стеке активной вкладки (как клик по токену в Ордерах).
    fn open_chart(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(e) = self.rows.get(ix) else {
            return;
        };
        let core = e.open_core;
        let market = e.row.market.clone();
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            b.open_request_activate = false;
            bcx.notify();
        });
    }

    fn table(&self, cx: &Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let rows = self.rows.clone();
        let row_count = rows.len();
        let view = cx.entity();
        let table_rows = rows.clone();
        let sort_view = cx.entity();
        let dbl_view = cx.entity();
        let state_reset = self.table_state.clone();
        // Видимые колонки в каноничном порядке — общий список для header и строк.
        let visible: Rc<Vec<&'static ColDef>> = Rc::new(
            COLS.iter()
                .filter(|c| self.visible_cols.contains(c.0))
                .collect(),
        );
        let row_cols = visible.clone();

        data_table_host(
            "screener-table-host",
            row_count == 0,
            t!("screener.empty").to_string(),
            p,
            cx,
            MoonDataTable::new("screener-table", row_count, move |ix, _window, _app| {
                screener_row(&table_rows[ix], &view, p, &row_cols)
            })
            .columns(
                visible
                    .iter()
                    .map(|&&(key, title, width, right)| {
                        let col = MoonDataTableColumn::new(key, title, width).sortable(true);
                        let col = if right { col.right() } else { col };
                        if key == "market" { col.fixed_left() } else { col }
                    })
                    .collect::<Vec<_>>(),
            )
            .state(&self.table_state)
            .header_height(design::TABLE_HEAD_H)
            .row_height(design::TABLE_ROW_H)
            .on_sort(move |key, ascending, _window, app| {
                let key = key.to_string();
                sort_view.update(app, |t, cx| t.set_sort(&key, !ascending, cx));
            })
            .on_double_click_row(move |ix, _window, app| {
                dbl_view.update(app, |t, cx| t.open_chart(ix, cx));
            })
            .on_select_row(move |_ix, _window, app| {
                // Выделение строк не используем (как Orders) — сбрасываем сразу.
                state_reset.update(app, |s, c| {
                    s.selected_row = None;
                    s.selected_column = None;
                    s.selected_cell = None;
                    c.notify();
                });
            }),
        )
    }

    /// Поле-список ядра (Все ядра + подключённые) — как источник в Ордерах.
    fn source_combo(&self, cx: &Context<Self>) -> impl IntoElement {
        let cores: Vec<(CoreId, String)> = {
            let b = self.backend.read(cx);
            b.session
                .sessions()
                .iter()
                .map(|s| (s.id, s.name.clone()))
                .collect()
        };
        let cur = match self.source {
            ScrSource::All => t!("screener.all_cores").to_string(),
            ScrSource::Core(id) => cores
                .iter()
                .find(|(c, _)| *c == id)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| t!("screener.all_cores").to_string()),
        };
        let view = cx.entity();
        let mut options: Vec<(ScrSource, SharedString, SharedString)> = vec![(
            ScrSource::All,
            "all".into(),
            t!("screener.all_cores").to_string().into(),
        )];
        for (id, name) in &cores {
            options.push((
                ScrSource::Core(*id),
                format!("core-{id}").into(),
                name.clone().into(),
            ));
        }
        let items = radio_items(options, self.source, RadioMark::Check, move |app, src| {
            view.update(app, |t, cx| t.set_source(src, cx));
        });
        MoonDropdown::new("screener-source")
            .label(format!("{cur} ▾"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 118.0))
            .menu_width(design::font_w(cx, 160.0))
            .menu_size(MoonMenuSize::Compact)
            .items(items)
    }

    /// Поле-список видимых колонок: чекбокс-тоглы, меню не закрывается на клик,
    /// последнюю видимую колонку скрыть нельзя. Список персистится (layout).
    fn columns_menu(&self, cx: &Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let visible_count = self.visible_cols.len();
        let mut menu = MoonDropdown::new("screener-columns")
            // Кнопка-глиф вместо поля со списком (общий вид селекторов колонок).
            .segment(moon_ui::MoonButtonSegment::new("▦"))
            .trigger_variant(MoonButtonVariant::Soft)
            .trigger_size(MoonButtonSize::Action)
            .trigger_width(design::font_w(cx, 34.0))
            .menu_width(design::font_w(cx, 170.0))
            .menu_size(MoonMenuSize::Compact)
            .close_on_select(false);
        // «Все» — тумблер: включить все колонки / повторно — оставить одну первую.
        let all_on = COLS.iter().all(|c| self.visible_cols.contains(c.0));
        let all_view = view.clone();
        menu = menu.item(
            MoonMenuItem::with_key("col-all", t!("report.filter.all").to_string())
                .checked(all_on)
                .selected(all_on)
                .on_click(move |_, _, app| {
                    all_view.update(app, |t, cx| t.toggle_all_cols(cx));
                }),
        );
        for &(key, title, ..) in COLS {
            let shown = self.visible_cols.contains(key);
            let last_visible = shown && visible_count == 1;
            let view = view.clone();
            menu = menu.item(
                MoonMenuItem::with_key(format!("col-{key}"), title)
                    .checked(shown)
                    .disabled(last_visible)
                    .on_click(move |_, _, app| {
                        view.update(app, |t, cx| t.toggle_col(key, cx));
                    }),
            );
        }
        div()
            .id("screener-cols-tip")
            .tooltip(|_window, cx| {
                cx.new(|_| moon_ui::MoonTooltipView::new(t!("screener.columns").to_string()))
                    .into()
            })
            .child(menu)
    }

    /// Нижняя полоса: селектор ядра + фильтры Coin/DVol слева (как в Moonbot),
    /// меню колонок и счётчик справа.
    fn bottom_bar(&self, p: MoonPalette, cx: &Context<Self>) -> impl IntoElement {
        let label = |text: &'static str| {
            div()
                .text_size(design::t_body(cx))
                .text_color(rgb(p.text_soft))
                .child(text)
        };
        h_flex()
            .flex_none()
            .w_full()
            .h(design::fit_h_px(cx, 30.0, 14.0, 9.0))
            .px(design::ui_px(cx, 8.0))
            .gap(design::ui_px(cx, 8.0))
            .items_center()
            .bg(moon(p.shell_high))
            .border_t(px(1.0))
            .border_color(moon_alpha(p.border, 1.0))
            .child(self.source_combo(cx))
            .child(label("Coin"))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-coin")
                        .state(&self.coin_input)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(label("DVol"))
            .child(
                div().w(px(90.0)).child(
                    MoonInput::new("scr-dvol")
                        .state(&self.dvol_input)
                        .small()
                        .cleanable(true),
                ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(design::t_body(cx))
                    .text_color(rgb(p.text_muted))
                    .child(format!(
                        "{} / {} {}",
                        self.rows.len(),
                        self.total,
                        t!("screener.coins")
                    )),
            )
            .child(self.columns_menu(cx))
    }
}

impl EventEmitter<()> for ScreenerView {}
impl Focusable for ScreenerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ScreenerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let chrome_width = match window.window_bounds() {
            WindowBounds::Windowed(b)
            | WindowBounds::Maximized(b)
            | WindowBounds::Fullscreen(b) => f32::from(b.size.width),
        };
        v_flex()
            .size_full()
            .relative()
            .bg(moon(p.shell))
            .text_color(moon(p.text))
            .font_family(design::mono())
            .text_size(design::t_body(cx))
            .line_height(design::line_px(cx, 14.0))
            .track_focus(&self.focus)
            .child(screener_header(p, cx))
            .child(self.table(cx))
            .child(self.bottom_bar(p, cx))
            .child(
                MoonWindowFrame::tool("screener-window-frame-hit", chrome_width)
                    .header_height(SCREENER_HEADER_H)
                    .leading_inset(design::titlebar_leading_inset())
                    .show_controls(design::show_custom_window_controls())
                    .hit_overlay(),
            )
    }
}

fn screener_header(p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .id("screener-window-header")
        .relative()
        .flex_none()
        .w_full()
        .h(design::fit_h_px(cx, SCREENER_HEADER_H, 14.0, 9.0))
        .justify_between()
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .bg(moon(p.shell_high))
        .border_b(px(1.0))
        .border_color(moon_alpha(p.border, 1.0))
        .child(
            MoonWindowFrame::tool("screener-titlebar-title", 0.0)
                .title_cluster(t!("screener.window_title").to_string(), cx)
                .h_full()
                .flex_1()
                .min_w_0(),
        )
        .when(design::show_custom_window_controls(), |this| {
            this.child(
                MoonWindowFrame::tool("screener-window-frame-visual", 0.0)
                    .header_height(SCREENER_HEADER_H)
                    .show_controls(true)
                    .visual_controls(cx),
            )
        })
}

/// Открыть окно «Скринер» (tool-окно, singleton). Дедуп/фокус — в `Backend`.
pub fn open(
    backend: Entity<Backend>,
    owner: Option<AnyWindowHandle>,
    owner_display: Option<DisplayId>,
    cx: &mut App,
) {
    // Уже открыто → сфокусировать.
    if let Some(handle) = backend.read(cx).screener_window {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let saved = backend.read(cx).layout.screener_window;
    let bounds = saved.map_or(
        Bounds {
            origin: point(px(100.0), px(80.0)),
            size: size(px(1500.0), px(760.0)),
        },
        |g| Bounds {
            origin: point(px(g.x as f32), px(g.y as f32)),
            size: size(px(g.w as f32), px(g.h as f32)),
        },
    );
    // Мультимонитор: монитор по сохранённой точке (не-мак) либо от владельца.
    let display_id = crate::windowing::saved_or_owner_display_id(
        saved.map(|g| point(px(g.x as f32), px(g.y as f32))),
        owner,
        owner_display,
        cx,
    );
    let mut opts = crate::windowing::tool_window_options(
        t!("screener.window_title").to_string(),
        WindowBounds::Windowed(bounds),
        Some(size(px(900.0), px(480.0))),
        owner,
    );
    opts.display_id = display_id;
    let b = backend.clone();
    if let Ok(handle) = cx.open_window(opts, move |window, cx| {
        crate::windowing::configure_shell_clear_color(window, cx);
        let view = cx.new(|cx| ScreenerView::new(b, window, cx));
        cx.new(|cx| Root::new(view, window, cx).background_policy(MoonBackgroundPolicy::Opaque))
    }) {
        backend.update(cx, |bk, _| bk.screener_window = Some(handle));
        crate::windowing::activate_new_window(handle.into(), cx);
    }
}
