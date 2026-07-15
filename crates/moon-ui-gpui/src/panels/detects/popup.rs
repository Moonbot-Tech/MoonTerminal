//! Попап-конструктор отображения ленты детектов: ⚙ в полоске панели — ТРИГГЕР
//! [`MoonPopover`] (оверлей-слой moonui: не клипуется панелью, закрытие по клику-вне
//! даром). Вкладка размера = АКТИВНЫЙ размер ленты (переключение сразу меняет ленту —
//! она и есть живое превью). Слайдеры Ширина/Высота/Полоса/Градиент, тип графика и
//! грид слотов, повторяющий ПОЗИЦИИ полей на кнопке (мини 2×2, средний 3×2, крупный
//! 3×3) внутри рамки-«карточки» с rail. Элементы — в идиоме попапов настроек чарта
//! (framed-группы, сегменты, stateless-контролы). Копировать/Вставить обмениваются
//! конфигом группы текстом detects_view.toml (как вкладки Настроек).

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonDropdown, MoonMenuSize,
    MoonNotification, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonSegmentItem,
    MoonSegmentedControl, MoonSlider, MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use moon_core::config::{
    DETECT_SIZE_LARGE, DETECT_SIZE_MEDIUM, DETECT_SIZE_MINI, DetectChart, DetectField,
    DetectViewCfg, detect_slot_count,
};

use super::{DetectsPanel, cards};
use crate::design;
use crate::panels::{RadioMark, radio_items};

/// Ширина попапа (лог. px): 3 колонки слотов крупного (выпадашка 76 + 3 тогла по 20)
/// + зазоры/рамки/паддинги с запасом.
const POPUP_W: f32 = 560.0;

/// Вкладки размеров: (код, ключ локали).
const TABS: [(u8, &str); 3] = [
    (DETECT_SIZE_MINI, "detects.view.size_mini"),
    (DETECT_SIZE_MEDIUM, "detects.view.size_medium"),
    (DETECT_SIZE_LARGE, "detects.view.size_large"),
];

/// Типы графика: (значение, ключ локали).
const CHARTS: [(DetectChart, &str); 3] = [
    (DetectChart::None, "detects.cfg.chart_none"),
    (DetectChart::Candles, "detects.cfg.chart_candles"),
    (DetectChart::Line, "detects.cfg.chart_line"),
];

/// Ключ локали подписи поля слота.
fn field_key(f: DetectField) -> &'static str {
    match f {
        DetectField::None => "detects.field.none",
        DetectField::Coin => "detects.field.coin",
        DetectField::Time => "detects.view.time",
        DetectField::Badge => "detects.view.badge",
        DetectField::Core => "detects.view.core",
        DetectField::Delta24h => "detects.field.d24",
        DetectField::Delta1h => "detects.field.d1",
        DetectField::Exchange => "detects.view.exchange",
        DetectField::ExchangeKind => "detects.view.exchange_kind",
    }
}

impl DetectsPanel {
    /// Засеять слайдеры значениями конфига редактируемой вкладки (эхо гасит write_view).
    fn seed_sliders(&self, window: &mut Window, cx: &mut Context<Self>) {
        let cfg = self.view_cfg(cx);
        let s = *cfg.size_cfg(self.popup_tab);
        for (sl, v) in [
            (&self.w_slider, f32::from(s.w)),
            (&self.h_slider, f32::from(s.h)),
            (&self.rail_slider, f32::from(s.rail_w_clamped())),
            (&self.grad_slider, f32::from(s.rail_grad)),
        ] {
            sl.update(cx, |s, c| s.set_value(v, window, c));
        }
    }

    /// «Копировать»: конфиг группы текстом detects_view.toml → буфер.
    fn copy_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.view_cfg(cx).to_share_string() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(
            MoonNotification::success(t!("settings.copied").to_string()),
            cx,
        );
    }

    /// «Вставить»: разобрать буфер как конфиг группы, применить + persist + пересидировать.
    fn paste_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let Some(cfg) = DetectViewCfg::parse_share(&text) else {
            window.push_notification(
                MoonNotification::error(t!("settings.paste_wrong").to_string()),
                cx,
            );
            return;
        };
        self.write_view(cx, |c| *c = cfg);
        self.popup_tab = cfg.size_clamped();
        self.seed_sliders(window, cx);
    }
}

