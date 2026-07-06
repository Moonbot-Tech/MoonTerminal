//! Хост-вид ОС-окна откреплённой чарт-вкладки (`DetachedChartHost`): шапка (поиск монеты +
//! масштаб + попап раскладки ⚙ + «закрыть все графики») над панелью чарт-стека. Сам пишет
//! геометрию окна и per-tab настройки в `charts.json` и просит репин по закрытию. Жизненный
//! цикл самого окна (создание/восстановление/репин) живёт в `windows.rs` (`impl ChartTabs`);
//! общая логика попапа ⚙/поиска монеты — трейты [`super::common`] (реализации внизу).

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState};
use rust_i18n::t;
use std::time::Duration;

use super::common::{
    CoinPopupHost, LayoutPopupHost, LayoutPopupSnapshot, StackSetting, set_stack_setting,
};
use super::{AddChartStack, chart_pane_label, coin_search};
use crate::Backend;
use crate::chart_persist::{self, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

mod render;

/// Хост-вид окна откреплённой чарт-вкладки: шапка (масштаб + «закрыть все графики») + панель.
/// Сам пишет геометрию окна в charts.json (`observe_window_bounds`) и просит репин по закрытию
/// (`on_release` → `chart_repin_request`, дренит ChartTabs).
pub(super) struct DetachedChartHost {
    panel: Entity<AddChartStack>,
    backend: Entity<Backend>,
    group: String,
    num: u32,
    bucket: ChartBucket,
    /// Можно ли сохранять геометрию из `observe_window_bounds`. У ВОССТАНОВЛЕННОГО окна сперва
    /// false: авто-размещение gpui на не-primary DPI читается со сдвигом ×scale, и пересохранять
    /// его НЕЛЬЗЯ (иначе позиция уезжает с каждым запуском). Армируется через ~1.5с — дальше
    /// пишем только реальные перемещения пользователя. У свежего детача — сразу true.
    persist_armed: bool,
    /// Логический размер для коррекции на ПЕРВОМ render восстановленного окна: gpui создаёт окно
    /// на primary, и `WM_DPICHANGED` при переезде на монитор с другим DPI пере-масштабирует
    /// РАЗМЕР (позиция уже верная) → форсим сохранённый логический размер один раз. None у детача.
    restore_size: Option<Size<Pixels>>,
    /// Кнопку окна из таскбара убираем `ITaskbarList::DeleteTab` на первых рендерах (когда окно
    /// уже показано и кнопка создана). Окно при этом остаётся обычным independent → FancyZones его
    /// видит. Несколько тиков — подстраховка от гонки «кнопка ещё не появилась».
    taskbar_hide_ticks: u8,
    /// In-scene попап настроек раскладки этой вкладки (кнопка ⚙). Не отдельное ОС-окно:
    /// chart text теперь лежит ниже обычной GPUI scene.
    layout_popup_open: bool,
    /// Был ли курсор внутри popup-а. Уход после первого входа закрывает popup и коммитит ввод.
    layout_popup_hovered: bool,
    /// Поле высоты режима Fit.
    layout_fit_input: Entity<MoonInputState>,
    /// Поле высоты режима Scroll.
    layout_scroll_input: Entity<MoonInputState>,
    /// Поле имени кастомной вкладки (в попапе ⚙, только если окно — откреплённая Custom-вкладка).
    custom_name_input: Entity<MoonInputState>,
    /// Поле ввода монеты (поиск) шапки окна; набор зависит от ядер bucket-а этого окна.
    coin_input: Entity<MoonInputState>,
    /// Текущий текст в поле монеты (зеркало `coin_input`).
    coin_query: String,
    /// Открыт ли список совпадений монеты.
    coin_popup_open: bool,
    /// Фокус корня окна — чтобы хоткеи (`on_key_down`) ловились, когда ничего другого не
    /// сфокусировано. Фокусируем на создании; клик в поле монеты уводит фокус, но клавиши
    /// всплывают обратно к корню. Пока только Scale +/− (масштаб панели окна).
    focus: FocusHandle,
}

impl DetachedChartHost {
    pub(super) fn new(
        panel: Entity<AddChartStack>,
        backend: Entity<Backend>,
        group: String,
        num: u32,
        bucket: ChartBucket,
        restored: bool,
        restore_size: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Геометрия окна (causal bounds event) → charts.json («то же место» при загрузке).
        cx.observe_window_bounds(window, |this, window, cx| {
            this.persist_geometry(window, cx);
        })
        .detach();
        // Состав панели изменился (закрыли «×»/добавили монету) → если окно держит откреплённую
        // кастомную вкладку, пере-персист её тикеров (diff внутри, no-op для обычных окон).
        cx.observe(&panel, |this, _panel, cx| {
            this.persist_custom_coins_if_any(cx);
        })
        .detach();
        // Восстановленное окно не пишет стартовые bounds сразу: на не-primary DPI GPUI/Win32
        // могут прислать временную позицию/размер со scale-сдвигом. Это нельзя сохранять, иначе
        // окно будет уезжать на каждом запуске. Через короткое окно стабилизации снова разрешаем
        // обычный persist пользовательских move/resize. Свежий detach сохраняет геометрию сразу.
        if restored {
            cx.spawn(async move |this, cx| {
                let executor = cx.update(|cx| cx.background_executor().clone());
                executor.timer(Duration::from_millis(1500)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |this, _cx| {
                        this.persist_armed = true;
                        moon_core::detect_diag::line(&format!(
                            "[geom] n={} bucket={:?} persist armed after restore settle",
                            this.num, this.bucket
                        ));
                    })
                    .is_ok()
                });
            })
            .detach();
        }
        // Закрытие окна → репин в стрип (дренит ChartTabs). На выходе приложения запрос не
        // обработается → спека остаётся откреплённой → окно восстановится на след. запуске.
        let (g, n, c) = (group.clone(), num, bucket.clone());
        cx.on_release(move |this, app| {
            this.backend.update(app, |b, cx| {
                b.chart_repin_request.push((g.clone(), n, c.clone()));
                cx.notify();
            });
        })
        .detach();
        // Восстановить сохранённую раскладку + флаг стакана вкладки из charts.json в панель.
        let (group2, num2, bucket2) = (group.clone(), num, bucket.clone());
        let saved = backend.read(cx).chart_specs.iter().find_map(|s| {
            s.matches(&group2, num2, &bucket2).then(|| {
                (
                    s.layout_mode,
                    s.layout_height_fit,
                    s.layout_height_scroll,
                    s.orderbook_enabled,
                    s.liquidations_enabled,
                    s.show_zone,
                    s.auto_pin,
                    (s.cancel_buy_pos, s.panic_sell_pos),
                    s.price_axis_pos,
                    s.time_axis_visible,
                    s.line_labels,
                    s.cursor_labels,
                )
            })
        });
        if let Some((
            m,
            hf,
            hs,
            ob,
            liq,
            sz,
            ap,
            action_pos,
            axis_pos,
            time_axis,
            line_labels,
            cursor_labels,
        )) = saved
        {
            if m.is_some() || hf.is_some() || hs.is_some() {
                panel.update(cx, |p, pcx| p.set_layout(m, hf, hs, pcx));
            }
            if ob.is_some() {
                panel.update(cx, |p, pcx| p.set_orderbook_enabled(ob, pcx));
            }
            if liq.is_some() {
                panel.update(cx, |p, pcx| p.set_liquidations_enabled(liq, pcx));
            }
            if sz.is_some() {
                panel.update(cx, |p, pcx| p.set_show_zone(sz, pcx));
            }
            if ap.is_some() {
                panel.update(cx, |p, pcx| p.set_auto_pin(ap, pcx));
            }
            if action_pos.0.is_some() || action_pos.1.is_some() {
                panel.update(cx, |p, pcx| {
                    p.set_action_btn_pos(action_pos.0, action_pos.1, pcx)
                });
            }
            if axis_pos.is_some() {
                panel.update(cx, |p, pcx| p.set_price_axis_pos(axis_pos, pcx));
            }
            if time_axis.is_some() {
                panel.update(cx, |p, pcx| p.set_time_axis_visible(time_axis, pcx));
            }
            if line_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_line_labels(line_labels, pcx));
            }
            if cursor_labels.is_some() {
                panel.update(cx, |p, pcx| p.set_cursor_labels(cursor_labels, pcx));
            }
        }
        let layout_fit_input = cx.new(|cx| MoonInputState::new(window, cx));
        let layout_scroll_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &layout_fit_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &layout_scroll_input,
            |this, _input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    this.commit_layout_popup(cx);
                }
            },
        )
        .detach();
        // Поле имени кастомной вкладки: коммит переименования по Blur/Enter.
        let custom_name_input = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe(
            &custom_name_input,
            |this, input, ev: &MoonInputEvent, cx| {
                if this.layout_popup_open
                    && matches!(ev, MoonInputEvent::Blur | MoonInputEvent::PressEnter { .. })
                {
                    let name = input.read(cx).value().to_string();
                    this.rename_custom(name, cx);
                }
            },
        )
        .detach();
        let coin_input = cx.new(|cx| {
            MoonInputState::new(window, cx).placeholder(t!("chart.coin.search").to_string())
        });
        cx.subscribe(&coin_input, |this, input, ev: &MoonInputEvent, cx| {
            if matches!(ev, MoonInputEvent::Change) {
                let value = input.read(cx).value().to_string();
                if this.coin_query != value {
                    this.coin_popup_open = !value.trim().is_empty();
                    this.coin_query = value;
                    cx.notify();
                }
            }
        })
        .detach();
        // Фокус корня для хоткеев (масштаб): фокусируем сразу, чтобы Scale +/− работали
        // без предварительного клика в тело окна.
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            panel,
            backend,
            group,
            num,
            bucket,
            persist_armed: !restored,
            restore_size,
            taskbar_hide_ticks: 8,
            layout_popup_open: false,
            layout_popup_hovered: false,
            layout_fit_input,
            layout_scroll_input,
            custom_name_input,
            coin_input,
            coin_query: String::new(),
            coin_popup_open: false,
            focus,
        }
    }

    /// Однозначная торговая цель окна откреплённого чарта: залоченный якорь сравнения либо
    /// единственная монета окна. Многопанельное окно без якоря → `None` (цель неоднозначна,
    /// торговые хоткеи пропускаем — не угадываем рынок).
    fn window_target(&self, cx: &App) -> Option<(CoreId, String)> {
        let p = self.panel.read(cx);
        if let Some(anchor) = p.compare_anchor() {
            return Some(anchor);
        }
        let mut coins = p.coins(cx);
        if coins.len() == 1 {
            return coins.pop();
        }
        None
    }

    /// Хоткей окна откреплённого чарта через ЕДИНЫЙ распознаватель [`crate::hotkeys`].
    /// Масштаб — свой у панели этого окна (применяем напрямую, rev-механизм групп не при
    /// чём). Торговые/фигурные действия — через общий `apply` относительно цели ЭТОГО окна
    /// (`window_target`); фигуры — глобальное состояние, работают всегда.
    fn on_hotkey(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        use crate::hotkeys::HotkeyAction;
        let action = {
            let b = self.backend.read(cx);
            crate::hotkeys::resolve(ev, &b.preview.as_ref().unwrap_or(&b.config).hotkeys)
        };
        let Some(action) = action else {
            return;
        };
        let handled = match action {
            HotkeyAction::ScalePlus | HotkeyAction::ScaleMinus => {
                let zoom_in = matches!(action, HotkeyAction::ScalePlus);
                let next = crate::controls::step_scale(self.panel.read(cx).scale(), zoom_in);
                self.panel.update(cx, |st, scx| st.set_scale(next, scx));
                cx.notify();
                true
            }
            // Ручной ордер по цене под курсором — через чарт под мышью (`hovered_chart`).
            HotkeyAction::NewLong | HotkeyAction::NewShort => {
                let short = matches!(action, HotkeyAction::NewShort);
                let chart = self
                    .backend
                    .read(cx)
                    .hovered_chart
                    .clone()
                    .and_then(|w| w.upgrade());
                match chart {
                    Some(chart) => chart.update(cx, |p, pcx| p.place_order_at_cursor(short, pcx)),
                    None => false,
                }
            }
            other => {
                let target = self.window_target(cx);
                let active_core = target.as_ref().map(|(c, _)| *c);
                self.backend.update(cx, |b, bcx| {
                    crate::hotkeys::apply(other, b, bcx, target.clone(), active_core)
                })
            }
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// Это окно — откреплённая кастомная вкладка? (спек с `custom_coins`).
    fn is_custom(&self, cx: &App) -> bool {
        let (group, num, bucket) = (&self.group, self.num, &self.bucket);
        self.backend
            .read(cx)
            .chart_specs
            .iter()
            .any(|s| s.matches(group, num, bucket) && s.custom_coins.is_some())
    }

    /// Переименовать кастомную вкладку этого окна (поле имени в попапе ⚙): пишем `custom_label`
    /// в charts.json. Заголовок окна (через `chart_pane_label`) обновится на следующем render.
    fn rename_custom(&mut self, name: String, cx: &mut Context<Self>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                s.custom_label = Some(name);
                b.chart_specs_dirty = true;
            }
        });
        cx.notify();
    }

    /// Совпадения поля монеты для этого окна (ядра bucket-а).
    fn coin_results(&self, cx: &App) -> Vec<(CoreId, String, String)> {
        coin_search::search(
            self.backend.read(cx),
            &self.group,
            Some(&self.bucket),
            &self.coin_query,
        )
    }

    /// Если спек этого окна — кастомная вкладка (`custom_coins.is_some()`), переписать её тикеры
    /// из текущего состава панели — ТОЛЬКО при изменении (observe-колбэк зовётся часто). Для
    /// обычных AddToChart-окон — no-op.
    fn persist_custom_coins_if_any(&self, cx: &mut Context<Self>) {
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let is_custom = {
            let specs = &self.backend.read(cx).chart_specs;
            specs
                .iter()
                .any(|s| s.matches(&group, num, &bucket) && s.custom_coins.is_some())
        };
        if !is_custom {
            return;
        }
        let (coins, anchor, broom) = {
            let p = self.panel.read(cx);
            (p.coins(cx), p.compare_anchor(), p.compare_orderbook_only())
        };
        self.backend.update(cx, |b, _| {
            if let Some(s) = b
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                if s.custom_coins.as_deref() != Some(coins.as_slice())
                    || s.compare_anchor != anchor
                    || s.compare_orderbook_only != broom
                {
                    s.custom_coins = Some(coins);
                    s.compare_anchor = anchor;
                    s.compare_orderbook_only = broom;
                    b.chart_specs_dirty = true;
                }
            }
        });
    }

    /// Текущая per-tab раскладка панели этого окна: `(mode, height_fit, height_scroll)`.
    fn panel_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        let p = self.panel.read(cx);
        (
            p.layout_mode(),
            p.layout_height_fit(),
            p.layout_height_scroll(),
        )
    }

    fn persist_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
        // У восстановленного окна сохранение задержано до `persist_armed`: не даём стартовому
        // авто-размещению GPUI/Win32 перезаписать сохранённую позицию DPI-мусором.
        if !self.persist_armed {
            return;
        }
        let Some((x, y, w, h)) = crate::windowing::window_geom(window) else {
            moon_core::detect_diag::line(&format!(
                "[geom] n={} НЕ Windowed → геометрия не сохранена",
                self.num
            ));
            return;
        };
        let geom = chart_persist::WinGeom { x, y, w, h };
        let (group, num, bucket) = (self.group.clone(), self.num, self.bucket.clone());
        let found = self.backend.update(cx, |bk, _| {
            if let Some(s) = bk
                .chart_specs
                .iter_mut()
                .find(|s| s.matches(&group, num, &bucket))
            {
                let cur = s.detached.map(|g| (g.x, g.y, g.w, g.h));
                if cur != Some((geom.x, geom.y, geom.w, geom.h)) {
                    s.detached = Some(geom);
                    bk.chart_specs_dirty = true;
                }
                true
            } else {
                false
            }
        });
        moon_core::detect_diag::line(&format!(
            "[geom] n={num} bucket={bucket:?} → x={} y={} w={} h={} (spec_found={found})",
            geom.x, geom.y, geom.w, geom.h
        ));
    }
}

