//! Общая обвязка per-вкладочных настроек чарт-стека для ДВУХ хозяев: полоски вкладок
//! (`ChartTabs`, активная вкладка) и выносного окна (`DetachedChartHost`, его панель).
//! Здесь: значение настройки ([`StackSetting`]: применение к стеку + запись в спек),
//! апсёрт спека, трейт [`LayoutPopupHost`] с общей логикой попапа раскладки ⚙
//! (открыть/засеять/прочитать/закоммитить/закрыть + применение настроек) и общий рендер
//! его оверлея/дисмисса, плюс трейт [`CoinPopupHost`] с обвязкой поля поиска монеты.
//! Различия хозяев (какая вкладка/панель — цель, «применить ко всем», is_custom) остаются
//! в их реализациях трейтов ([`super::settings`] / [`super::detached_host`]).

use gpui::*;
use moon_ui::{MoonInputState, MoonPalette};

use super::{layout_popup, stack};
use crate::Backend;
use crate::chart_persist::{self, ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation};
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// Одно per-вкладочное значение настройки чарт-стека. Умеет записать себя в спек
/// (persist-половина, общая); применение к панелям — макросом [`set_stack_setting!`]
/// (duck-typed: сеттеры одинаково называются у `MainChartStack` и `AddChartStack`).
#[derive(Clone, Copy)]
pub(super) enum StackSetting {
    /// Режим раскладки + раздельные высоты Fit/Scroll.
    Layout(Option<StackLayoutMode>, Option<u16>, Option<u16>),
    /// Ориентация стека (верт/гор).
    Orientation(StackOrientation),
    /// Стакан вкл/выкл.
    Orderbook(bool),
    /// Трейды ликвидаций вкл/выкл.
    Liquidations(bool),
    /// Заливка зоны управления вкл/выкл.
    ShowZone(bool),
    /// Авто-пин при ордере вкл/выкл.
    AutoPin(bool),
    /// Позиции кнопок Cancel Buy / Panic Sell.
    ActionPos(Option<ChartBtnPos>, Option<ChartBtnPos>),
    /// Положение оси цен.
    PriceAxis(PriceAxisPos),
    /// Видимость оси времени.
    TimeAxis(bool),
    /// Подписи у ордер-линий.
    LineLabels(bool),
    /// Подписи у перекрестия.
    CursorLabels(bool),
    /// Настройки отображения свечей/трейдов (попап ❚).
    CandleView(moon_core::market::CandleViewCfg),
}

impl StackSetting {
    /// Записать значение в спек вкладки (persist-часть `apply_*`, общая для обоих хозяев).
    pub(super) fn write_spec(self, s: &mut chart_persist::ChartTabSpec) {
        match self {
            StackSetting::Layout(mode, hf, hs) => {
                s.layout_mode = mode;
                s.layout_height_fit = hf;
                s.layout_height_scroll = hs;
            }
            StackSetting::Orientation(o) => s.layout_orientation = Some(o),
            StackSetting::Orderbook(v) => s.orderbook_enabled = Some(v),
            StackSetting::Liquidations(v) => s.liquidations_enabled = Some(v),
            StackSetting::ShowZone(v) => s.show_zone = Some(v),
            StackSetting::AutoPin(v) => s.auto_pin = Some(v),
            StackSetting::ActionPos(cancel, panic) => {
                s.cancel_buy_pos = cancel;
                s.panic_sell_pos = panic;
            }
            StackSetting::PriceAxis(p) => s.price_axis_pos = Some(p),
            StackSetting::TimeAxis(v) => s.time_axis_visible = Some(v),
            StackSetting::LineLabels(v) => s.line_labels = Some(v),
            StackSetting::CursorLabels(v) => s.cursor_labels = Some(v),
            StackSetting::CandleView(v) => s.candle_view = Some(v),
        }
    }
}

