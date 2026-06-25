//! Торговый тулбар: прикладная сборка терминала поверх MoonPalette.
//!
//! Логика остаётся терминальной: size/sell пока логируют todo, scale/live пишут в
//! `Backend`. Визуальные контролы берём из палитры, выведенной из HTML-эталона.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSegment, MoonButtonSize, MoonButtonVariant, MoonCheckbox,
    MoonCheckboxSize, MoonDropdown, MoonInput, MoonInputState, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, MoonSlider, MoonSliderState,
    MoonTooltipView, h_flex, v_flex,
};

use moon_core::feed::ClientSettingsEdit;
use moon_core::session::CoreId;

use crate::shell::Shell;
use crate::{Backend, design};

/// Границы слайдеров торговых метрик `(min, max, step)` (по смыслу ядра). Использует и
/// `Shell` при создании состояний слайдеров.
pub const TP_NORMAL: (f32, f32, f32) = (2.0, 100.0, 1.0); // x_tmode off: 2..100% (мин = 2)
/// Граница, ниже которой работает файн-слайдер (суб-процент через scalp). Верхний TP на 2
/// = на минимуме → нижний слайдер активен (0..2). Выше — нижний disabled.
pub const TP_FINE_MAX: f32 = 2.0;
pub const TP_EXT: (f32, f32, f32) = (100.0, 900.0, 10.0); // x_tmode on («s9»): 100..900%
pub const SL_BOUNDS: (f32, f32, f32) = (-20.0, 1.0, 0.01); // знаковый: -20..+1%
pub const LEV_BOUNDS: (f32, f32, f32) = (1.0, 125.0, 1.0);

/// Формат значения с сотыми, точка-разделитель: `50` → "50.00".
pub fn fmt_field2(v: f32) -> String {
    format!("{v:.2}")
}

/// Со знаком (для SL, который может быть и +, и −): `1` → "+1.00", `-20` → "-20.00".
pub fn fmt_field2_signed(v: f32) -> String {
    format!("{v:+.2}")
}

/// Торговая метрика тулбара с собственным попапом (слайдер + поле ввода).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TradeMetric {
    Tp,
    Sl,
    Lev,
}

impl TradeMetric {
    fn id(self) -> &'static str {
        match self {
            TradeMetric::Tp => "toolbar-tp",
            TradeMetric::Sl => "toolbar-sl",
            TradeMetric::Lev => "toolbar-lev",
        }
    }

    fn label(self) -> &'static str {
        match self {
            TradeMetric::Tp => "TP",
            TradeMetric::Sl => "SL",
            TradeMetric::Lev => "Lev",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            TradeMetric::Lev => "×",
            _ => "%",
        }
    }

    fn title(self) -> String {
        match self {
            TradeMetric::Tp => t!("toolbar.tp_title").to_string(),
            TradeMetric::Sl => t!("toolbar.sl_title").to_string(),
            TradeMetric::Lev => t!("toolbar.lev_title").to_string(),
        }
    }

    /// Текущее значение метрики активного ядра (для сидирования слайдера/инпута при открытии).
    /// Lev зависит ОТ ЯДРА И ТЕКУЩЕЙ МОНЕТЫ: плечо рынка main-чарта из ассетов активного ядра.
    pub fn current(self, b: &Backend, group: &str) -> Option<f32> {
        let core = b.active_trade_core(group)?;
        let cd = b.session.store().core(core)?;
        match self {
            TradeMetric::Tp => cd
                .client_settings
                .as_ref()
                .map(|s| s.take_profit_pct as f32),
            TradeMetric::Sl => cd.client_settings.as_ref().map(|s| s.stop_loss_pct),
            TradeMetric::Lev => {
                // Плечо монеты main-чарта из per-core карты (любой отслеживаемый рынок, не
                // только с позицией). Нет в карте → плечо неизвестно (покажем «—»).
                let (_, market) = b.main_chart_target(group)?;
                cd.assets.leverage.get(&market).map(|l| *l as f32)
            }
        }
    }
}

