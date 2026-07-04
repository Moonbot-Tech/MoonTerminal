//! `ChartTabs`: per-вкладочные настройки (контроллер). Геттеры текущих настроек активной
//! вкладки (`active_*`: раскладка/ориентация/стакан/зона/авто-пин/масштаб), применение ко всем
//! стекам/окнам группы (`apply_layout_to_all` + дренаж запросов выносных окон) и реализация
//! [`LayoutPopupHost`] — общая логика попапа ⚙ и одиночных применений (`apply_tab_setting`)
//! живёт в [`super::common`], отрисовка попапа — в [`super::layout_popup`].

use gpui::*;

use super::common::{LayoutPopupHost, LayoutPopupSnapshot, StackSetting, set_stack_setting};
use super::{AddChartStack, ChartTabs, Tab};
use crate::Backend;
use crate::chart_persist::{ChartBtnPos, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_ui::MoonInputState;

impl ChartTabs {
    /// Ключ персиста активной вкладки: Main → (0, Shared); AddToChart/Custom → (num, bucket).
    /// (Для Custom персист всё равно пропускается — см. `persist_active`.)
    pub(super) fn active_stack_key(&self) -> (u32, ChartBucket) {
        match &self.active {
            Tab::Main => (0, ChartBucket::Shared),
            Tab::Add(n, b) | Tab::Custom(n, b) => (*n, b.clone()),
        }
    }

    /// Кастомная (мульти-монетная) вкладка активна? Влияет на юниверс поиска монеты (все ядра
    /// группы) и на гейтинг подписок стаканов по фокусу.
    pub(super) fn active_is_custom(&self) -> bool {
        matches!(self.active, Tab::Custom(..))
    }

    /// Активный Add/Custom-стек (None для Main / если не найден).
    pub(super) fn active_stack(&self) -> Option<Entity<AddChartStack>> {
        match &self.active {
            Tab::Main => None,
            Tab::Add(n, b) | Tab::Custom(n, b) => self.add_stack(*n, b),
        }
    }

    /// Per-tab режим раскладки активной вкладки (None = дефолт Fit).
    pub(super) fn active_layout_mode(&self, cx: &App) -> Option<StackLayoutMode> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_mode(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).layout_mode())
            }
        }
    }

    /// Per-tab высота Fit активной вкладки.
    pub(super) fn active_layout_height_fit(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_fit(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_height_fit()),
        }
    }

    /// Per-tab высота Scroll активной вкладки.
    pub(super) fn active_layout_height_scroll(&self, cx: &App) -> Option<u16> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_height_scroll(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_height_scroll()),
        }
    }

    /// Стакан включён на активной вкладке (None → дефолт вкл).
    pub(super) fn active_orderbook_enabled(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).orderbook_enabled(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).orderbook_enabled()),
        };
        v.unwrap_or(true)
    }

    /// Трейды ликвидаций рисуются на активной вкладке (None → дефолт вкл).
    pub(super) fn active_liquidations_enabled(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).liquidations_enabled(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).liquidations_enabled()),
        };
        v.unwrap_or(true)
    }

    /// Заливка зоны управления включена на активной вкладке (None → дефолт вкл).
    pub(super) fn active_show_zone(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).show_zone(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).show_zone())
            }
        };
        v.unwrap_or(true)
    }

    /// Авто-пин при ордере включён на активной вкладке (None → дефолт выкл).
    pub(super) fn active_auto_pin(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).auto_pin(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).auto_pin())
            }
        };
        v.unwrap_or(false)
    }

    /// Позиции кнопок Cancel Buy / Panic Sell активной вкладки (None → дефолт Right).
    pub(super) fn active_action_btn_pos(&self, cx: &App) -> (ChartBtnPos, ChartBtnPos) {
        let (c, pp) = self.active_action_btn_pos_opt(cx);
        (c.unwrap_or_default(), pp.unwrap_or_default())
    }

    fn active_action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        match &self.active {
            Tab::Main => self.main.read(cx).action_btn_pos(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .map(|p| p.read(cx).action_btn_pos())
                .unwrap_or((None, None)),
        }
    }

    /// Положение оси цен активной вкладки (None → дефолт Left).
    pub(super) fn active_price_axis_pos(&self, cx: &App) -> crate::chart_persist::PriceAxisPos {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).price_axis_pos(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).price_axis_pos()),
        };
        v.unwrap_or_default()
    }

    /// Видимость оси времени активной вкладки (None → дефолт вкл).
    pub(super) fn active_time_axis_visible(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).time_axis_visible(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).time_axis_visible()),
        };
        v.unwrap_or(true)
    }

    /// Видимость подписей у линий активной вкладки (None → дефолт вкл).
    pub(super) fn active_line_labels(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).line_labels(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).line_labels())
            }
        };
        v.unwrap_or(true)
    }

    /// Видимость подписей у перекрестия активной вкладки (None → дефолт вкл).
    pub(super) fn active_cursor_labels(&self, cx: &App) -> bool {
        let v = match &self.active {
            Tab::Main => self.main.read(cx).cursor_labels(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).cursor_labels()),
        };
        v.unwrap_or(true)
    }

    /// Ориентация стека активной вкладки (None → дефолт Vertical).
    pub(super) fn active_layout_orientation(&self, cx: &App) -> Option<StackOrientation> {
        match &self.active {
            Tab::Main => self.main.read(cx).layout_orientation(),
            Tab::Add(n, b) | Tab::Custom(n, b) => self
                .add_stack(*n, b)
                .and_then(|p| p.read(cx).layout_orientation()),
        }
    }

    /// Масштаб цены активной вкладки (None = Авто).
    pub(super) fn active_scale_value(&self, cx: &App) -> Option<f32> {
        match &self.active {
            Tab::Main => self.main.read(cx).scale(),
            Tab::Add(n, b) | Tab::Custom(n, b) => {
                self.add_stack(*n, b).and_then(|p| p.read(cx).scale())
            }
        }
    }

    /// Применить ВСЕ настройки вкладки-источника ко ВСЕМ стекам группы: режим+высоты раскладки,
    /// масштаб цены и галку стакана. `include_main`: трогать ли Main (true — из попапа Main → ко
    /// всем окнам; false — из чартов → Main не трогаем). Персист каждой вкладки.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_layout_to_all(
        &mut self,
        include_main: bool,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        scale: Option<f32>,
        orderbook: Option<bool>,
        liquidations: Option<bool>,
        show_zone: Option<bool>,
        auto_pin: Option<bool>,
        orientation: Option<StackOrientation>,
        cancel_pos: Option<ChartBtnPos>,
        panic_pos: Option<ChartBtnPos>,
        price_axis_pos: Option<crate::chart_persist::PriceAxisPos>,
        time_axis_visible: Option<bool>,
        line_labels: Option<bool>,
        cursor_labels: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let ob = orderbook.unwrap_or(true);
        let liq = liquidations.unwrap_or(true);
        let sz = show_zone.unwrap_or(true);
        let ap = auto_pin.unwrap_or(false);
        let axis = price_axis_pos.unwrap_or_default();
        let time_axis = time_axis_visible.unwrap_or(true);
        let lbl = line_labels.unwrap_or(true);
        let curl = cursor_labels.unwrap_or(true);
        if include_main {
            self.main.update(cx, |s, c| {
                s.set_layout(mode, height_fit, height_scroll, c);
                s.set_scale(scale, c);
                s.set_orderbook_enabled(Some(ob), c);
                s.set_liquidations_enabled(Some(liq), c);
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
                s.set_line_labels(Some(lbl), c);
                s.set_cursor_labels(Some(curl), c);
            });
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.liquidations_enabled = Some(liq);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
                s.line_labels = Some(lbl);
                s.cursor_labels = Some(curl);
            });
        }
        // «Чарты» = add-вкладки в стрипе + кастомные + откреплённые в окна (стеки в self.detached).
        let targets: Vec<(u32, ChartBucket, Entity<AddChartStack>)> = self
            .add
            .iter()
            .chain(self.custom.iter())
            .chain(self.detached.iter())
            .map(|(n, b, p)| (*n, b.clone(), p.clone()))
            .collect();
        for (num, bucket, panel) in targets {
            panel.update(cx, |s, c| {
                s.set_layout(mode, height_fit, height_scroll, c);
                s.set_scale(scale, c);
                s.set_orderbook_enabled(Some(ob), c);
                s.set_liquidations_enabled(Some(liq), c);
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
                s.set_line_labels(Some(lbl), c);
                s.set_cursor_labels(Some(curl), c);
            });
            self.upsert_spec(cx, num, &bucket, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.liquidations_enabled = Some(liq);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
                s.line_labels = Some(lbl);
                s.cursor_labels = Some(curl);
            });
        }
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    /// Дренаж запросов «применить ко всем» из выносных окон чартов ЭТОЙ группы (у них нет доступа
    /// к стекам группы, поэтому шлют через Backend).
    pub(super) fn drain_apply_all(&mut self, cx: &mut Context<Self>) {
        let group = self.group.clone();
        let reqs: Vec<crate::ChartApplyAll> = self.backend.update(cx, |b, _| {
            let (mine, rest): (Vec<_>, Vec<_>) =
                b.chart_apply_all.drain(..).partition(|r| r.group == group);
            b.chart_apply_all = rest;
            mine
        });
        for r in reqs {
            self.apply_layout_to_all(
                r.include_main,
                r.mode,
                r.height_fit,
                r.height_scroll,
                r.scale,
                r.orderbook,
                r.liquidations,
                r.show_zone,
                r.auto_pin,
                r.orientation,
                r.cancel_pos,
                r.panic_pos,
                r.price_axis_pos,
                r.time_axis_visible,
                r.line_labels,
                r.cursor_labels,
                cx,
            );
        }
    }
}