/// Применить [`StackSetting`] к стеку `$s` (внутри `entity.update`): один диспатч на все
/// сеттеры. Макрос (а не функция), чтобы работать и с `MainChartStack`, и с `AddChartStack` —
/// типы разные, но сеттеры совпадают по имени/сигнатуре.
macro_rules! set_stack_setting {
    ($s:expr, $c:expr, $v:expr) => {
        match $v {
            crate::chart_tabs::common::StackSetting::Layout(mode, hf, hs) => {
                $s.set_layout(mode, hf, hs, $c)
            }
            crate::chart_tabs::common::StackSetting::Orientation(o) => {
                $s.set_orientation(Some(o), $c)
            }
            crate::chart_tabs::common::StackSetting::Orderbook(v) => {
                $s.set_orderbook_enabled(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::Liquidations(v) => {
                $s.set_liquidations_enabled(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::ShowZone(v) => $s.set_show_zone(Some(v), $c),
            crate::chart_tabs::common::StackSetting::AutoPin(v) => $s.set_auto_pin(Some(v), $c),
            crate::chart_tabs::common::StackSetting::ActionPos(cancel, panic) => {
                $s.set_action_btn_pos(cancel, panic, $c)
            }
            crate::chart_tabs::common::StackSetting::PriceAxis(p) => {
                $s.set_price_axis_pos(Some(p), $c)
            }
            crate::chart_tabs::common::StackSetting::TimeAxis(v) => {
                $s.set_time_axis_visible(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::LineLabels(v) => {
                $s.set_line_labels(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::CursorLabels(v) => {
                $s.set_cursor_labels(Some(v), $c)
            }
            crate::chart_tabs::common::StackSetting::CandleView(v) => {
                $s.set_candle_view(Some(v), $c)
            }
        }
    };
}
pub(super) use set_stack_setting;

/// Найти/создать спеку вкладки (group/num/bucket), применить мутатор, пометить dirty.
/// Общий апсёрт для полоски вкладок и выносных окон.
pub(super) fn upsert_spec(
    backend: &Entity<Backend>,
    group: &str,
    num: u32,
    bucket: &ChartBucket,
    cx: &mut App,
    f: impl FnOnce(&mut chart_persist::ChartTabSpec),
) {
    let group = group.to_string();
    backend.update(cx, |b, _| {
        chart_persist::upsert(&mut b.chart_specs, &group, num, bucket, f);
        b.chart_specs_dirty = true;
    });
}

/// Снимок текущих настроек вкладки-цели для отрисовки попапа ⚙ (эффективные значения,
/// дефолты уже подставлены).
pub(super) struct LayoutPopupSnapshot {
    pub mode: StackLayoutMode,
    pub orientation: StackOrientation,
    pub orderbook: bool,
    pub liquidations: bool,
    pub show_zone: bool,
    pub auto_pin: bool,
    pub cancel_pos: ChartBtnPos,
    pub panic_pos: ChartBtnPos,
    pub price_axis_pos: PriceAxisPos,
    pub time_axis: bool,
    pub line_labels: bool,
    pub cursor_labels: bool,
}

/// Хозяин попапа настроек раскладки ⚙. Требуемые методы — доступ к состоянию/цели
/// (у `ChartTabs` цель = активная вкладка, у `DetachedChartHost` = панель окна);
/// default-методы — ОБЩАЯ логика попапа и применения настроек (раньше дублировалась).
pub(super) trait LayoutPopupHost: Sized + 'static {
    // --- состояние попапа/инпуты хозяина ---
    fn popup_open(&self) -> bool;
    fn set_popup_open(&mut self, open: bool);
    fn popup_hovered(&self) -> bool;
    fn set_popup_hovered(&mut self, hovered: bool);
    fn fit_input(&self) -> &Entity<MoonInputState>;
    fn scroll_input(&self) -> &Entity<MoonInputState>;
    fn rename_input(&self) -> &Entity<MoonInputState>;

    // --- вкладка-цель ---
    fn backend(&self) -> &Entity<Backend>;
    fn spec_group(&self) -> &str;
    /// Ключ персиста цели: (num, bucket).
    fn spec_key(&self) -> (u32, ChartBucket);
    /// Текущая раскладка цели: (mode, height_fit, height_scroll).
    fn current_layout(&self, cx: &App) -> (Option<StackLayoutMode>, Option<u16>, Option<u16>);
    fn current_orientation(&self, cx: &App) -> Option<StackOrientation>;
    /// Позиции кнопок действий цели (сырые Option — для частичной правки cancel/panic).
    fn action_btn_pos_opt(&self, cx: &App) -> (Option<ChartBtnPos>, Option<ChartBtnPos>);
    fn layout_popup_snapshot(&self, cx: &App) -> LayoutPopupSnapshot;
    /// Цель — кастомная (мульти-монетная) вкладка? (показ поля переименования).
    fn popup_is_custom(&self, cx: &App) -> bool;
    /// Засеять поле имени кастомной вкладки (no-op, если цель не кастомная).
    fn seed_rename_input(&self, window: &mut Window, cx: &mut Context<Self>);
    /// Применить значение к стеку(ам) цели (диспатч Main/активный стек либо панель окна).
    fn set_on_stacks(&mut self, v: StackSetting, cx: &mut Context<Self>);
    /// «Применить ко всем»: у полоски — напрямую ко всем стекам группы, у окна — через Backend.
    fn apply_all_from_popup(&mut self, cx: &mut Context<Self>);

    // --- общая логика (default) ---

    /// Применить настройку к цели + persist в спек (+ пересбор спроса стаканов для Orderbook).
    fn apply_tab_setting(&mut self, v: StackSetting, cx: &mut Context<Self>) {
        self.set_on_stacks(v, cx);
        let (num, bucket) = self.spec_key();
        let backend = self.backend().clone();
        upsert_spec(&backend, self.spec_group(), num, &bucket, cx, move |s| {
            v.write_spec(s)
        });
        if matches!(v, StackSetting::Orderbook(_)) {
            // Пересобрать набор рынков, которым нужен стакан (мог измениться спрос).
            backend.update(cx, |b, _| b.rebuild_orderbook_wanted());
        }
        cx.notify();
    }

    /// Позиция кнопки Cancel Buy + persist (Panic Sell не трогаем).
    fn apply_cancel_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (_, panic) = self.action_btn_pos_opt(cx);
        self.apply_tab_setting(StackSetting::ActionPos(Some(pos), panic), cx);
    }

    /// Позиция кнопки Panic Sell + persist (Cancel Buy не трогаем).
    fn apply_panic_pos(&mut self, pos: ChartBtnPos, cx: &mut Context<Self>) {
        let (cancel, _) = self.action_btn_pos_opt(cx);
        self.apply_tab_setting(StackSetting::ActionPos(cancel, Some(pos)), cx);
    }

    /// Тоггл ориентации: текущая → противоположная.
    fn toggle_orientation_setting(&mut self, cx: &mut Context<Self>) {
        use StackOrientation as O;
        let next = match self.current_orientation(cx).unwrap_or(O::Vertical) {
            O::Vertical => O::Horizontal,
            O::Horizontal => O::Vertical,
        };
        self.apply_tab_setting(StackSetting::Orientation(next), cx);
    }

    /// Открыть/закрыть in-scene popup раскладки цели.
    fn toggle_layout_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup_open() {
            self.close_layout_popup(true, cx);
        } else {
            self.seed_layout_popup_inputs(window, cx);
            self.set_popup_open(true);
            self.set_popup_hovered(false);
            cx.notify();
        }
    }

    /// Засеять поля высоты ЭФФЕКТИВНЫМИ значениями (Fit→0, Scroll→дефолт) — иначе после
    /// рестарта у неустановленных высот поле было бы пустым, без цифр.
    fn seed_layout_popup_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (_, hf, hs) = self.current_layout(cx);
        let fit = hf.unwrap_or(0).to_string();
        let scroll = hs.unwrap_or(stack::DEFAULT_SCROLL_HEIGHT).to_string();
        self.fit_input()
            .clone()
            .update(cx, |input, c| input.set_value(fit, window, c));
        self.scroll_input()
            .clone()
            .update(cx, |input, c| input.set_value(scroll, window, c));
        self.seed_rename_input(window, cx);
    }

    /// Прочитать высоту режима из его поля (пусто → None; мусор → текущее значение цели).
    fn read_layout_height(&self, mode: StackLayoutMode, cx: &App) -> Option<u16> {
        let (_, fit_fallback, scroll_fallback) = self.current_layout(cx);
        let (input, fallback) = match mode {
            StackLayoutMode::Fit => (self.fit_input(), fit_fallback),
            StackLayoutMode::Scroll => (self.scroll_input(), scroll_fallback),
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

    /// Закоммитить содержимое полей попапа в раскладку цели.
    fn commit_layout_popup(&mut self, cx: &mut Context<Self>) {
        let (mode, _, _) = self.current_layout(cx);
        let hf = self.read_layout_height(StackLayoutMode::Fit, cx);
        let hs = self.read_layout_height(StackLayoutMode::Scroll, cx);
        self.apply_tab_setting(
            StackSetting::Layout(Some(mode.unwrap_or(StackLayoutMode::Fit)), hf, hs),
            cx,
        );
    }

    fn close_layout_popup(&mut self, commit: bool, cx: &mut Context<Self>) {
        if !self.popup_open() {
            return;
        }
        if commit {
            self.commit_layout_popup(cx);
        }
        self.set_popup_open(false);
        self.set_popup_hovered(false);
        cx.notify();
    }
}

/// Оверлей попапа раскладки ⚙ (общий рендер обоих хозяев): позиционируемая сцена с
/// hover-закрытием + [`layout_popup::render_layout_popup`] со ВСЕМИ колбэками через трейт.
/// `id_prefix` — "chart-layout" (полоска) / "detached-chart-layout" (окно); `top` — якорь.
/// None, если попап закрыт.
pub(super) fn layout_popup_overlay<T: LayoutPopupHost>(
    this: &T,
    id_prefix: &'static str,
    top: Pixels,
    apply_all_label: String,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.popup_open() {
        return None;
    }
    let p = MoonPalette::active(cx);
    let snap = this.layout_popup_snapshot(cx);
    let is_custom = this.popup_is_custom(cx);
    let entity = cx.entity();
    let popup_w = layout_popup::content_width(cx, is_custom);
    let hover_entity = entity.clone();
    let pick_entity = entity.clone();
    let all_entity = entity.clone();
    let ob_entity = entity.clone();
    let liq_entity = entity.clone();
    let sz_entity = entity.clone();
    let ap_entity = entity.clone();
    let or_entity = entity.clone();
    let cbp_entity = entity.clone();
    let psp_entity = entity.clone();
    let pap_entity = entity.clone();
    let tav_entity = entity.clone();
    let ll_entity = entity.clone();
    let cl_entity = entity;
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-scene")))
            .absolute()
            .right(px(6.0))
            .top(top)
            .w(popup_w)
            .on_mouse_down(MouseButton::Left, |_, _window, app| {
                app.stop_propagation();
            })
            .on_hover(move |hovered, _window, app| {
                hover_entity.update(app, |this, cx| {
                    if *hovered {
                        this.set_popup_hovered(true);
                    } else if this.popup_hovered() {
                        this.close_layout_popup(true, cx);
                    }
                });
            })
            .child(layout_popup::render_layout_popup(
                id_prefix,
                snap.mode,
                snap.orientation,
                is_custom.then_some(this.rename_input()),
                this.fit_input(),
                this.scroll_input(),
                snap.orderbook,
                snap.liquidations,
                snap.show_zone,
                snap.auto_pin,
                snap.cancel_pos,
                snap.panic_pos,
                snap.price_axis_pos,
                snap.time_axis,
                snap.line_labels,
                snap.cursor_labels,
                p,
                cx,
                move |mode, app| {
                    pick_entity.update(app, |this, cx| {
                        let hf = this.read_layout_height(StackLayoutMode::Fit, cx);
                        let hs = this.read_layout_height(StackLayoutMode::Scroll, cx);
                        this.apply_tab_setting(StackSetting::Layout(Some(mode), hf, hs), cx);
                    });
                },
                apply_all_label,
                move |app| {
                    all_entity.update(app, |this, cx| this.apply_all_from_popup(cx));
                },
                move |checked, app| {
                    ob_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::Orderbook(checked), cx)
                    });
                },
                move |checked, app| {
                    liq_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::Liquidations(checked), cx)
                    });
                },
                move |checked, app| {
                    sz_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::ShowZone(checked), cx)
                    });
                },
                move |checked, app| {
                    ap_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::AutoPin(checked), cx)
                    });
                },
                move |app| {
                    or_entity.update(app, |this, cx| this.toggle_orientation_setting(cx));
                },
                move |pos, app| {
                    cbp_entity.update(app, |this, cx| this.apply_cancel_pos(pos, cx));
                },
                move |pos, app| {
                    psp_entity.update(app, |this, cx| this.apply_panic_pos(pos, cx));
                },
                move |pos, app| {
                    pap_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::PriceAxis(pos), cx)
                    });
                },
                move |checked, app| {
                    tav_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::TimeAxis(checked), cx)
                    });
                },
                move |checked, app| {
                    ll_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::LineLabels(checked), cx)
                    });
                },
                move |checked, app| {
                    cl_entity.update(app, |this, cx| {
                        this.apply_tab_setting(StackSetting::CursorLabels(checked), cx)
                    });
                },
            )),
    )
}

