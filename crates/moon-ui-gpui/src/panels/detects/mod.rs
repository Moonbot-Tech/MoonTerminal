//! Лента детектов — откпрепляемая панель (порт egui `DetectRibbon`). Втягивает детекты
//! ядер группы с `SoundAlert=Yes` и `AddToChart==0` (AddToChart-детекты — в чарт-вкладки),
//! держит `KeepAlert` секунд, новые сверху. Клик → открыть монету на Main (через
//! `Backend.open_request`, который читает Shell).
//!
//! Отображение настраивается попапом-конструктором ⚙ (габариты/график/rail/слоты по
//! размерам) — per-group, persist в отдельном `detects_view.toml` (см. [`popup`]).
//! Раскладка карточек и векторные мини-чарты (замороженный срез на момент детекта) —
//! в [`cards`].

mod cards;
mod popup;

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use crate::Backend;
use gpui::*;
use moon_chart::paint::now_unix_ms;
use moon_core::config::{DETECT_RAIL_MAX, DetectViewCfg};
use moon_core::session::CoreId;
use moon_ui::{
    MoonPalette, MoonSliderEvent, MoonSliderState, Panel, PanelEvent, PanelState, h_flex, v_flex,
};

/// Сколько закрытых 5м-свечей морозим для мини-чарта карточки (≈2ч истории). Под ПОЛЫЕ свечи
/// при типовой ширине зоны ~75-130px: ~24 свечи → 3-5px на свечу (тело 1px контур + просвет),
/// иначе «полое» схлопывается в закрашенное. Меньше свечей = шире и читаемее.
const DETECT_THUMB_BARS: usize = 24;

/// Кнопка ленты детектов (порт `src/dock/detects.rs::RibbonItem`).
pub(crate) struct DetectItem {
    core: CoreId,
    core_name: String,
    /// Полный символ рынка (для подписки при клике).
    market: String,
    /// Подпись кнопки — монета без quote подключения (`ADAUSDT` → `ADA`).
    base: String,
    color: [u8; 3],
    /// Ordinal вида стратегии-источника (`DetectRow.kind`) — для бейджа типа детекта.
    kind: u8,
    /// Направление стратегии-источника (`DetectRow.is_short`) — для обводки бейджа.
    is_short: bool,
    born_ms: f64,
    ttl_ms: f64,
    /// Замороженный на момент детекта срез 5м-свечей `(open, high, low, close)` для мини-чарта.
    bars: Vec<(f32, f32, f32, f32)>,
    /// Цены (close) 24ч для режима «линия» (старые→новые).
    line: Vec<f32>,
    /// Дельты цены 24ч/1ч, % (на момент детекта).
    delta_24h: f32,
    delta_1h: f32,
    /// Имя биржи (Binance Futures/Bybit/…) на момент детекта.
    exchange_name: String,
    /// Тип биржи (Спот/Фьючи/DEX/…) на момент детекта.
    exchange_kind: String,
}

pub struct DetectsPanel {
    backend: Entity<Backend>,
    group: String,
    items: VecDeque<DetectItem>,
    last_seq: HashMap<CoreId, u64>,
    last_sig: u64,
    prune_timer_armed: bool,
    focus: FocusHandle,
    /// Попап-конструктор отображения (⚙): открыт ли и какой РАЗМЕР редактируется
    /// (вкладка = активный размер ленты).
    popup_open: bool,
    popup_tab: u8,
    /// Слайдеры попапа (ширина/высота карточки, полоса сервера, градиент) — сидируются
    /// из конфига вкладки при открытии/смене вкладки, Change пишет обратно в конфиг.
    w_slider: Entity<MoonSliderState>,
    h_slider: Entity<MoonSliderState>,
    rail_slider: Entity<MoonSliderState>,
    grad_slider: Entity<MoonSliderState>,
}

const MAX_DETECT_BTNS: usize = 48;
const DEFAULT_SERVER_COLOR: [u8; 3] = [0xff, 0xb3, 0x47];