/// Хозяин попапа ⚙ со стороны выносного окна: цель = ЕДИНСТВЕННАЯ панель окна, ключ персиста —
/// фиксированные (num, bucket) окна. Общая логика попапа/применений — default-методы трейта.
impl LayoutPopupHost for DetachedChartHost {
    fn popup_open(&self) -> bool {
        self.layout_popup_open
    }
    fn set_popup_open(&mut self, open: bool) {
        self.layout_popup_open = open;
    }
    fn popup_hovered(&self) -> bool {
        self.layout_popup_hovered
    }
    fn set_popup_hovered(&mut self, hovered: bool) {
        self.layout_popup_hovered = hovered;
    }
    fn fit_input(&self) -> &Entity<MoonInputState> {
        &self.layout_fit_input
    }
    fn scroll_input(&self) -> &Entity<MoonInputState> {
        &self.layout_scroll_input
    }
    fn rename_input(&self) -> &Entity<MoonInputState> {
        &self.custom_name_input
    }
    fn backend(&self) -> &Entity<Backend> {
        &self.backend
    }
    fn spec_group(&self) -> &str {
        &self.group
    }
    fn spec_key(&self) -> (u32, ChartBucket) {
        (self.num, self.bucket.clone())
    }
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        self.panel_layout(cx)
    }
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation> {
        self.panel.read(cx).layout_orientation()
    }
    fn action_btn_pos_opt(
        &self,
        cx: &App,
    ) -> (
        Option<chart_persist::ChartBtnPos>,
        Option<chart_persist::ChartBtnPos>,
    ) {
        self.panel.read(cx).action_btn_pos()
    }
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot {
        let p = self.panel.read(cx);
        let (cancel_pos, panic_pos) = p.action_btn_pos();
        LayoutPopupSnapshot {
            mode: p.layout_mode().unwrap_or(StackLayoutMode::Fit),
            orientation: p.layout_orientation().unwrap_or(StackOrientation::Vertical),
            orderbook: p.orderbook_enabled().unwrap_or(true),
            liquidations: p.liquidations_enabled().unwrap_or(true),
            show_zone: p.show_zone().unwrap_or(true),
            auto_pin: p.auto_pin().unwrap_or(false),
            cancel_pos: cancel_pos.unwrap_or_default(),
            panic_pos: panic_pos.unwrap_or_default(),
            price_axis_pos: p.price_axis_pos().unwrap_or_default(),
            time_axis: p.time_axis_visible().unwrap_or(true),
            line_labels: p.line_labels().unwrap_or(true),
            cursor_labels: p.cursor_labels().unwrap_or(true),
        }
    }
    fn popup_is_custom(&self, cx: &App) -> bool {
        self.is_custom(cx)
    }
    /// Имя кастомной вкладки — для поля переименования (только если окно держит Custom-вкладку).
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_custom(cx) {
            let name = chart_pane_label(&self.backend, &self.group, self.num, &self.bucket, cx);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        self.panel.update(cx, |s, c| set_stack_setting!(s, c, v));
    }
    /// «Ко всем» из окна: у хоста нет доступа к стекам группы → шлём запрос через Backend
    /// (`ChartApplyAll`, дренится полоской вкладок). Копируем ВСЕ настройки этого окна:
    /// + масштаб + галку стакана. Main не трогаем (include_main=false).
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.panel_layout(cx);
        let mode = Some(mode.unwrap_or(StackLayoutMode::Fit));
        let height_fit = self.read_layout_height(StackLayoutMode::Fit, cx);
        let height_scroll = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let group = self.group.clone();
        let scale = self.panel.read(cx).scale();
        let orderbook = Some(self.panel.read(cx).orderbook_enabled().unwrap_or(true));
        let liquidations = Some(self.panel.read(cx).liquidations_enabled().unwrap_or(true));
        let show_zone = Some(self.panel.read(cx).show_zone().unwrap_or(true));
        let auto_pin = Some(self.panel.read(cx).auto_pin().unwrap_or(false));
        let orientation = self.panel.read(cx).layout_orientation();
        let (cancel_pos, panic_pos) = {
            let (c, pp) = self.panel.read(cx).action_btn_pos();
            (Some(c.unwrap_or_default()), Some(pp.unwrap_or_default()))
        };
        let price_axis_pos = Some(self.panel.read(cx).price_axis_pos().unwrap_or_default());
        let time_axis_visible = Some(self.panel.read(cx).time_axis_visible().unwrap_or(true));
        let line_labels = Some(self.panel.read(cx).line_labels().unwrap_or(true));
        let cursor_labels = Some(self.panel.read(cx).cursor_labels().unwrap_or(true));
        self.backend.update(cx, |bk, bcx| {
            bk.chart_apply_all.push(crate::ChartApplyAll {
                group,
                include_main: false,
                mode,
                height_fit,
                height_scroll,
                scale,
                orderbook,
                liquidations,
                show_zone,
                auto_pin,
                orientation,
                cancel_pos,
                panic_pos,
                price_axis_pos,
                time_axis_visible,
                line_labels,
                cursor_labels,
            });
            bcx.notify();
        });
    }
}

/// Поиск монеты в шапке окна: выбранная монета открывается в стеке ЭТОГО окна; для откреплённой
/// кастомной вкладки состав тикеров тут же пере-персистится.
impl CoinPopupHost for DetachedChartHost {
    fn clear_coin_search(&mut self, cx: &mut Context<Self>) {
        self.coin_query.clear();
        self.coin_popup_open = false;
        cx.notify();
    }
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.panel.update(cx, |p, c| {
            p.add_coin(core, &market, coin_search::MANUAL_COIN_TTL_MS, c)
        });
        // Если это окно — откреплённая КАСТОМНАЯ вкладка, держим её список тикеров в charts.json
        // синхронным (добавили монету в окне → попадёт в персист и переживёт рестарт).
        self.persist_custom_coins_if_any(cx);
        cx.notify();
    }
}