/// Полоска сверху панели: ⚙ = триггер попапа-конструктора (MoonPopover, поверх всего).
pub(super) fn toolbar(
    this: &DetectsPanel,
    cfg: &DetectViewCfg,
    p: MoonPalette,
    cx: &mut Context<DetectsPanel>,
) -> Div {
    let entity = cx.entity();
    let gear = MoonButton::new("detects-view-gear")
        .label("⚙")
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .tooltip(t!("detects.cfg.title").to_string())
        .render();
    let popover = MoonPopover::new("detects-view-popover")
        .placement(MoonPopoverPlacement::BottomStart)
        .width(POPUP_W)
        .close_on_content_click(false)
        // Меню дропдаунов — отдельные deferred-слои и вылезают за границы попапа: клик
        // по ним ловился бы как «мимо» и закрывал попап. Закрытие — крестиком/ESC/⚙.
        .overlay_closable(false)
        .open(this.popup_open)
        .on_open_change(move |open, window, app| {
            entity.update(app, |this, cx| {
                this.popup_open = open;
                if open {
                    this.popup_tab = this.view_cfg(cx).size_clamped();
                    this.seed_sliders(window, cx);
                }
                cx.notify();
            });
        })
        .trigger(gear)
        .content(content(this, cfg, p, cx));
    h_flex()
        .w_full()
        .flex_none()
        .h(design::fit_h_px(cx, 28.0, 13.0, 7.5))
        .items_center()
        .px_2()
        .bg(rgb(p.tabbar))
        .child(popover)
}

/// Рамка-группа: тонкая граница + заголовок-капшен (идиома candle_popup::framed).
fn framed(title: String, p: MoonPalette, cx: &App, body: AnyElement) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .px(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(title),
        )
        .child(body)
}