impl DetectsPanel {
    pub fn new(backend: Entity<Backend>, group: String, cx: &mut Context<Self>) -> Self {
        let initial_sig = detects_sig(backend.read(cx), &group);
        cx.observe(&backend, |this, backend, cx| {
            let now = now_unix_ms();
            let sig = detects_sig(backend.read(cx), &this.group);
            let mut changed = false;
            if sig != this.last_sig {
                this.last_sig = sig;
                changed |= this.ingest(backend.read(cx));
            }
            changed |= this.prune(now);
            this.arm_prune_timer(cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
        let initial_backend = backend.clone();
        let mk_slider = |cx: &mut Context<Self>, min: f32, max: f32, step: f32| {
            cx.new(|_| MoonSliderState::new().min(min).max(max).step(step))
        };
        let w_slider = mk_slider(cx, 20.0, 320.0, 2.0);
        let h_slider = mk_slider(cx, 20.0, 320.0, 2.0);
        let rail_slider = mk_slider(cx, 0.0, f32::from(DETECT_RAIL_MAX), 1.0);
        // Градиент: шкала до максимума ширины; фактический потолок = ширина карточки
        // (клампится в конфиге, подпись показывает уже клампнутое значение).
        let grad_slider = mk_slider(cx, 0.0, 320.0, 2.0);
        // Change слайдеров → записать в конфиг РЕДАКТИРУЕМОЙ вкладки (гард от эха сидинга).
        for (sl, apply) in [
            (
                &w_slider,
                (|c: &mut moon_core::config::DetectSizeCfg, v: f32| c.w = v.round() as u16)
                    as fn(&mut moon_core::config::DetectSizeCfg, f32),
            ),
            (&h_slider, |c, v| c.h = v.round() as u16),
            (&rail_slider, |c, v| c.rail_w = v.round() as u8),
            (&grad_slider, |c, v| c.rail_grad = v.round() as u16),
        ] {
            cx.subscribe(sl, move |this, _, ev: &MoonSliderEvent, cx| {
                if let MoonSliderEvent::Change(v) = ev {
                    let v = v.end();
                    let tab = this.popup_tab;
                    this.write_view(cx, |cfg| {
                        apply(cfg.size_cfg_mut(tab), v);
                    });
                }
            })
            .detach();
        }
        let mut this = Self {
            backend,
            group,
            items: VecDeque::new(),
            last_seq: HashMap::new(),
            last_sig: initial_sig,
            prune_timer_armed: false,
            focus: cx.focus_handle(),
            popup_open: false,
            popup_tab: 0,
            w_slider,
            h_slider,
            rail_slider,
            grad_slider,
        };
        this.ingest(initial_backend.read(cx));
        this.prune(now_unix_ms());
        this.arm_prune_timer(cx);
        this
    }

    /// Правка конфига отображения группы: текущее → мутатор → persist в detects_view.toml.
    /// Ничего не пишет, если мутатор не изменил конфиг (эхо сидинга слайдеров).
    fn write_view(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut DetectViewCfg)) {
        let group = self.group.clone();
        let changed = self.backend.update(cx, |b, bcx| {
            let mut cfg = b.detects_view.group(&group);
            let before = cfg;
            f(&mut cfg);
            if cfg == before {
                return false;
            }
            b.detects_view.set_group(&group, cfg);
            b.detects_view.save();
            bcx.notify();
            true
        });
        if changed {
            cx.notify();
        }
    }

    /// Втянуть свежие детекты ядер группы (seq > курсора, sound_alert, не AddToChart).
    fn ingest(&mut self, b: &Backend) -> bool {
        let mut changed = false;
        // Цвет + quote ядра берём из его сервера в конфиге: quote выводим из рынка по
        // умолчанию (`server.market`), чтобы резать суффикс монеты (`ADAUSDT` → `ADA`),
        // как egui `CoreInfo.quote`.
        // Canonical order: fresh events are appended core by core and rendered back in
        // reverse insertion order, with no chronological re-sort anywhere — so this order is
        // what decides how detects of the same instant read on screen.
        let order = crate::core_order::CoreOrder::new(&b.config);
        let mut cores: Vec<(CoreId, String, [u8; 3], String)> = b
            .session
            .sessions()
            .iter()
            .filter(|s| s.group == self.group)
            .map(|s| {
                let (color, quote) = b
                    .config
                    .servers
                    .iter()
                    .find(|sv| sv.id == s.id)
                    .map(|sv| (sv.color, moon_core::symbol::resolve_quote(&sv.market)))
                    .unwrap_or((DEFAULT_SERVER_COLOR, String::new()));
                (s.id, s.name.clone(), color, quote)
            })
            .collect();
        order.sort_by(&mut cores, |(id, _, _, _)| *id);
        for (id, name, color, _quote) in cores {
            let Some(d) = b.session.store().core(id) else {
                continue;
            };
            let last = self.last_seq.get(&id).copied().unwrap_or(0);
            let mut fresh: Vec<&moon_core::feed::DetectRow> = Vec::new();
            for det in d.detects.iter().rev() {
                if det.seq <= last {
                    break;
                }
                fresh.push(det);
            }
            if fresh.is_empty() {
                continue;
            }
            self.last_seq.insert(id, fresh[0].seq);
            for det in fresh.iter().rev() {
                // Показываем детекты со звук-алертом И срабатывания алертов (даже без
                // стратегии); AddToChart-детекты уходят в чарт-вкладки, не сюда.
                if (!det.sound_alert && !det.is_alert) || det.add_to_chart > 0 {
                    continue;
                }
                let ttl = (det.keep_alert_secs.max(1) as f64) * 1000.0;
                // Замороженный снимок на момент детекта: мини-чарт (5м H/L) + имя/тип биржи.
                // Данные бесплатны (собственная запись ядра, не биржевой API).
                let snap =
                    b.session
                        .market_source()
                        .detect_snapshot(id, &det.market, DETECT_THUMB_BARS);
                if let Some(it) = self
                    .items
                    .iter_mut()
                    .find(|it| it.core == id && it.market == det.market)
                {
                    it.born_ms = det.time_ms;
                    it.ttl_ms = ttl;
                    it.color = color;
                    it.kind = det.kind;
                    it.is_short = det.is_short;
                    // Пере-морозка на свежее срабатывание той же монеты.
                    it.bars = snap.bars;
                    it.line = snap.line;
                    it.delta_24h = snap.delta_24h;
                    it.delta_1h = snap.delta_1h;
                    it.exchange_name = snap.exchange_name;
                    it.exchange_kind = snap.exchange_kind;
                    changed = true;
                } else {
                    self.items.push_back(DetectItem {
                        core: id,
                        core_name: name.clone(),
                        market: det.market.clone(),
                        base: moon_core::symbol::coin_of_market(&det.market).to_string(),
                        color,
                        kind: det.kind,
                        is_short: det.is_short,
                        born_ms: det.time_ms,
                        ttl_ms: ttl,
                        bars: snap.bars,
                        line: snap.line,
                        delta_24h: snap.delta_24h,
                        delta_1h: snap.delta_1h,
                        exchange_name: snap.exchange_name,
                        exchange_kind: snap.exchange_kind,
                    });
                    changed = true;
                }
            }
        }
        while self.items.len() > MAX_DETECT_BTNS {
            self.items.pop_front();
            changed = true;
        }
        changed
    }

    fn prune(&mut self, now_ms: f64) -> bool {
        let before = self.items.len();
        self.items.retain(|it| now_ms - it.born_ms < it.ttl_ms);
        self.items.len() != before
    }

    /// 1-Гц тик ПОКА есть детекты: обновляет обратный отсчёт («Ns») по СВОИМ часам и
    /// убирает истёкшие. Раньше отсчёт перерисовывался только по приходу данных (backend-
    /// пульс) — это «время сцеплено с данными»: на тихой/отключённой группе цифры замирали,
    /// а кнопка просто исчезала в конце. Время — по часам, не по приходу тиков. Тик сам
    /// гаснет, когда детектов нет (не плодим таймер вхолостую — executor.timer недёшев).
    fn arm_prune_timer(&mut self, cx: &mut Context<Self>) {
        if self.prune_timer_armed || self.items.is_empty() {
            return;
        }
        self.prune_timer_armed = true;
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            executor.timer(Duration::from_millis(1000)).await;
            let alive = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.prune_timer_armed = false;
                    this.prune(now_unix_ms());
                    // Перерисовать отсчёт / отразить пропажу истёкших, пока есть что показывать.
                    if !this.items.is_empty() {
                        cx.notify();
                    }
                    this.arm_prune_timer(cx);
                })
                .is_ok()
            });
            if !alive {
                return;
            }
        })
        .detach();
    }

    /// Открыть монету на Main: запрос в Backend (Shell откроет чарт) + убрать кнопку.
    fn open(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.items
            .retain(|it| !(it.core == core && it.market == market));
        self.backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market.clone()));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            // Клик по детекту открывает монету на Main, но окно НЕ поднимает.
            b.open_request_activate = false;
            bcx.notify();
        });
        self.arm_prune_timer(cx);
        cx.notify();
    }

    /// ПКМ: открыть монету в НОВОЙ кастомной вкладке в режиме сравнения — якорь = монета
    /// детекта + та же монета (точное имя) с других ядер группы без повторов по бирже,
    /// замок+метла. Запрос в Backend, читает ChartTabs группы (`open_compare_tab`).
    fn open_compare(&mut self, core: CoreId, market: String, cx: &mut Context<Self>) {
        self.items
            .retain(|it| !(it.core == core && it.market == market));
        self.backend.update(cx, |b, bcx| {
            b.open_compare_request = Some((core, market.clone()));
            b.open_compare_request_rev = b.open_compare_request_rev.wrapping_add(1);
            bcx.notify();
        });
        self.arm_prune_timer(cx);
        cx.notify();
    }

    /// Текущая настройка отображения группы (или дефолт) — из detects_view.toml.
    fn view_cfg(&self, cx: &App) -> DetectViewCfg {
        self.backend.read(cx).detects_view.group(&self.group)
    }
}

