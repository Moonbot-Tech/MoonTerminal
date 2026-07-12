//! Попап «Свечи и трейды» — настройки отображения свечей/зоны трейдов АКТИВНОЙ вкладки
//! или выносного окна (кнопка ❚ рядом с ⚙; per-вкладка, как настройки раскладки).
//! Persist — спек вкладки в charts.json (`ChartTabSpec::candle_view`); вкладки без своего
//! значения следуют глобальному дефолту `layout.candle_view`. Кнопка ⧉ распространяет
//! набор на все вкладки/окна (как «применить ко всем» у ⚙) и обновляет глобальный дефолт.
//! Все контролы stateless (сегменты/чекбоксы/свотчи). Нейтральный цвет — в тему чарта
//! активного режима (theme.toml `candle_neutral`, общий).

use gpui::*;
use moon_core::market::candles::{
    CANDLE_MODE_FILLED, CANDLE_MODE_OFF, CANDLE_MODE_OUTLINE, CANDLE_MODE_OUTLINE_IN_ZONE,
    CandleViewCfg,
};
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use super::common::{LayoutPopupHost, StackSetting};
use crate::design;

/// Таймфреймы (подпись) — синхронно с `CANDLE_TF_CHOICES_MIN` (0 = 30 секунд).
const TFS: [(u32, &str); 7] = [
    (0, "30с"),
    (1, "1м"),
    (5, "5м"),
    (30, "30м"),
    (60, "1ч"),
    (240, "4ч"),
    (1440, "1д"),
];

/// Режимы: «Нет» (чистый тик-чарт) первым, дальше как в MoonBot.
const MODES: [u8; 4] = [
    CANDLE_MODE_OFF,
    CANDLE_MODE_FILLED,
    CANDLE_MODE_OUTLINE,
    CANDLE_MODE_OUTLINE_IN_ZONE,
];

/// Ступени «сколько последних свечей перерисовываем трейдами» (0 = только свечи) —
/// они же для «скрыть последних свечей».
const ZONES: [u16; 7] = [0, 1, 2, 3, 5, 10, 20];

const OUTLINES: [u8; 3] = [1, 2, 3];

/// Пресеты нейтрального цвета зоны трейдов (sRGB).
const NEUTRALS: [[u8; 3]; 6] = [
    [128, 128, 128],
    [90, 90, 90],
    [170, 170, 170],
    [96, 125, 160],
    [150, 140, 90],
    [120, 100, 140],
];

/// Ширина сценового попапа (лог. px): самый широкий ряд — ТФ, 7 сегментов × 42 + поля/рамка.
pub(super) fn content_width(cx: &App) -> Pixels {
    let pad = f32::from(design::ui_px(cx, 8.0));
    let fpx = f32::from(design::ui_px(cx, 6.0));
    px(7.0 * 42.0 + 2.0 * pad + 2.0 * fpx + 8.0)
}

/// Рамка-группа: тонкая граница + заголовок-капшен (копия layout_popup::framed).
fn framed(title: String, p: MoonPalette, cx: &App, body: AnyElement) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 4.0))
        .px(design::ui_px(cx, 6.0))
        .py(design::ui_px(cx, 4.0))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::ui_px(cx, 4.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(title),
        )
        .child(body)
}