/// Хозяин попапа ⚙ со стороны полоски вкладок: цель = АКТИВНАЯ вкладка (Main или Add/Custom-стек),
/// ключ персиста — `active_stack_key`. Общая логика попапа/применений — default-методы трейта.
impl LayoutPopupHost for ChartTabs {
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
        self.active_stack_key()
    }
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>) {
        (
            self.active_layout_mode(cx),
            self.active_layout_height_fit(cx),
            self.active_layout_height_scroll(cx),
        )
    }
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation> {
        self.active_layout_orientation(cx)
    }
    fn action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>) {
        self.active_action_btn_pos_opt(cx)
    }
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot {
        let (cancel_pos, panic_pos) = self.active_action_btn_pos(cx);
        LayoutPopupSnapshot {
            mode: self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit),
            orientation: self
                .active_layout_orientation(cx)
                .unwrap_or(StackOrientation::Vertical),
            orderbook: self.active_orderbook_enabled(cx),
            liquidations: self.active_liquidations_enabled(cx),
            show_zone: self.active_show_zone(cx),
            auto_pin: self.active_auto_pin(cx),
            cancel_pos,
            panic_pos,
            price_axis_pos: self.active_price_axis_pos(cx),
            time_axis: self.active_time_axis_visible(cx),
            line_labels: self.active_line_labels(cx),
            cursor_labels: self.active_cursor_labels(cx),
        }
    }
    fn popup_is_custom(&self, _cx: &App) -> bool {
        self.active_is_custom()
    }
    /// Имя кастомной вкладки — для поля переименования в попапе (только Custom).
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Tab::Custom(n, _) = &self.active {
            let name = self.custom_label(*n);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| set_stack_setting!(s, c, v)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| set_stack_setting!(s, c, v));
                }
            }
        }
    }
    /// «Ко всем» из попапа полоски: копируем ВСЕ настройки активной вкладки (+масштаб+стакан+
    /// ориентация) ко всем стекам группы напрямую. `include_main` — попап открыт на Main.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>) {
        let include_main = matches!(self.active, Tab::Main);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let mode = Some(self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit));
        let scale = self.active_scale_value(cx);
        let ob = Some(self.active_orderbook_enabled(cx));
        let liq = Some(self.active_liquidations_enabled(cx));
        let sz = Some(self.active_show_zone(cx));
        let ap = Some(self.active_auto_pin(cx));
        let or = self.active_layout_orientation(cx);
        let (cp, pp) = self.active_action_btn_pos(cx);
        let pax = self.active_price_axis_pos(cx);
        let tax = self.active_time_axis_visible(cx);
        let ll = self.active_line_labels(cx);
        let cl = self.active_cursor_labels(cx);
        self.apply_layout_to_all(
            include_main,
            mode,
            hf,
            hs,
            scale,
            ob,
            liq,
            sz,
            ap,
            or,
            Some(cp),
            Some(pp),
            Some(pax),
            Some(tax),
            Some(ll),
            Some(cl),
            cx,
        );
    }
}
