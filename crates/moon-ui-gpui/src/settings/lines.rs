//! Вкладка «Линии» — стиль ордер-линий (порт egui `settings/lines.rs`): сворачиваемый
//! блок на каждый вид линии (Buy/Sell/Stop/…) + Path + Global. НАЗВАНИЯ линий —
//! трейдинг-термины, остаются англ. (как в оригинале); атрибуты/общие подписи локализованы
//! (`locales/lines.yml`). Правки идут в draft (живое превью), «Сохранить» — orders.toml.
//! Состояние редактора — [`Lines`]; раскрытость блоков живёт в `SettingsView.open_lines`.

use gpui::*;
use moon_ui::{
    MoonAccordion, MoonCheckboxSize, MoonColorPicker, MoonColorPickerState, MoonPalette,
    MoonSliderState, StyledExt, h_flex, v_flex,
};

use super::{SettingsView, separator, slider_row};
use crate::Backend;
use moon_core::config::{OrdersStyle, UiThemeMode};
use rust_i18n::t;

/// Чекбокс ордер-стиля: (id, локализованная подпись, геттер, сеттер) — для тела блока линии.
type Check = (
    &'static str,
    String,
    fn(&OrdersStyle) -> bool,
    fn(&mut OrdersStyle, bool),
);

/// Редактор одной ордер-линии: цвет + слайдеры (маркеры/прозрачность используются не всеми).
struct LineEd {
    color: Entity<MoonColorPickerState>,
    thickness: Entity<MoonSliderState>,
    marker_size: Entity<MoonSliderState>,
    marker_thickness: Entity<MoonSliderState>,
    knot_size: Entity<MoonSliderState>,
    /// Прозрачность невыставленного (fill=0) — показывается только у `buy`/`buy_short`.
    pending_alpha: Entity<MoonSliderState>,
    /// Цвет невыставленного (fill=0) — показывается только у `buy`/`buy_short`.
    pending_color: Entity<MoonColorPickerState>,
}

/// Color-picker поля OrdersStyle активной темы (`is_light`) — пишет в draft.orders[тема].
fn ord_color(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
    is_light: bool,
    get: fn(&OrdersStyle) -> [u8; 3],
    set: fn(&mut OrdersStyle, [u8; 3]),
) -> Entity<MoonColorPickerState> {
    let cur = get(backend.read(cx).config.orders.get(is_light));
    super::draft_color(window, cx, cur, move |p, c| {
        if get(p.orders.get(is_light)) != c {
            set(p.orders.get_mut(is_light), c);
            true
        } else {
            false
        }
    })
}

/// Слайдер f32 поля OrdersStyle активной темы (`is_light`) — пишет в draft.orders[тема].
#[allow(clippy::too_many_arguments)]
fn ord_slider(
    backend: &Entity<Backend>,
    cx: &mut Context<SettingsView>,
    is_light: bool,
    get: fn(&OrdersStyle) -> f32,
    set: fn(&mut OrdersStyle, f32),
    min: f32,
    max: f32,
    step: f32,
) -> Entity<MoonSliderState> {
    let cur = get(backend.read(cx).config.orders.get(is_light));
    super::draft_slider(cx, min, max, step, cur, move |p, f, _bcx| {
        if get(p.orders.get(is_light)) != f {
            set(p.orders.get_mut(is_light), f);
            true
        } else {
            false
        }
    })
}

/// Строит [`LineEd`] для поля `$line` OrdersStyle (fn-ptr аксессоры).
macro_rules! line_ed {
    ($b:expr, $w:expr, $cx:expr, $il:expr, $line:ident) => {
        LineEd {
            color: ord_color($b, $w, $cx, $il, |o| o.$line.color, |o, v| o.$line.color = v),
            thickness: ord_slider(
                $b,
                $cx,
                $il,
                |o| o.$line.thickness,
                |o, v| o.$line.thickness = v,
                0.5,
                6.0,
                0.1,
            ),
            marker_size: ord_slider(
                $b,
                $cx,
                $il,
                |o| o.$line.marker_size,
                |o, v| o.$line.marker_size = v,
                2.0,
                24.0,
                0.5,
            ),
            marker_thickness: ord_slider(
                $b,
                $cx,
                $il,
                |o| o.$line.marker_thickness,
                |o, v| o.$line.marker_thickness = v,
                0.5,
                5.0,
                0.1,
            ),
            knot_size: ord_slider(
                $b,
                $cx,
                $il,
                |o| o.$line.knot_size,
                |o, v| o.$line.knot_size = v,
                1.0,
                10.0,
                0.5,
            ),
            pending_alpha: ord_slider(
                $b,
                $cx,
                $il,
                |o| o.$line.pending_alpha,
                |o, v| o.$line.pending_alpha = v,
                0.0,
                1.0,
                0.01,
            ),
            pending_color: ord_color(
                $b,
                $w,
                $cx,
                $il,
                |o| o.$line.pending_color.unwrap_or(o.$line.color),
                |o, v| o.$line.pending_color = Some(v),
            ),
        }
    };
}