/// Ряд слайдера: подпись слева, слайдер, значение справа.
fn slider_row(
    caption: String,
    slider: &Entity<moon_ui::MoonSliderState>,
    value_label: String,
    p: MoonPalette,
    cx: &App,
) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .w(design::ui_px(cx, 64.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(div().flex_1().child(MoonSlider::new(slider).height(16.0)))
        .child(
            div()
                .w(design::ui_px(cx, 34.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(value_label),
        )
}

/// Микро-кнопка-тогл слота (глиф + тултип), в духе кнопок инструментов рисования.
#[allow(clippy::too_many_arguments)]
fn slot_toggle(
    id: SharedString,
    glyph: &'static str,
    tip: String,
    on: bool,
    entity: Entity<DetectsPanel>,
    tab: u8,
    slot_ix: usize,
    set: fn(&mut moon_core::config::DetectSlot, bool),
) -> impl IntoElement {
    MoonButton::new(id)
        .label(glyph)
        .size(MoonButtonSize::Micro)
        .width(20.0)
        .variant(if on {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(on)
        .tooltip(tip)
        .on_click(move |_, _w, app| {
            let next = !on;
            entity.update(app, |this, cx| {
                this.write_view(cx, |cfg| {
                    set(&mut cfg.size_cfg_mut(tab).slots[slot_ix], next);
                });
            });
        })
        .render()
}

/// Ячейка слота: выпадашка поля + тоглы ВПЛОТНУЮ к ней. Вложенный MoonDropdown внутри
/// MoonPopover безопасен с фикса форка `0f3ace9` (deferred-раунды на месте).
fn slot_cell(
    cfg: &DetectViewCfg,
    tab: u8,
    slot_ix: usize,
    cx: &mut Context<DetectsPanel>,
) -> impl IntoElement {
    let scfg = cfg.size_cfg(tab);
    let slot = scfg.slots[slot_ix];
    let entity = cx.entity();

    let entity_pick = entity.clone();
    let items = radio_items(
        DetectField::ALL.iter().map(|f| {
            (
                *f,
                SharedString::from(format!("ds-{tab}-{slot_ix}-{f:?}")),
                SharedString::from(t!(field_key(*f)).to_string()),
            )
        }),
        slot.field,
        RadioMark::Check,
        move |app, f: DetectField| {
            entity_pick.update(app, |this, cx| {
                this.write_view(cx, |cfg| {
                    cfg.size_cfg_mut(tab).slots[slot_ix].field = f;
                });
            });
        },
    );
    let dropdown = MoonDropdown::new(SharedString::from(format!("det-slot-{tab}-{slot_ix}")))
        .label(format!("{} ▾", t!(field_key(slot.field))))
        .trigger_variant(MoonButtonVariant::Soft)
        .trigger_size(MoonButtonSize::Micro)
        .trigger_width(design::font_w(cx, 76.0))
        .menu_width(design::font_w(cx, 130.0))
        .menu_size(MoonMenuSize::Compact)
        .items(items);

    // Кнопки прижаты к полю (без распорок) — ячейка пакуется влево.
    let mut row = h_flex().items_center().gap(px(2.0)).child(dropdown);
    let chart_on = scfg.chart != DetectChart::None;
    if tab != DETECT_SIZE_MINI && chart_on {
        row = row.child(slot_toggle(
            SharedString::from(format!("det-over-{tab}-{slot_ix}")),
            "▧",
            if slot.over {
                t!("detects.cfg.over_on").to_string()
            } else {
                t!("detects.cfg.over_off").to_string()
            },
            slot.over,
            entity.clone(),
            tab,
            slot_ix,
            |s, v| s.over = v,
        ));
    }
    row = row.child(slot_toggle(
        SharedString::from(format!("det-align-{tab}-{slot_ix}")),
        if slot.right { "⇥" } else { "⇤" },
        if slot.right {
            t!("detects.cfg.align_right").to_string()
        } else {
            t!("detects.cfg.align_left").to_string()
        },
        slot.right,
        entity.clone(),
        tab,
        slot_ix,
        |s, v| s.right = v,
    ));
    if tab == DETECT_SIZE_LARGE {
        row = row.child(slot_toggle(
            SharedString::from(format!("det-vpos-{tab}-{slot_ix}")),
            if slot.below { "↓" } else { "↑" },
            if slot.below {
                t!("detects.cfg.vpos_below").to_string()
            } else {
                t!("detects.cfg.vpos_above").to_string()
            },
            slot.below,
            entity.clone(),
            tab,
            slot_ix,
            |s, v| s.below = v,
        ));
    }
    row
}

/// Контент попапа. Значения читаются из конфига на каждый рендер (stateless, кроме слайдеров).
fn content(
    this: &DetectsPanel,
    cfg: &DetectViewCfg,
    p: MoonPalette,
    cx: &mut Context<DetectsPanel>,
) -> AnyElement {
    let tab = this.popup_tab.min(DETECT_SIZE_LARGE);
    let scfg = cfg.size_cfg(tab);
    let entity = cx.entity();

    // --- Шапка: заголовок + Копировать/Вставить ---
    let head = h_flex()
        .w_full()
        .items_center()
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("detects.cfg.title").to_string()),
        )
        .child(div().flex_1())
        .child(
            MoonButton::new("det-view-copy")
                .label(t!("settings.copy").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.copy_view(window, cx)))
                .render(),
        )
        .child(
            MoonButton::new("det-view-paste")
                .label(t!("settings.paste").to_string())
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| this.paste_view(window, cx)))
                .render(),
        )
        .child(
            MoonButton::new("det-view-close")
                .label("✕")
                .size(MoonButtonSize::Micro)
                .variant(MoonButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _w, cx| {
                    this.popup_open = false;
                    cx.notify();
                }))
                .render(),
        );

    // --- Вкладки размеров: выбор = АКТИВНЫЙ размер ленты (лента = живое превью) ---
    let entity_tab = entity.clone();
    let tabs = MoonSegmentedControl::new("det-view-tabs")
        .accent(MoonAccent::Blue)
        .items(TABS.iter().map(|(sz, key)| {
            let mut it = MoonSegmentItem::new("", t!(*key).to_string()).width(92.0);
            if *sz == tab {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, window, app| {
            if let Some((sz, _)) = TABS.get(ix) {
                let sz = *sz;
                entity_tab.update(app, |this, cx| {
                    this.popup_tab = sz;
                    this.write_view(cx, |c| c.size = sz);
                    this.seed_sliders(window, cx);
                    cx.notify();
                });
            }
        })
        .render();
    // Точность дельт (0/1/2 знака) — ОДНА настройка на все размеры, справа от вкладок.
    let entity_dec = entity.clone();
    let cur_dec = cfg.delta_decimals_clamped();
    let dec_seg = MoonSegmentedControl::new("det-view-decimals")
        .accent(MoonAccent::Blue)
        .items((0..=2usize).map(|d| {
            let mut it = MoonSegmentItem::new("", format!("{d}")).width(26.0);
            if d == cur_dec {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, _w, app| {
            entity_dec.update(app, |this, cx| {
                this.write_view(cx, |c| c.delta_decimals = ix as u8);
            });
        })
        .render();
    let tabs_row = h_flex()
        .w_full()
        .items_center()
        .child(tabs)
        .child(div().flex_1())
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .mr(design::ui_px(cx, 5.0))
                .child(t!("detects.cfg.decimals").to_string()),
        )
        .child(dec_seg);

    // --- Размер и график ---
    let w_row = slider_row(
        t!("detects.cfg.width").to_string(),
        &this.w_slider,
        format!("{}", scfg.w),
        p,
        cx,
    );
    let h_row = slider_row(
        t!("detects.cfg.height").to_string(),
        &this.h_slider,
        format!("{}", scfg.h),
        p,
        cx,
    );
    let entity_chart = entity.clone();
    let chart_seg = MoonSegmentedControl::new("det-view-chart")
        .accent(MoonAccent::Blue)
        .items(CHARTS.iter().map(|(k, key)| {
            let mut it = MoonSegmentItem::new("", t!(*key).to_string()).width(70.0);
            if *k == scfg.chart {
                it = it.selected(true);
            }
            it
        }))
        .on_click(move |ix, _, _w, app| {
            if let Some((k, _)) = CHARTS.get(ix) {
                let k = *k;
                entity_chart.update(app, |this, cx| {
                    let tab = this.popup_tab;
                    this.write_view(cx, |c| c.size_cfg_mut(tab).chart = k);
                });
            }
        })
        .render();
    let chart_row = h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .w(design::ui_px(cx, 64.0))
                .flex_none()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(t!("detects.cfg.chart").to_string()),
        )
        .child(chart_seg);

    // --- Полоса сервера: свотч-подпись + два слайдера ---
    let rail_caption = h_flex()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .child(
            div()
                .w(px(3.0))
                .h(px(12.0))
                .rounded(px(2.0))
                .bg(rgb(p.blue)),
        )
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("detects.cfg.rail_hint").to_string()),
        );
    let rail_row = slider_row(
        t!("detects.cfg.rail_w").to_string(),
        &this.rail_slider,
        format!("{}", scfg.rail_w_clamped()),
        p,
        cx,
    );
    let grad_row = slider_row(
        t!("detects.cfg.rail_grad").to_string(),
        &this.grad_slider,
        format!("{}", scfg.rail_grad_clamped()),
        p,
        cx,
    );

    // --- Грид слотов: ПОЗИЦИИ как на кнопке (мини 2×2, средний 3×2, крупный 3×3),
    // внутри рамки-«карточки» с rail — как будто настраиваешь сам детект. ---
    let cols = if tab == DETECT_SIZE_MINI { 2 } else { 3 };
    let n = detect_slot_count(tab);
    let mut grid = v_flex().w_full().gap(design::ui_px(cx, 8.0));
    let mut ix = 0usize;
    while ix < n {
        let mut row = h_flex().w_full().gap(design::ui_px(cx, 6.0));
        for col in 0..cols {
            let i = ix + col;
            let cell = div().flex_1().min_w(px(0.0));
            row = row.child(if i < n {
                cell.child(slot_cell(cfg, tab, i, cx))
            } else {
                cell
            });
        }
        grid = grid.child(row);
        ix += cols;
    }
    let grid_card = div()
        .relative()
        .w_full()
        .rounded(design::ui_px(cx, 6.0))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.shell_high))
        .overflow_hidden()
        .children(cards::rail_layers(p.blue, 3.0, 20.0, POPUP_W - 28.0, cx))
        .child(
            div()
                .w_full()
                .pl(px(12.0))
                .pr(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 8.0))
                .child(grid),
        );

    v_flex()
        .id("detects-view-popup")
        .w_full()
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        .child(head)
        .child(tabs_row)
        .child(framed(
            t!("detects.cfg.frame_size").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(w_row)
                .child(h_row)
                .child(chart_row)
                .into_any_element(),
        ))
        .child(framed(
            t!("detects.cfg.rail_frame").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(rail_caption)
                .child(rail_row)
                .child(grad_row)
                .into_any_element(),
        ))
        .child(framed(
            t!("detects.cfg.frame_fields").to_string(),
            p,
            cx,
            grid_card.into_any_element(),
        ))
        .into_any_element()
}