/// Высота полосы тулбара: 2-я строка header из HTML-эталона.
pub const TOOLBAR_H: f32 = design::TOOLBAR_H;

/// Пресеты масштаба цены (Y) — 1:1 с egui `dock/controls.rs::SCALES`. `None` = «Авто».
const SCALES: [(&str, Option<f32>); 6] = [
    ("Авто", None),
    ("50%", Some(0.50)),
    ("20%", Some(0.20)),
    ("10%", Some(0.10)),
    ("5%", Some(0.05)),
    ("2%", Some(0.02)),
];

/// Кнопка-триггер торговой метрики. Клик открывает/закрывает её попап в `Shell`
/// (overlay со слайдером/полем; закрытие — как у попапа раскладки чарта).
fn metric_button(
    metric: TradeMetric,
    value_str: String,
    color: u32,
    width: f32,
    open: bool,
    shell: Entity<Shell>,
    p: MoonPalette,
) -> impl IntoElement {
    MoonButton::new(metric.id())
        .width(width)
        .variant(if open {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Neutral
        })
        .size(MoonButtonSize::Toolbar)
        .selected(open)
        .segment(
            MoonButtonSegment::new(metric.label())
                .color(p.text_muted)
                .weight(400.0),
        )
        .text_segment(value_str, color, 500.0)
        .on_click(move |_, window, app| {
            shell.update(app, |this, cx| this.toggle_metric_popup(metric, window, cx));
        })
        .render()
}

/// Контент попапа метрики (overlay-бокс со своим фоном/рамкой): заголовок + слайдер + поле;
/// для TP — ещё чекбокс расширенного диапазона `x_tmode`/«s9». Рисуется `Shell` поверх дока
/// на абсолютной позиции под кнопкой. `slider` уже выбран вызывающим (для TP — обычный/
/// расширенный по `extended`).
#[allow(clippy::too_many_arguments)]
pub fn metric_popup_content(
    metric: TradeMetric,
    slider: &Entity<MoonSliderState>,
    fine_slider: &Entity<MoonSliderState>,
    input: &Entity<MoonInputState>,
    extended: bool,
    hedge_on: bool,
    backend: &Entity<Backend>,
    group: &str,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let mut content = v_flex()
        .w(px(220.0))
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(metric.title()),
        )
        .child(
            MoonSlider::new(slider)
                .id(format!("{}-slider", metric.id()))
                .height(18.0),
        )
        .child(
            h_flex()
                .gap(design::ui_px(cx, 6.0))
                .items_center()
                .child(
                    div().w(px(72.0)).child(
                        MoonInput::new(SharedString::from(format!("{}-input", metric.id())))
                            .state(input)
                            .small(),
                    ),
                )
                .child(div().text_color(rgb(p.text_muted)).child(metric.unit())),
        );

    if matches!(metric, TradeMetric::Tp) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-tp-ext")
                .label(t!("toolbar.tp_ext").to_string())
                .checked(extended)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let ext = *ch;
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    let cur = b
                        .session
                        .store()
                        .core(core)
                        .and_then(|d| d.client_settings.as_ref())
                        .map(|s| s.take_profit_pct)
                        .unwrap_or(0.0);
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::TakeProfit {
                            pct: cur,
                            extended: ext,
                        },
                    ) {
                        log::warn!("tp extended toggle failed: {error}");
                    }
                }),
        );
        // Файн-слайдер: суб-процентный TP (0..2, шаг 0.01) через scalp. Активен ТОЛЬКО когда
        // верхний TP на минимуме (=2, без галки ×10); поднял верхний выше 2 — нижний disabled.
        let coarse_tp = slider.read(cx).value().end();
        let fine_enabled = !extended && coarse_tp <= TP_FINE_MAX + 0.001;
        content = content
            .child(
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .opacity(if fine_enabled { 1.0 } else { 0.4 })
                    .child(t!("toolbar.tp_fine").to_string()),
            )
            .child(
                MoonSlider::new(fine_slider)
                    .id("toolbar-tp-fine-slider")
                    .disabled(!fine_enabled)
                    .height(18.0),
            );
    }

    if matches!(metric, TradeMetric::Lev) {
        let backend = backend.clone();
        let group = group.to_string();
        content = content.child(
            MoonCheckbox::new("toolbar-hedge")
                .label(t!("toolbar.hedge").to_string())
                .checked(hedge_on)
                .size(MoonCheckboxSize::Compact)
                .on_change(move |ch: &bool, _w, app| {
                    let on = *ch;
                    let b = backend.read(app);
                    let Some(core) = b.active_trade_core(&group) else {
                        return;
                    };
                    if let Err(error) = b.session.set_hedge_mode(core, on) {
                        log::warn!("set hedge mode failed: {error}");
                    }
                }),
        );
    }
    content.into_any_element()
}