fn detects_sig(b: &Backend, group: &str) -> u64 {
    let store = b.session.store();
    b.session
        .sessions()
        .iter()
        .filter(|s| s.group == group)
        .filter_map(|s| store.core(s.id))
        .fold(0u64, |a, c| a.wrapping_mul(31).wrapping_add(c.detects_rev))
}

impl EventEmitter<PanelEvent> for DetectsPanel {}
impl Focusable for DetectsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Panel for DetectsPanel {
    fn panel_name(&self) -> &'static str {
        "Detects"
    }
    /// Visible tab caption. `panel_name` is the stable persistence key and stays untouched.
    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        crate::panel_meta::tab_label(self.panel_name())
    }
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::panel_meta::panel_title(self.panel_name())
    }
    fn dump(&self, _cx: &App) -> PanelState {
        crate::dock_persist::panel_state_with_group("Detects", &self.group)
    }
}

impl Render for DetectsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = MoonPalette::active(cx);
        let is_light = p.is_light();
        let cfg = self.view_cfg(cx);
        // Настройки бейджей (код/цвет по видам + обводка направления) — из конфига,
        // под активную тему. Клон дешёвый (≤пара десятков записей), лента редко рендерится.
        let badges = self.backend.read(cx).config.badges.clone();
        // Тема чарта — цвета векторных свечей/линии карточек.
        let theme = self.backend.read(cx).config.chart_theme().clone();
        let now = now_unix_ms();

        // Полоска-настройки сверху (⚙ = триггер попапа-конструктора) + разделитель.
        let toolbar = popup::toolbar(self, &cfg, p, cx);
        let divider = div().w_full().h(px(1.0)).flex_none().bg(rgb(p.border));

        // Карточки: новые сверху, кнопки фикс-ширины из конфига — сетка с переносом.
        let mut container = h_flex().flex_wrap().gap_1p5().content_start();
        for (i, it) in self.items.iter().enumerate().rev() {
            let secs = ((it.ttl_ms - (now - it.born_ms)) / 1000.0).ceil().max(0.0) as u32;
            let (core, market) = (it.core, it.market.clone());
            let market_rmb = it.market.clone();
            let card = cards::card(it, secs, &cfg, &theme, &badges, p, is_light, cx)
                .id(SharedString::from(format!("det-{i}")))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open(core, market.clone(), cx);
                }))
                // ПКМ — открыть в новой кастомной вкладке в режиме сравнения (замок+метла).
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        this.open_compare(core, market_rmb.clone(), cx);
                        cx.stop_propagation();
                    }),
                );
            container = container.child(card);
        }

        let scroll = div()
            .id("detects-scroll")
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_2()
            .child(container);

        v_flex()
            .id("detects")
            .relative()
            .size_full()
            .min_h(px(0.0))
            .track_focus(&self.focus)
            .bg(rgb(p.table_body))
            .child(toolbar)
            .child(divider)
            .child(scroll)
    }
}