/// Слой-перехватчик клика вне попапа раскладки: закрыть с коммитом. None, если попап закрыт.
pub(super) fn layout_popup_dismiss<T: LayoutPopupHost>(
    this: &T,
    id_prefix: &'static str,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.popup_open() {
        return None;
    }
    let entity = cx.entity();
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-dismiss")))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                entity.update(app, |this, cx| this.close_layout_popup(true, cx));
                app.stop_propagation();
            }),
    )
}

/// Хозяин поля поиска монеты (полоска вкладок / шапка выносного окна): куда открывать
/// выбранную монету и как чистить поле/попап. Общая обвязка — [`coin_pick_handler`] /
/// [`coin_dismiss_handler`]; сам рендер списка — [`super::coin_search::render_popup`].
pub(super) trait CoinPopupHost: Sized + 'static {
    /// Очистить поле монеты и закрыть список (после выбора / по клику вне).
    fn clear_coin_search(&mut self, cx: &mut Context<Self>);
    /// Открыть выбранную монету на цели хозяина (активная вкладка / стек окна).
    fn open_picked_coin(&mut self, core: CoreId, market: String, cx: &mut Context<Self>);
}

/// Обработчик выбора монеты из списка: открыть → очистить поле → закрыть попап.
pub(super) fn coin_pick_handler<T: CoinPopupHost>(
    cx: &Context<T>,
    input: Entity<MoonInputState>,
) -> impl Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static {
    // ВАЖНО: НЕ читаем `cx.entity().read(cx)` здесь — этот хелпер вызывается во ВРЕМЯ
    // рендера хоста (ChartTabs/DetachedChartHost), который уже занят как `&mut self` →
    // `read` даёт панику «cannot read … while it is already being updated» (краш при
    // открытии поиска монет). `coin_input` берём параметром у вызывающего (у него `&self`).
    let view = cx.entity();
    move |core, market, window, app| {
        view.update(app, |this, cx| this.open_picked_coin(core, market, cx));
        input.update(app, |inp, c| {
            inp.set_value(SharedString::default(), window, c)
        });
        view.update(app, |this, cx| this.clear_coin_search(cx));
    }
}

/// Обработчик клика по слою-дисмиссеру списка монеты (геометрию слоя задаёт вызывающий).
pub(super) fn coin_dismiss_handler<T: CoinPopupHost>(
    cx: &Context<T>,
) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    let entity = cx.entity();
    move |_, _w, app| {
        entity.update(app, |this, cx| this.clear_coin_search(cx));
        app.stop_propagation();
    }
}