/// Мелкая тусклая подпись группы (`size`/`sell`/`МАСШТАБ`) — стендовый `.strip-label`.
fn strip_label(text: &'static str, p: MoonPalette, cx: &App) -> impl IntoElement {
    div()
        .text_size(design::t_caption(cx))
        .font_family(design::ui_font())
        .text_color(rgb(p.text_muted))
        .child(text)
}

/// Вертикальный разделитель групп (стендовый `.divider`): тонкая линия высотой 16px.
fn divider(p: MoonPalette) -> impl IntoElement {
    design::vline(16.0, p)
}

/// Ширины кнопок размера — единая 62 на все слоты.
const SIZE_W: [f32; 6] = [62.0, 62.0, 62.0, 62.0, 62.0, 62.0];

/// Дефолтный выбранный пресет размера (F3), когда у ядра ещё нет своего выбора.
const SIZE_SEL_DEFAULT: usize = 2;

/// Умная подпись значения по порядку величины (size/sell). Точность адаптивная: ≥100 — целое
/// (без десятых), 10..100 — десятые (без сотых), 1..10 — сотые, <1 — столько знаков, чтобы
/// показать ~2 значащих (0.6→"0.6", 0.001→"0.001", 0.00001→"0.00001"). Хвостовые нули убираем
/// (убирает и float-мусор от f32, напр. 0.6000000238 → "0.6").
pub fn fmt_adaptive(v: f64) -> String {
    let a = v.abs();
    let decimals: usize = if a == 0.0 {
        0
    } else if a >= 100.0 {
        0
    } else if a >= 10.0 {
        1
    } else if a >= 1.0 {
        2
    } else {
        // <1: ~2 значащих цифры. 0.6→2, 0.001→4, 0.00001→6.
        let lead = (-a.log10().floor()) as i32; // 0.6→1, 0.001→3, 0.00001→5
        (lead + 1).clamp(2, 8) as usize
    };
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Шаг колеса по порядку величины: `step = frac · 10^floor(log10(v))`. `frac=1.0` — полный
/// разряд (размер: 18→20→30; 93→100→200; 980→1000→2000; 0.001→0.002). `frac=0.5` — полразряда
/// (sell: 10→15→20→25; 0.1→0.15→0.2). Вверх — следующий кратный, вниз — предыдущий; на точной
/// степени 10 шаг вниз падает на разряд ниже (111→100→90→80, а не стоп на 100).
fn wheel_step(value: f64, up: bool, frac: f64) -> f64 {
    if !(value > 0.0) {
        return value;
    }
    let step = frac * 10f64.powf(value.log10().floor());
    let raw = if up {
        ((value / step + 1e-9).floor() + 1.0) * step
    } else {
        let mut down = ((value / step - 1e-9).ceil() - 1.0) * step;
        if down <= 0.0 {
            // value на точной ступени (например 100 при frac=1) → один шаг разрядом ниже.
            let lower = frac * 10f64.powf((value * (1.0 - 1e-9)).log10().floor());
            down = value - lower;
        }
        if down <= 0.0 {
            return value;
        }
        down
    };
    (raw * 1e8).round() / 1e8
}

/// Направление колеса (вверх = +Y). Если в реале инвертировано — поменять знак сравнения.
fn scroll_up(ev: &ScrollWheelEvent) -> bool {
    let y = match ev.delta {
        ScrollDelta::Lines(p) => p.y,
        ScrollDelta::Pixels(p) => f32::from(p.y),
    };
    y > 0.0
}

/// Полоса пресетов размера ордера (значения, без подписей F1-F6). Значения — из конфига ядра
/// (или дефолт по базе BTC/USDT), выбор хранится per-core в `Backend::order_size_sel`.
/// Взаимодействие — прозрачным overlay поверх каждой кнопки (MoonSegmentedControl сам колесо
/// не умеет): одиночный клик = выбор; дабл-клик = инлайн-правка (`order_size_edit_req`); КОЛЕСО
/// = ±значение с шагом по порядку величины (наведи и крути, не нажимая). `core=None` → без
/// взаимодействия.
fn size_strip(
    values: [f64; 6],
    sel: usize,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let mut it = MoonSegmentItem::new("", fmt_adaptive(values[i])).width(SIZE_W[i]);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-size-presets")
        .accent(MoonAccent::Amber)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "size",
        &SIZE_W,
        edit_ix,
        "toolbar-size-edit",
        input,
        core.is_some(),
        // Одиночный клик = выбор пресета; дабл = инлайн-правка (`order_size_edit_req`).
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.order_size_edit_req = Some((core, i));
                } else {
                    b.order_size_sel.insert(core, i);
                }
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
            });
        },
        // Колесо = ±значение с шагом по порядку величины (frac 1.0).
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.order_size_value(core, i);
                let next = wheel_step(cur, up, 1.0);
                if next != cur {
                    b.set_order_size_value(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                }
            });
        },
    )
}

