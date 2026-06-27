//! `ChartTabs`: УПРАВЛЕНИЕ per-вкладочными настройками (контроллер). Открытие/засев/коммит/
//! закрытие попапа ⚙, геттеры текущих настроек активной вкладки (`active_*`: раскладка/
//! ориентация/стакан/зона/авто-пин/масштаб) и их применение к активной вкладке и ко всем
//! стекам/окнам группы (`apply_*`). НЕ путать с [`super::layout_popup`] — там ОТРИСОВКА самого
//! попапа (свободные функции), здесь — логика `ChartTabs` за ним. Вынесено из `mod.rs`.

use gpui::*;

use super::{AddChartStack, ChartTabs, Tab, layout_popup, stack};
use crate::chart_persist::{ChartBtnPos, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;

impl ChartTabs {
    /// Открыть/закрыть in-scene popup настроек раскладки активной вкладки.
    pub(super) fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout_popup_open {
            self.close_layout_popup(true, cx);
        } else {
            self.seed_layout_popup_inputs(window, cx);
            self.layout_popup_open = true;
            self.layout_popup_hovered = false;
            cx.notify();
        }
    }

    pub(super) fn seed_layout_popup_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        // Показываем ЭФФЕКТИВНЫЕ значения (а не пусто при None): Fit→0 (растянуть), Scroll→дефолт.
        // Иначе после рестарта у неустановленных высот поле было пустым, без цифр.
        let fit = self.active_layout_height_fit(cx).unwrap_or(0).to_string();
        let scroll = self
            .active_layout_height_scroll(cx)
            .unwrap_or(stack::DEFAULT_SCROLL_HEIGHT)
            .to_string();
        self.layout_fit_input
            .update(cx, |input, c| input.set_value(fit, window, c));
        self.layout_scroll_input
            .update(cx, |input, c| input.set_value(scroll, window, c));
        // Имя кастомной вкладки — для поля переименования в попапе.
        if let Tab::Custom(n, _) = &self.active {
            let name = self.custom_label(*n);
            self.custom_name_input
                .update(cx, |input, c| input.set_value(name, window, c));
        }
    }

    pub(super) fn read_layout_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (input, fallback) = match mode {
            StackLayoutMode::Fit => (&self.layout_fit_input, self.active_layout_height_fit(cx)),
            StackLayoutMode::Scroll => (
                &self.layout_scroll_input,
                self.active_layout_height_scroll(cx),
            ),
        };
        let value = input.read(cx).value().to_string();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        trimmed
            .parse::<u16>()
            .ok()
            .map(|raw| layout_popup::clamp_height(mode, raw))
            .or(fallback)
    }

    pub(super) fn commit_layout_popup(&mut self, cx: &mut Context<Self>) {
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        let mode = Some(self.active_layout_mode(cx).unwrap_or(StackLayoutMode::Fit));
        self.apply_layout(mode, hf, hs, cx);
    }

    pub(super) fn close_layout_popup(&mut self, commit: bool, cx: &mut Context<Self>) {
        if !self.layout_popup_open {
            return;
        }
        if commit {
            self.commit_layout_popup(cx);
        }
        self.layout_popup_open = false;
        self.layout_popup_hovered = false;
        cx.notify();
    }

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

    /// Позиция кнопки Cancel Buy на активной вкладке + persist (Panic Sell не трогаем).
    pub(super) fn apply_cancel_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (_, panic) = self.active_action_btn_pos_opt(cx);
        self.apply_action_pos(Some(pos), panic, cx);
    }

    /// Позиция кнопки Panic Sell на активной вкладке + persist (Cancel Buy не трогаем).
    pub(super) fn apply_panic_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (cancel, _) = self.active_action_btn_pos_opt(cx);
        self.apply_action_pos(cancel, Some(pos), cx);
    }

    fn apply_action_pos(
        &mut self,
        cancel: Option<ChartBtnPos>,
        panic: Option<ChartBtnPos>,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_action_btn_pos(cancel, panic, c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_action_btn_pos(cancel, panic, c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.cancel_buy_pos = cancel;
            s.panic_sell_pos = panic;
        });
        cx.notify();
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

    /// Положение оси цен на АКТИВНОЙ вкладке + persist.
    pub(super) fn apply_price_axis_pos(
        &mut self,
        pos: crate::chart_persist::PriceAxisPos,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| s.set_price_axis_pos(Some(pos), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_price_axis_pos(Some(pos), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.price_axis_pos = Some(pos);
        });
        cx.notify();
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

    /// Видимость оси времени на АКТИВНОЙ вкладке + persist.
    pub(super) fn apply_time_axis_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_time_axis_visible(Some(visible), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_time_axis_visible(Some(visible), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.time_axis_visible = Some(visible);
        });
        cx.notify();
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

    /// Вкл/выкл стакан на АКТИВНОЙ вкладке + persist.
    pub(super) fn apply_orderbook(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_orderbook_enabled(Some(enabled), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_orderbook_enabled(Some(enabled), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.orderbook_enabled = Some(enabled);
        });
        // Stage 2: пересобрать набор рынков, которым нужен стакан (мог измениться спрос).
        self.backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        cx.notify();
    }

    /// Вкл/выкл заливку зоны управления на АКТИВНОЙ вкладке + persist.
    pub(super) fn apply_show_zone(&mut self, show: bool, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| s.set_show_zone(Some(show), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_show_zone(Some(show), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.show_zone = Some(show);
        });
        cx.notify();
    }

    /// Вкл/выкл авто-пин при ордере на АКТИВНОЙ вкладке + persist.
    pub(super) fn apply_auto_pin(&mut self, on: bool, cx: &mut Context<Self>) {
        match self.active.clone() {
            Tab::Main => self.main.update(cx, |s, c| s.set_auto_pin(Some(on), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_auto_pin(Some(on), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.auto_pin = Some(on);
        });
        cx.notify();
    }

    /// Сменить ориентацию (верт/гор) на АКТИВНОЙ вкладке + persist. Тоггл из попапа ⚙.
    pub(super) fn apply_orientation(
        &mut self,
        orientation: StackOrientation,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_orientation(Some(orientation), c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_orientation(Some(orientation), c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.layout_orientation = Some(orientation);
        });
        cx.notify();
    }

    /// Применить раскладку (режим + раздельные высоты Fit/Scroll) к АКТИВНОЙ вкладке и
    /// сохранить в charts.json.
    pub(super) fn apply_layout(
        &mut self,
        mode: Option<StackLayoutMode>,
        height_fit: Option<u16>,
        height_scroll: Option<u16>,
        cx: &mut Context<Self>,
    ) {
        match self.active.clone() {
            Tab::Main => self
                .main
                .update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c)),
            Tab::Add(..) | Tab::Custom(..) => {
                if let Some(p) = self.active_stack() {
                    p.update(cx, |s, c| s.set_layout(mode, height_fit, height_scroll, c));
                }
            }
        }
        let (num, bucket) = self.active_stack_key();
        self.upsert_spec(cx, num, &bucket, move |s| {
            s.layout_mode = mode;
            s.layout_height_fit = height_fit;
            s.layout_height_scroll = height_scroll;
        });
        cx.notify();
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
        show_zone: Option<bool>,
        auto_pin: Option<bool>,
        orientation: Option<StackOrientation>,
        cancel_pos: Option<ChartBtnPos>,
        panic_pos: Option<ChartBtnPos>,
        price_axis_pos: Option<crate::chart_persist::PriceAxisPos>,
        time_axis_visible: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let ob = orderbook.unwrap_or(true);
        let sz = show_zone.unwrap_or(true);
        let ap = auto_pin.unwrap_or(false);
        let axis = price_axis_pos.unwrap_or_default();
        let time_axis = time_axis_visible.unwrap_or(true);
        if include_main {
            self.main.update(cx, |s, c| {
                s.set_layout(mode, height_fit, height_scroll, c);
                s.set_scale(scale, c);
                s.set_orderbook_enabled(Some(ob), c);
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
            });
            self.upsert_spec(cx, 0, &ChartBucket::Shared, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
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
                s.set_show_zone(Some(sz), c);
                s.set_auto_pin(Some(ap), c);
                s.set_orientation(orientation, c);
                s.set_action_btn_pos(cancel_pos, panic_pos, c);
                s.set_price_axis_pos(Some(axis), c);
                s.set_time_axis_visible(Some(time_axis), c);
            });
            self.upsert_spec(cx, num, &bucket, |s| {
                s.layout_mode = mode;
                s.layout_height_fit = height_fit;
                s.layout_height_scroll = height_scroll;
                s.scale = scale;
                s.orderbook_enabled = Some(ob);
                s.show_zone = Some(sz);
                s.auto_pin = Some(ap);
                s.layout_orientation = orientation;
                s.cancel_buy_pos = cancel_pos;
                s.panic_sell_pos = panic_pos;
                s.price_axis_pos = Some(axis);
                s.time_axis_visible = Some(time_axis);
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
                r.show_zone,
                r.auto_pin,
                r.orientation,
                r.cancel_pos,
                r.panic_pos,
                r.price_axis_pos,
                r.time_axis_visible,
                cx,
            );
        }
    }
}