/// Подпись + сегмент-контрол под ней (одна настройка).
fn seg_row(
    id: String,
    caption: String,
    labels: Vec<(String, bool)>,
    seg_w: f32,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(usize, &mut App) + 'static,
) -> impl IntoElement {
    let items: Vec<MoonSegmentItem> = labels
        .into_iter()
        .map(|(label, selected)| {
            let mut it = MoonSegmentItem::new("", label).width(seg_w);
            if selected {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| on_pick(ix, cx))
        .render();
    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 2.0))
        .child(
            div()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(seg)
}

/// Многострочный хинт под контролом.
fn hint_block(key: &str, p: MoonPalette, cx: &App) -> impl IntoElement {
    v_flex().children(
        t!(key)
            .to_string()
            .split('\n')
            .map(|line| {
                div()
                    .text_size(design::t_caption(cx))
                    .text_color(rgb(p.text_muted))
                    .child(line.to_string())
            })
            .collect::<Vec<_>>(),
    )
}

/// Правка cfg цели: текущее значение → мутатор → применить к вкладке + persist в спек.
fn write_cfg<T: CandlePopupHost>(
    entity: &Entity<T>,
    app: &mut App,
    f: impl FnOnce(&mut CandleViewCfg),
) {
    entity.update(app, |this, cx| {
        let mut cfg = this.candle_view_current(cx);
        f(&mut cfg);
        this.apply_candle_view(cfg, cx);
    });
}

/// Контент попапа. Значения читаются из цели на каждый рендер (stateless-контролы).
fn render_candle_popup<T: CandlePopupHost>(
    id: &str,
    entity: Entity<T>,
    cfg: CandleViewCfg,
    neutral: [u8; 3],
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    // --- Рамка «Свечи»: ТФ + режим + толщина контура ---
    let tf_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-tf"),
            t!("chart.candles.tf").to_string(),
            TFS.iter()
                .map(|(m, l)| (l.to_string(), *m == cfg.tf_min))
                .collect(),
            42.0,
            p,
            cx,
            move |ix, app| {
                if let Some((m, _)) = TFS.get(ix) {
                    let m = *m;
                    write_cfg(&entity, app, |c| c.tf_min = m);
                }
            },
        )
    };
    let mode_label = |m: u8| -> String {
        match m {
            CANDLE_MODE_OFF => t!("chart.candles.mode_off").to_string(),
            CANDLE_MODE_FILLED => t!("chart.candles.mode_filled").to_string(),
            CANDLE_MODE_OUTLINE => t!("chart.candles.mode_outline").to_string(),
            _ => t!("chart.candles.mode_zone").to_string(),
        }
    };
    let mode_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-mode"),
            t!("chart.candles.mode").to_string(),
            MODES
                .iter()
                .map(|m| (mode_label(*m), *m == cfg.mode.min(CANDLE_MODE_OFF)))
                .collect(),
            70.0,
            p,
            cx,
            move |ix, app| {
                if let Some(m) = MODES.get(ix) {
                    let m = *m;
                    write_cfg(&entity, app, |c| c.mode = m);
                }
            },
        )
    };
    let outline_row = {
        let entity = entity.clone();
        let cur = (cfg.outline_px.round() as u8).clamp(1, 3);
        seg_row(
            format!("{id}-outline"),
            t!("chart.candles.outline").to_string(),
            OUTLINES
                .iter()
                .map(|w| (format!("{w}"), *w == cur))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(w) = OUTLINES.get(ix) {
                    let w = *w as f32;
                    write_cfg(&entity, app, |c| c.outline_px = w);
                }
            },
        )
    };

    // --- Рамка «Трейды»: зона (K свечей) + скрытие свечей + лимит + галки + нейтраль ---
    let zone_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-zone"),
            t!("chart.candles.zone").to_string(),
            ZONES
                .iter()
                .map(|k| (format!("{k}"), *k == cfg.trade_candles))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(k) = ZONES.get(ix) {
                    let k = *k;
                    write_cfg(&entity, app, |c| c.trade_candles = k);
                }
            },
        )
    };
    // «Скрыть последних свечей»: в этих бакетах свечи не рисуются вовсе — только трейды.
    let hide_row = {
        let entity = entity.clone();
        seg_row(
            format!("{id}-hide"),
            t!("chart.candles.hide").to_string(),
            ZONES
                .iter()
                .map(|k| (format!("{k}"), *k == cfg.hide_candles))
                .collect(),
            34.0,
            p,
            cx,
            move |ix, app| {
                if let Some(k) = ZONES.get(ix) {
                    let k = *k;
                    write_cfg(&entity, app, |c| c.hide_candles = k);
                }
            },
        )
    };
    // Галка «Линии цены» — оранжевая LastPrice + голубая MarkPrice линии moonproto.
    let price_lines_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-price-lines")))
            .label(t!("chart.candles.price_lines").to_string())
            .checked(cfg.price_lines)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.price_lines = v);
            })
    };
    let wicks_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-wicks")))
            .label(t!("chart.candles.wicks_in_zone").to_string())
            .checked(cfg.wicks_in_zone)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.wicks_in_zone = v);
            })
    };
    let neutral_cb = {
        let entity = entity.clone();
        MoonCheckbox::new(SharedString::from(format!("{id}-neutral-zone")))
            .label(t!("chart.candles.neutral_in_zone").to_string())
            .checked(cfg.neutral_in_zone)
            .size(MoonCheckboxSize::Compact)
            .on_change(move |ch: &bool, _w, app| {
                let v = *ch;
                write_cfg(&entity, app, |c| c.neutral_in_zone = v);
            })
    };
    // Свотчи нейтрального цвета: пишут в тему чарта АКТИВНОГО режима + сохраняют theme.toml
    // (цвета — общие для всех чартов, как остальная тема).
    let neutral_swatches = h_flex()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(t!("chart.candles.neutral").to_string()),
        )
        .children(NEUTRALS.iter().enumerate().map(|(i, c)| {
            let color = *c;
            let selected = color == neutral;
            let entity = entity.clone();
            let is_light = p.is_light();
            div()
                .id(SharedString::from(format!("{id}-neutral-{i}")))
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(3.0))
                .bg(gpui::rgb(
                    ((color[0] as u32) << 16) | ((color[1] as u32) << 8) | color[2] as u32,
                ))
                .border_1()
                .border_color(rgb(if selected { p.blue } else { p.border }))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _w, app| {
                    entity.update(app, |this, cx| {
                        this.backend().clone().update(cx, |b, bcx| {
                            let theme = b.config.theme.get_mut(is_light);
                            if theme.candle_neutral != color {
                                theme.candle_neutral = color;
                                if let Err(e) = b.config.theme.save() {
                                    log::warn!("theme.toml save failed: {e:#}");
                                }
                                bcx.notify();
                            }
                        });
                    });
                    app.stop_propagation();
                })
        }));

    // Иконка «применить ко всем» (⧉) — как у попапа раскладки: раздать набор ЭТОЙ цели
    // всем вкладкам/окнам + обновить глобальный дефолт (новые вкладки наследуют).
    let apply_all_btn = {
        let entity = entity.clone();
        MoonButton::new(SharedString::from(format!("{id}-apply-all")))
            .label("⧉")
            .tooltip(t!("chart.candles.apply_all").to_string())
            .size(MoonButtonSize::Micro)
            .variant(MoonButtonVariant::Ghost)
            .on_click(move |_, _w, app| {
                entity.update(app, |this, cx| {
                    let cfg = this.candle_view_current(cx);
                    this.apply_candle_view_all(cfg, cx);
                });
            })
            .render()
    };

    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("chart.candles.title").to_string()),
                )
                .child(div().flex_1())
                .child(apply_all_btn),
        )
        .child(framed(
            t!("chart.candles.frame_candles").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(tf_row)
                .child(mode_row)
                .child(hint_block("chart.candles.mode_hint", p, cx))
                .child(outline_row)
                .into_any_element(),
        ))
        .child(framed(
            t!("chart.candles.frame_trades").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(zone_row)
                .child(hint_block("chart.candles.zone_hint", p, cx))
                .child(hide_row)
                .child(hint_block("chart.candles.hide_hint", p, cx))
                .child(price_lines_cb)
                .child(wicks_cb)
                .child(neutral_cb)
                .child(neutral_swatches)
                .into_any_element(),
        ))
        .into_any_element()
}