/// Состояние редактора ордер-линий.
pub(super) struct Lines {
    /// Тема, набор которой сейчас редактируется (по активной теме приложения при открытии
    /// Настроек): true = светлая, false = тёмная. Нужна `ord_check` для записи в нужный набор.
    is_light: bool,
    buy: LineEd,
    buy_short: LineEd,
    sell: LineEd,
    sell_short: LineEd,
    stop: LineEd,
    trailing: LineEd,
    take_profit: LineEd,
    vstop: LineEd,
    pending_cond: LineEd,
    liq: LineEd,
    path_color: Entity<MoonColorPickerState>,
    path_thickness: Entity<MoonSliderState>,
    active_alpha: Entity<MoonSliderState>,
    closed_alpha: Entity<MoonSliderState>,
    max_closed: Entity<MoonSliderState>,
}

/// Собрать редактор ордер-линий из текущего draft (зовётся из `SettingsView::new`).
pub(super) fn build(
    backend: &Entity<Backend>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> Lines {
    // Редактируем набор линий АКТИВНОЙ темы приложения (по `ui_theme_mode`). Сменить тему
    // (вкладка Interface) + сохранить → откроется набор другой темы.
    let is_light = backend.read(cx).config.ui_theme_mode == UiThemeMode::Light;
    Lines {
        is_light,
        buy: line_ed!(backend, window, cx, is_light, buy),
        buy_short: line_ed!(backend, window, cx, is_light, buy_short),
        sell: line_ed!(backend, window, cx, is_light, sell),
        sell_short: line_ed!(backend, window, cx, is_light, sell_short),
        stop: line_ed!(backend, window, cx, is_light, stop),
        trailing: line_ed!(backend, window, cx, is_light, trailing),
        take_profit: line_ed!(backend, window, cx, is_light, take_profit),
        vstop: line_ed!(backend, window, cx, is_light, vstop),
        pending_cond: line_ed!(backend, window, cx, is_light, pending_cond),
        liq: line_ed!(backend, window, cx, is_light, liq),
        path_color: ord_color(
            backend,
            window,
            cx,
            is_light,
            |o| o.path.color,
            |o, v| o.path.color = v,
        ),
        path_thickness: ord_slider(
            backend,
            cx,
            is_light,
            |o| o.path.thickness,
            |o, v| o.path.thickness = v,
            0.5,
            6.0,
            0.1,
        ),
        active_alpha: ord_slider(
            backend,
            cx,
            is_light,
            |o| o.active_alpha,
            |o, v| o.active_alpha = v,
            0.05,
            1.0,
            0.01,
        ),
        closed_alpha: ord_slider(
            backend,
            cx,
            is_light,
            |o| o.closed_alpha,
            |o, v| o.closed_alpha = v,
            0.0,
            1.0,
            0.01,
        ),
        max_closed: ord_slider(
            backend,
            cx,
            is_light,
            |o| o.max_closed_orders as f32,
            |o, v| o.max_closed_orders = v as u32,
            0.0,
            5000.0,
            50.0,
        ),
    }
}

impl SettingsView {
    /// Checkbox булева поля OrdersStyle (пишет в draft.orders, notify групп+view).
    fn ord_check(
        &self,
        cx: &Context<Self>,
        id: &'static str,
        label: &str,
        get: fn(&OrdersStyle) -> bool,
        set: fn(&mut OrdersStyle, bool),
    ) -> impl IntoElement {
        let is_light = self.lines.is_light;
        let cur = {
            let b = self.backend.read(cx);
            get(b.preview.as_ref().unwrap_or(&b.config).orders.get(is_light))
        };
        self.draft_checkbox(cx, id, cur, move |p, v| {
            if get(p.orders.get(is_light)) != v {
                set(p.orders.get_mut(is_light), v);
                true
            } else {
                false
            }
        })
        .label(label)
        .size(MoonCheckboxSize::Compact)
    }

    /// Сворачиваемый блок на компоненте MoonUI `MoonAccordion` (один item на ключ): заголовок
    /// с шевроном + тело. Раскрытость хранится во `SettingsView.open_lines[key]`; клик по
    /// заголовку переключает её через `on_toggle_click` (для single-item: открыт ⇔ ix `[0]`).
    fn collapse_section(
        &self,
        cx: &Context<Self>,
        key: &'static str,
        title: &str,
        body: AnyElement,
        // `use<>`: возвращаемый элемент НЕ заимствует `title` (сразу копируем в owned SharedString).
        // Без этого Rust 2024 синтаксически «захватывает» лайфтайм `&str` в `impl IntoElement`,
        // и временный `&t!(..)` из вложенного блока (Path) ловит E0716.
    ) -> impl IntoElement + use<> {
        let open = self.open_lines.contains(key);
        let title: SharedString = title.to_string().into();
        let entity = cx.entity();
        MoonAccordion::new(SharedString::from(format!("lines-acc-{key}")))
            .item(move |item| item.title(title).open(open).child(body))
            .on_toggle_click(move |open_ixs, _window, cx| {
                let now_open = !open_ixs.is_empty();
                entity.update(cx, |this, c| {
                    let changed = if now_open {
                        this.open_lines.insert(key)
                    } else {
                        this.open_lines.remove(key)
                    };
                    if changed {
                        c.notify();
                    }
                });
            })
    }

    /// Тело блока ордер-линии (порт egui `line_block`): цвет+толщина в строке, `dashed`,
    /// и при `markers` — маркеры начала/конца, размер/толщина креста, узлы, размер узла.
    /// `checks` = `[dashed]` или `[dashed, start, end, knots]`.
    fn line_body(
        &self,
        cx: &Context<Self>,
        ed: &LineEd,
        markers: bool,
        pending: bool,
        checks: &[Check],
    ) -> AnyElement {
        let p = MoonPalette::active(cx);
        let chk = |idx: usize| -> AnyElement {
            match checks.get(idx) {
                Some((id, label, get, set)) => {
                    self.ord_check(cx, id, label, *get, *set).into_any_element()
                }
                None => div().into_any_element(),
            }
        };
        let mut col = v_flex()
            .w_full()
            .gap_1()
            .pl_4()
            .child(
                h_flex()
                    .gap(px(10.0))
                    .items_center()
                    .child(MoonColorPicker::new(&ed.color))
                    .child(slider_row(&t!("lines.thickness"), &ed.thickness, cx)),
            )
            .child(chk(0));
        // Состояние «выставлен, но не исполнен» (fill=0) — только у входных линий buy/buy_short:
        // отдельный цвет + прозрачность. Цвет пустой = брать основной цвет линии.
        if pending {
            col = col
                .child(
                    div()
                        .text_size(crate::design::t_caption(cx))
                        .text_color(rgb(p.text_soft))
                        .child(t!("lines.pending_group").to_string()),
                )
                .child(
                    h_flex()
                        .gap(px(10.0))
                        .items_center()
                        .child(MoonColorPicker::new(&ed.pending_color))
                        .child(slider_row(&t!("lines.pending_alpha"), &ed.pending_alpha, cx)),
                );
        }
        if markers {
            col = col
                .child(separator(p, cx))
                .child(chk(1))
                .child(chk(2))
                .child(slider_row(&t!("lines.cross_size"), &ed.marker_size, cx))
                .child(slider_row(
                    &t!("lines.cross_thickness"),
                    &ed.marker_thickness,
                    cx,
                ))
                .child(chk(3))
                .child(slider_row(&t!("lines.knot_size"), &ed.knot_size, cx));
        }
        col.into_any_element()
    }

    /// Сворачиваемый блок линии: заголовок + тело (видно при раскрытии).
    fn line_section(
        &self,
        cx: &Context<Self>,
        key: &'static str,
        title: &str,
        ed: &LineEd,
        markers: bool,
        pending: bool,
        checks: &[Check],
    ) -> impl IntoElement {
        let body = self.line_body(cx, ed, markers, pending, checks);
        self.collapse_section(cx, key, title, body)
    }

    /// Вкладка «Линии»: «Order lines», по сворачиваемому блоку на вид линии (названия —
    /// англ. трейдинг-термины), затем «Path» и «Global». Атрибуты — из `locales/lines.yml`.
    pub(super) fn lines_tab(&self, cx: &Context<Self>) -> impl IntoElement {
        let l = &self.lines;
        v_flex()
            .w_full()
            .gap_1()
            .child(div().font_bold().child("Order lines"))
            .child(self.line_section(
                cx,
                "buy",
                "Buy",
                &l.buy,
                true,
                true,
                &[
                    (
                        "buy-d",
                        t!("lines.dashed").to_string(),
                        |o| o.buy.dashed,
                        |o, v| o.buy.dashed = v,
                    ),
                    (
                        "buy-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.buy.start_marker,
                        |o, v| o.buy.start_marker = v,
                    ),
                    (
                        "buy-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.buy.end_marker,
                        |o, v| o.buy.end_marker = v,
                    ),
                    (
                        "buy-k",
                        t!("lines.knots").to_string(),
                        |o| o.buy.knots,
                        |o, v| o.buy.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "buy-short",
                "Buy (short)",
                &l.buy_short,
                true,
                true,
                &[
                    (
                        "buyshort-d",
                        t!("lines.dashed").to_string(),
                        |o| o.buy_short.dashed,
                        |o, v| o.buy_short.dashed = v,
                    ),
                    (
                        "buyshort-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.buy_short.start_marker,
                        |o, v| o.buy_short.start_marker = v,
                    ),
                    (
                        "buyshort-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.buy_short.end_marker,
                        |o, v| o.buy_short.end_marker = v,
                    ),
                    (
                        "buyshort-k",
                        t!("lines.knots").to_string(),
                        |o| o.buy_short.knots,
                        |o, v| o.buy_short.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "sell",
                "Sell",
                &l.sell,
                true,
                false,
                &[
                    (
                        "sell-d",
                        t!("lines.dashed").to_string(),
                        |o| o.sell.dashed,
                        |o, v| o.sell.dashed = v,
                    ),
                    (
                        "sell-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.sell.start_marker,
                        |o, v| o.sell.start_marker = v,
                    ),
                    (
                        "sell-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.sell.end_marker,
                        |o, v| o.sell.end_marker = v,
                    ),
                    (
                        "sell-k",
                        t!("lines.knots").to_string(),
                        |o| o.sell.knots,
                        |o, v| o.sell.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "sell-short",
                "Sell (short)",
                &l.sell_short,
                true,
                false,
                &[
                    (
                        "sellshort-d",
                        t!("lines.dashed").to_string(),
                        |o| o.sell_short.dashed,
                        |o, v| o.sell_short.dashed = v,
                    ),
                    (
                        "sellshort-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.sell_short.start_marker,
                        |o, v| o.sell_short.start_marker = v,
                    ),
                    (
                        "sellshort-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.sell_short.end_marker,
                        |o, v| o.sell_short.end_marker = v,
                    ),
                    (
                        "sellshort-k",
                        t!("lines.knots").to_string(),
                        |o| o.sell_short.knots,
                        |o, v| o.sell_short.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "stop",
                "Stop",
                &l.stop,
                true,
                false,
                &[
                    (
                        "stop-d",
                        t!("lines.dashed").to_string(),
                        |o| o.stop.dashed,
                        |o, v| o.stop.dashed = v,
                    ),
                    (
                        "stop-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.stop.start_marker,
                        |o, v| o.stop.start_marker = v,
                    ),
                    (
                        "stop-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.stop.end_marker,
                        |o, v| o.stop.end_marker = v,
                    ),
                    (
                        "stop-k",
                        t!("lines.knots").to_string(),
                        |o| o.stop.knots,
                        |o, v| o.stop.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "trailing",
                "Trailing",
                &l.trailing,
                true,
                false,
                &[
                    (
                        "tr-d",
                        t!("lines.dashed").to_string(),
                        |o| o.trailing.dashed,
                        |o, v| o.trailing.dashed = v,
                    ),
                    (
                        "tr-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.trailing.start_marker,
                        |o, v| o.trailing.start_marker = v,
                    ),
                    (
                        "tr-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.trailing.end_marker,
                        |o, v| o.trailing.end_marker = v,
                    ),
                    (
                        "tr-k",
                        t!("lines.knots").to_string(),
                        |o| o.trailing.knots,
                        |o, v| o.trailing.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "tp",
                "Take Profit",
                &l.take_profit,
                true,
                false,
                &[
                    (
                        "tp-d",
                        t!("lines.dashed").to_string(),
                        |o| o.take_profit.dashed,
                        |o, v| o.take_profit.dashed = v,
                    ),
                    (
                        "tp-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.take_profit.start_marker,
                        |o, v| o.take_profit.start_marker = v,
                    ),
                    (
                        "tp-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.take_profit.end_marker,
                        |o, v| o.take_profit.end_marker = v,
                    ),
                    (
                        "tp-k",
                        t!("lines.knots").to_string(),
                        |o| o.take_profit.knots,
                        |o, v| o.take_profit.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "vstop",
                "VStop",
                &l.vstop,
                true,
                false,
                &[
                    (
                        "vs-d",
                        t!("lines.dashed").to_string(),
                        |o| o.vstop.dashed,
                        |o, v| o.vstop.dashed = v,
                    ),
                    (
                        "vs-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.vstop.start_marker,
                        |o, v| o.vstop.start_marker = v,
                    ),
                    (
                        "vs-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.vstop.end_marker,
                        |o, v| o.vstop.end_marker = v,
                    ),
                    (
                        "vs-k",
                        t!("lines.knots").to_string(),
                        |o| o.vstop.knots,
                        |o, v| o.vstop.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "pc",
                "Pending cond",
                &l.pending_cond,
                true,
                false,
                &[
                    (
                        "pc-d",
                        t!("lines.dashed").to_string(),
                        |o| o.pending_cond.dashed,
                        |o, v| o.pending_cond.dashed = v,
                    ),
                    (
                        "pc-s",
                        t!("lines.start_cross").to_string(),
                        |o| o.pending_cond.start_marker,
                        |o, v| o.pending_cond.start_marker = v,
                    ),
                    (
                        "pc-e",
                        t!("lines.end_cross").to_string(),
                        |o| o.pending_cond.end_marker,
                        |o, v| o.pending_cond.end_marker = v,
                    ),
                    (
                        "pc-k",
                        t!("lines.knots").to_string(),
                        |o| o.pending_cond.knots,
                        |o, v| o.pending_cond.knots = v,
                    ),
                ],
            ))
            .child(self.line_section(
                cx,
                "liq",
                "Liquidation",
                &l.liq,
                false,
                false,
                &[(
                    "liq-d",
                    t!("lines.dashed").to_string(),
                    |o| o.liq.dashed,
                    |o, v| o.liq.dashed = v,
                )],
            ))
            .child(separator(MoonPalette::active(cx), cx))
            // Path (trail / змейка) — свой сворачиваемый блок.
            .child({
                let body = v_flex()
                    .w_full()
                    .gap_1()
                    .pl_4()
                    .child(self.ord_check(
                        cx,
                        "path-show",
                        &t!("lines.show_path"),
                        |o| o.path.show,
                        |o, v| o.path.show = v,
                    ))
                    .child(
                        h_flex()
                            .gap(px(10.0))
                            .items_center()
                            .child(MoonColorPicker::new(&l.path_color))
                            .child(slider_row(&t!("lines.thickness"), &l.path_thickness, cx)),
                    )
                    .child(self.ord_check(
                        cx,
                        "path-dash",
                        &t!("lines.dashed"),
                        |o| o.path.dashed,
                        |o, v| o.path.dashed = v,
                    ))
                    .into_any_element();
                self.collapse_section(cx, "path", &t!("lines.path_title"), body)
            })
            .child(separator(MoonPalette::active(cx), cx))
            .child(
                div()
                    .mt_1()
                    .font_bold()
                    .child(t!("lines.global").to_string()),
            )
            .child(slider_row(&t!("lines.active_alpha"), &l.active_alpha, cx))
            .child(slider_row(
                &t!("lines.closed_visibility"),
                &l.closed_alpha,
                cx,
            ))
            .child(self.ord_check(
                cx,
                "pending-dash",
                &t!("lines.pending_dashed"),
                |o| o.pending_dashed,
                |o, v| o.pending_dashed = v,
            ))
            .child(slider_row(&t!("lines.max_closed"), &l.max_closed, cx))
            .child(
                div()
                    .mt_2()
                    .text_color(rgb(MoonPalette::active(cx).text_soft))
                    .child(t!("lines.hint").to_string()),
            )
    }
}