/// Ширины кнопок продажи — единая 62 на все слоты.
const SELL_W: [f32; 6] = [62.0, 62.0, 62.0, 62.0, 62.0, 62.0];

/// Полоса fixed-sell пресетов (S1-S6). Значения — из `ClientSettings` активного ядра
/// (видимые проценты), выбранный пресет подсвечен (`fixed_sell_slot`). Нет ядра/настроек —
/// прочерки. Запись (смена пресета) — Этап 4: пока клик логируется.
fn sell_strip(
    pcts: Option<[f64; 6]>,
    sel_slot: Option<usize>,
    edit_ix: Option<usize>,
    input: &Entity<MoonInputState>,
    backend: Entity<Backend>,
    core: Option<CoreId>,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = (0..6)
        .map(|i| {
            let value = match pcts {
                Some(p) => format!("+{}%", fmt_adaptive(p[i])),
                None => "—".to_string(),
            };
            let mut it = MoonSegmentItem::new("", value).width(SELL_W[i]);
            if sel_slot == Some(i + 1) {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new("toolbar-sell-presets")
        .accent(MoonAccent::Blue)
        .items(items)
        .render();

    let backend_click = backend.clone();
    strip_with_overlay(
        seg,
        "sell",
        &SELL_W,
        edit_ix,
        "toolbar-sell-edit",
        input,
        core.is_some(),
        // Одиночный клик = выбрать слот (меняет TP); дабл = инлайн-правка %.
        move |i, dbl, cx| {
            let Some(core) = core else { return };
            backend_click.update(cx, |b, bcx| {
                if dbl {
                    b.sell_edit_req = Some((core, i));
                } else if let Err(error) = b
                    .session
                    .edit_client_settings(core, ClientSettingsEdit::SelectFixedSellSlot(i + 1))
                {
                    log::warn!("select fixed-sell slot failed: {error}");
                }
                bcx.notify();
            });
        },
        // Колесо = ±% полразрядом (frac 0.5). Значение % — на ядре, читаем из снимка ClientSettings.
        move |i, up, cx| {
            let Some(core) = core else { return };
            backend.update(cx, |b, bcx| {
                let cur = b.fixed_sell_pct(core, i);
                let next = wheel_step(cur, up, 0.5);
                if next != cur {
                    // Оптимистично: локальный кэш + перерисовка СРАЗУ; в ядро — тоже.
                    b.set_fixed_sell_pct_local(core, i, next);
                    b.order_size_rev = b.order_size_rev.wrapping_add(1);
                    bcx.notify();
                    if let Err(error) = b.session.edit_client_settings(
                        core,
                        ClientSettingsEdit::SetFixedSellPct {
                            slot: i + 1,
                            pct: next,
                        },
                    ) {
                        log::warn!("set fixed-sell pct (wheel) failed: {error}");
                    }
                }
            });
        },
    )
}

/// Общий каркас полос пресетов (size/sell): сегментированный контрол + прозрачные overlay
/// поверх каждой кнопки (MoonSegmentedControl сам клик/дабл/колесо не различает) + инлайн-инпут
/// поверх редактируемой кнопки. Отличия size/sell — только в `on_click`/`on_wheel` (наведи и
/// крути, не нажимая). `overlay=false` (нет ядра) → без взаимодействия.
#[allow(clippy::too_many_arguments)]
fn strip_with_overlay(
    seg: impl IntoElement,
    id_prefix: &'static str,
    widths: &[f32; 6],
    edit_ix: Option<usize>,
    edit_input_id: &'static str,
    input: &Entity<MoonInputState>,
    overlay: bool,
    on_click: impl Fn(usize, bool, &mut App) + Clone + 'static,
    on_wheel: impl Fn(usize, bool, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let mut root = div().relative().flex().items_center().child(seg);

    if overlay {
        for i in 0..6 {
            let left: f32 = widths.iter().take(i).sum();
            let on_click = on_click.clone();
            let on_wheel = on_wheel.clone();
            root = root.child(
                div()
                    .id(SharedString::from(format!("{id_prefix}-hit-{i}")))
                    .absolute()
                    .left(px(left))
                    .top(px(0.0))
                    .w(px(widths[i]))
                    .h_full()
                    .on_mouse_down(MouseButton::Left, move |ev, _w, cx| {
                        on_click(i, ev.click_count >= 2, cx);
                    })
                    .on_scroll_wheel(move |ev, _w, cx| {
                        on_wheel(i, scroll_up(ev), cx);
                    }),
            );
        }
    }

    // Инпут поверх редактируемой кнопки (absolute по сумме ширин предыдущих), на самом верху.
    if let Some(ix) = edit_ix.filter(|i| *i < 6) {
        let left: f32 = widths.iter().take(ix).sum();
        root = root.child(
            div()
                .absolute()
                .left(px(left))
                .top(px(0.0))
                .w(px(widths[ix]))
                .h_full()
                .child(MoonInput::new(edit_input_id).state(input).small()),
        );
    }
    root
}

fn scale_label(scale: Option<f32>) -> &'static str {
    SCALES
        .iter()
        .find(|(_, value)| *value == scale)
        .map(|(label, _)| *label)
        .unwrap_or("Авто")
}

/// Дропдаун масштаба для полоски чарт-вкладок главного окна: применяет масштаб ТОЛЬКО к
/// АКТИВНОЙ вкладке (Main или конкретный AddToChart), не трогая другие вкладки/окна, и
/// сохраняет (per-вкладочный масштаб). Стоит рядом с кнопкой ⚙ настроек раскладки.
/// Общая сборка дропдауна масштаба: единственные отличия вкладок и AddToChart-stack —
/// набор id, размер триггера (`Micro`/`Toolbar`) и куда писать выбранный масштаб
/// (`on_pick`). Визуал/тултип/лупа/«А» для Авто — общие.
fn scale_dropdown(
    scale: Option<f32>,
    tip_id: &'static str,
    dropdown_id: &'static str,
    item_key_prefix: &'static str,
    trigger_size: MoonButtonSize,
    p: MoonPalette,
    on_pick: impl Fn(Option<f32>, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let selected_label = scale_label(scale);
    let mut items = Vec::with_capacity(SCALES.len());
    for (label, pct) in SCALES {
        let on_pick = on_pick.clone();
        items.push(
            MoonMenuItem::with_key(format!("{item_key_prefix}-{label}"), label)
                .selected(scale == pct)
                .checked(scale == pct)
                .on_click(move |_, _, cx| on_pick(pct, cx)),
        );
    }

    // Лупа вместо слова «МАСШТАБ» + «А» для Авто (компактнее); подсказка «Масштаб» — тултипом.
    let trigger_val = if scale.is_none() {
        "А"
    } else {
        selected_label
    };
    div()
        .id(tip_id)
        .tooltip(|_window, cx| {
            cx.new(|_| MoonTooltipView::new(t!("toolbar.scale").to_string()))
                .into()
        })
        .child(
            MoonDropdown::new(dropdown_id)
                .trigger_width(72.0)
                .trigger_variant(MoonButtonVariant::Neutral)
                .trigger_size(trigger_size)
                .menu_width(116.0)
                .menu_size(MoonMenuSize::Compact)
                .segment(
                    MoonButtonSegment::new("🔍")
                        .color(p.text_muted)
                        .weight(400.0),
                )
                .segment(
                    MoonButtonSegment::new(trigger_val)
                        .color(p.text)
                        .weight(500.0),
                )
                .items(items),
        )
}

pub(crate) fn scale_dropdown_for_tabs(
    scale: Option<f32>,
    tabs: Entity<crate::chart_tabs::ChartTabs>,
    p: MoonPalette,
) -> impl IntoElement {
    scale_dropdown(
        scale,
        "tabs-scale-tip",
        "tabs-scale-dropdown",
        "scale-tab",
        MoonButtonSize::Micro,
        p,
        move |pct, cx| {
            tabs.update(cx, |t, tcx| t.pick_active_scale(pct, tcx));
        },
    )
}

/// Дропдаун масштаба для AddToChart-stack: пишет масштаб во все отдельные ChartPanel внутри
/// stack-а. Это сохраняет Delphi-модель "один график = одна сущность", но управление масштабом
/// остаётся единым для окна/вкладки.
pub(crate) fn scale_dropdown_for_add_stack(
    scale: Option<f32>,
    stack: Entity<crate::chart_tabs::AddChartStack>,
    p: MoonPalette,
) -> impl IntoElement {
    scale_dropdown(
        scale,
        "detached-stack-scale-tip",
        "detached-stack-scale-dropdown",
        "scale-stack",
        MoonButtonSize::Toolbar,
        p,
        move |pct, cx| {
            stack.update(cx, |st, scx| st.set_scale(pct, scx));
        },
    )
}

/// Полоса тулбара: рисуется как обычный child `Shell` (между шапкой и доком), не dock-панель.
/// Читает текущий масштаб/follow из `backend`, клики пишут обратно (+notify → перерисовка).
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn toolbar(
    backend: &Entity<Backend>,
    group: &str,
    size_edit: Option<(CoreId, usize)>,
    size_input: &Entity<MoonInputState>,
    sell_edit: Option<(CoreId, usize)>,
    sell_input: &Entity<MoonInputState>,
    shell: &Entity<Shell>,
    open_metric: Option<TradeMetric>,
    cx: &App,
) -> impl IntoElement {
    let (follow, focus_core, size_values, size_sel, tp_str, sl_str, lev_str, sell_pcts, sell_slot) = {
        let b = backend.read(cx);
        // Активное торговое ядро = выбор в селекторе шапки (sticky-override) ИЛИ ядро
        // открытого фуллскрином Main-чарта. Все торговые контролы (размеры/TP/SL/Lev/sell)
        // читают ЕГО. Нет ядра → дефолтные размеры, прочерки, клики игнор.
        let focus_core = b.active_trade_core(group);
        let (size_values, size_sel) = match focus_core {
            Some(core) => b.manual_order_size_state(core),
            None => (
                moon_core::config::servers::default_order_sizes(""),
                SIZE_SEL_DEFAULT,
            ),
        };
        let core_data = focus_core.and_then(|c| b.session.store().core(c));
        let cs = core_data.and_then(|d| d.client_settings.as_ref());
        let tp_str = cs
            .map(|s| format!("{}%", fmt_field2(s.take_profit_pct as f32)))
            .unwrap_or_else(|| "—".to_string());
        // SL знаковый: «+1,00%» / «-20,00%» (а не «--» из ручного минуса перед отрицательным).
        let sl_str = cs
            .map(|s| format!("{}%", fmt_field2_signed(s.stop_loss_pct)))
            .unwrap_or_else(|| "—".to_string());
        // Накладываем оптимистичный локальный кэш поверх значений ядра (живой sell-дисплей).
        let sell_pcts = focus_core.zip(cs).map(|(core, s)| {
            let arr: [f64; 6] =
                std::array::from_fn(|i| b.fixed_sell_pct_with(core, i, s.fixed_sell_pcts[i]));
            arr
        });
        let sell_slot = cs.map(|s| s.fixed_sell_slot);
        // Lev = плечо монеты main-чарта на активном ядре (per-core, per-coin) из ассетов.
        let lev_str = TradeMetric::Lev
            .current(b, group)
            .filter(|l| *l > 0.0)
            .map(|l| format!("×{}", l as i32))
            .unwrap_or_else(|| "—".to_string());
        (
            b.follow,
            focus_core,
            size_values,
            size_sel,
            tp_str,
            sl_str,
            lev_str,
            sell_pcts,
            sell_slot,
        )
    };
    let p = MoonPalette::active(cx);

    let mut row = h_flex()
        .id("toolbar")
        .w_full()
        .h(design::fit_h_px(cx, TOOLBAR_H, 13.0, 9.5))
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .px(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .border_b_1()
        .border_color(rgb(p.border));

    row = row
        .child(metric_button(
            TradeMetric::Tp,
            tp_str,
            p.blue,
            74.6,
            open_metric == Some(TradeMetric::Tp),
            shell.clone(),
            p,
        ))
        .child(metric_button(
            TradeMetric::Sl,
            sl_str,
            p.red,
            74.6,
            open_metric == Some(TradeMetric::Sl),
            shell.clone(),
            p,
        ))
        .child(metric_button(
            TradeMetric::Lev,
            lev_str,
            p.text,
            61.6,
            open_metric == Some(TradeMetric::Lev),
            shell.clone(),
            p,
        ))
        .child(divider(p))
        .child(strip_label("size", p, cx))
        .child(size_strip(
            size_values,
            size_sel,
            // Редактируем инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
            size_edit
                .filter(|(c, _)| Some(*c) == focus_core)
                .map(|(_, i)| i),
            size_input,
            backend.clone(),
            focus_core,
        ))
        .child(divider(p))
        .child(strip_label("sell", p, cx))
        .child(sell_strip(
            sell_pcts,
            sell_slot,
            // Редактируем S-инпутом только если запрос относится к ФОКУСНОМУ ядру тулбара.
            sell_edit
                .filter(|(c, _)| Some(*c) == focus_core)
                .map(|(_, i)| i),
            sell_input,
            backend.clone(),
            focus_core,
        ))
        .child(divider(p));
    // Масштаб переехал в полоску чарт-вкладок (рядом с ⚙) и теперь per-вкладочный —
    // см. controls::scale_dropdown_for_tabs / chart_tabs::ChartTabs::pick_active_scale.

    let backend = backend.clone();
    row.child(
        MoonButton::new("live")
            .width(54.0)
            .variant(if follow {
                MoonButtonVariant::Green
            } else {
                MoonButtonVariant::Soft
            })
            .size(MoonButtonSize::Toolbar)
            .selected(follow)
            .label(if follow {
                t!("toolbar.live").to_string()
            } else {
                t!("toolbar.pause").to_string()
            })
            .on_click(move |_, _, cx| {
                backend.update(cx, |b, bcx| {
                    b.follow = !b.follow;
                    bcx.notify();
                });
            })
            .render(),
    )
}