/// Хозяин попапа свечей (полоска вкладок / шапка выносного окна). Цель — активная
/// вкладка (полоска) либо панель окна; применение/persist — через [`LayoutPopupHost`]
/// (`apply_tab_setting`), «ко всем» — своя реализация у каждого хозяина.
pub(super) trait CandlePopupHost: LayoutPopupHost {
    fn candle_popup_open(&self) -> bool;
    fn set_candle_popup_open(&mut self, open: bool);
    fn candle_popup_hovered(&self) -> bool;
    fn set_candle_popup_hovered(&mut self, hovered: bool);
    /// Per-вкладочный override цели (None = следует глобальному дефолту).
    fn candle_view_override(&self, cx: &App) -> Option<CandleViewCfg>;
    /// «Применить ко всем»: раздать набор всем вкладкам/окнам + обновить глобальный дефолт.
    fn apply_candle_view_all(&mut self, cfg: CandleViewCfg, cx: &mut Context<Self>);

    /// Эффективный набор цели: свой override, иначе глобальный дефолт из layout.
    fn candle_view_current(&self, cx: &App) -> CandleViewCfg {
        self.candle_view_override(cx)
            .unwrap_or(self.backend().read(cx).layout.candle_view)
    }

    /// Применить набор к цели (стек(и) + persist в спек вкладки).
    fn apply_candle_view(&mut self, cfg: CandleViewCfg, cx: &mut Context<Self>) {
        self.apply_tab_setting(StackSetting::CandleView(cfg), cx);
    }

    fn toggle_candle_popup(&mut self, cx: &mut Context<Self>) {
        let open = !self.candle_popup_open();
        self.set_candle_popup_open(open);
        self.set_candle_popup_hovered(false);
        cx.notify();
    }

    fn close_candle_popup(&mut self, cx: &mut Context<Self>) {
        if !self.candle_popup_open() {
            return;
        }
        self.set_candle_popup_open(false);
        self.set_candle_popup_hovered(false);
        cx.notify();
    }
}

/// Оверлей попапа свечей (позиционируемая сцена с hover-закрытием). None, если закрыт.
pub(super) fn candle_popup_overlay<T: CandlePopupHost>(
    this: &T,
    id_prefix: &'static str,
    top: Pixels,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.candle_popup_open() {
        return None;
    }
    let p = MoonPalette::active(cx);
    let cfg = this.candle_view_current(cx);
    let neutral = {
        let b = this.backend().read(cx);
        let eff = b.preview.as_ref().unwrap_or(&b.config);
        eff.theme.get(p.is_light()).candle_neutral
    };
    let entity = cx.entity();
    let hover_entity = entity.clone();
    let popup_w = content_width(cx);
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
                        this.set_candle_popup_hovered(true);
                    } else if this.candle_popup_hovered() {
                        this.close_candle_popup(cx);
                    }
                });
            })
            .child(render_candle_popup(id_prefix, entity, cfg, neutral, p, cx)),
    )
}

/// Слой-перехватчик клика вне попапа свечей (закрыть). None, если закрыт.
pub(super) fn candle_popup_dismiss<T: CandlePopupHost>(
    this: &T,
    id_prefix: &'static str,
    cx: &mut Context<T>,
) -> Option<Stateful<Div>> {
    if !this.candle_popup_open() {
        return None;
    }
    let entity = cx.entity();
    Some(
        div()
            .id(SharedString::from(format!("{id_prefix}-popup-dismiss")))
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |_, _window, app| {
                entity.update(app, |this, cx| this.close_candle_popup(cx));
                app.stop_propagation();
            }),
    )
}
