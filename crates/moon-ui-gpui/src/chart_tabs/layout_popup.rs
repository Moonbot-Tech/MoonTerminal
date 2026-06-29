//! In-scene попап настроек раскладки чарт-вкладки: режим (Fit/Scroll) + высота ТОЛЬКО
//! активного режима. Per-tab. Рендер общий для полоски вкладок главного окна и шапки
//! выносного окна; обработчики (применение к нужному стеку + persist) задаёт вызывающий.
//!
//! Семантика: Fit=0 → растяжение (делят окно); Fit≥20 → COMPRESS (фикс. высота без скролла);
//! Scroll → фикс. высота слота + скролл. Допустимый диапазон высоты — [MIN_H, MAX_H].

use gpui::*;
use moon_ui::{
    MoonAccent, MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize,
    MoonInput, MoonInputState, MoonPalette, MoonSegmentItem, MoonSegmentedControl, h_flex, v_flex,
};
use rust_i18n::t;

use crate::chart_persist::{ChartBtnPos, PriceAxisPos, StackLayoutMode, StackOrientation};
use crate::design;

/// Порядок режимов в сегмент-контроле попапа (два положения).
pub(super) const POPUP_MODES: [StackLayoutMode; 2] =
    [StackLayoutMode::Fit, StackLayoutMode::Scroll];

/// Порядок позиций в селекторе кнопок действий: «—»=скрыть, L=слева, C=центр, R=справа.
const BTN_POSITIONS: [ChartBtnPos; 4] = [
    ChartBtnPos::Hide,
    ChartBtnPos::Left,
    ChartBtnPos::Center,
    ChartBtnPos::Right,
];

fn pos_label(p: ChartBtnPos) -> &'static str {
    match p {
        ChartBtnPos::Hide => "—",
        ChartBtnPos::Left => "L",
        ChartBtnPos::Center => "C",
        ChartBtnPos::Right => "R",
    }
}

/// Строка-селектор позиции кнопки действия: подпись слева + сегмент-контрол [— L C R].
fn pos_selector_row(
    id: String,
    caption: &str,
    current: ChartBtnPos,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(ChartBtnPos, &mut App) + 'static,
) -> impl IntoElement {
    let sel = BTN_POSITIONS.iter().position(|x| *x == current).unwrap_or(3);
    let items: Vec<MoonSegmentItem> = BTN_POSITIONS
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut it = MoonSegmentItem::new("", pos_label(*x)).width(30.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(x) = BTN_POSITIONS.get(ix) {
                on_pick(*x, cx);
            }
        })
        .render();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption.to_string()),
        )
        .child(seg)
}

/// Порядок положений оси цен в селекторе: «—»=скрыть, L=слева, R=справа (за стаканом).
const AXIS_POSITIONS: [PriceAxisPos; 3] =
    [PriceAxisPos::Hide, PriceAxisPos::Left, PriceAxisPos::Right];

fn axis_label(p: PriceAxisPos) -> &'static str {
    match p {
        PriceAxisPos::Hide => "—",
        PriceAxisPos::Left => "L",
        PriceAxisPos::Right => "R",
    }
}

/// Строка-селектор положения оси цен: подпись слева + сегмент-контрол [— L R].
fn axis_selector_row(
    id: String,
    caption: String,
    current: PriceAxisPos,
    p: MoonPalette,
    cx: &App,
    on_pick: impl Fn(PriceAxisPos, &mut App) + 'static,
) -> impl IntoElement {
    let sel = AXIS_POSITIONS.iter().position(|x| *x == current).unwrap_or(1);
    let items: Vec<MoonSegmentItem> = AXIS_POSITIONS
        .iter()
        .enumerate()
        .map(|(i, x)| {
            let mut it = MoonSegmentItem::new("", axis_label(*x)).width(30.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(id)
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(x) = AXIS_POSITIONS.get(ix) {
                on_pick(*x, cx);
            }
        })
        .render();
    h_flex()
        .w_full()
        .items_center()
        .gap(design::ui_px(cx, 6.0))
        .child(
            div()
                .flex_1()
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text))
                .child(caption),
        )
        .child(seg)
}

/// Границы высоты слота (px). Меньше MIN (кроме 0 у Fit = растяжение) и больше MAX вводить нельзя.
pub(super) const MIN_H: u16 = 20;
pub(super) const MAX_H: u16 = 4000;

/// Ширина сценового попапа (логич. px). Высоту НЕ считаем: контейнер сжимается по контенту
/// (см. `w_full` у корня + отсутствие `.h(...)` у сцены), поэтому пустого места снизу нет.
/// Ширину держим фиксированной — её определяет сегмент-контрол FIT/SCROLL (2×110) + поля/рамка.
pub(super) fn content_width(cx: &App, _with_rename: bool) -> Pixels {
    let pad = f32::from(design::ui_px(cx, 8.0));
    let fpx = f32::from(design::ui_px(cx, 6.0)); // гор. паддинг рамки (×2)
    let border = 2.0;
    let fb = 2.0; // граница рамки
    px(2.0 * 110.0 + 20.0 + 2.0 * pad + border + 2.0 * fpx + fb)
}

fn mode_label(m: StackLayoutMode) -> &'static str {
    match m {
        StackLayoutMode::Fit => "FIT",
        StackLayoutMode::Scroll => "SCROLL",
    }
}

/// Рамка-группа: тонкая граница + заголовок-капшен сверху, содержимое внутри. Метрики
/// (паддинги/зазоры/граница) ДОЛЖНЫ совпадать с расчётом высоты в [`content_size`].
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

/// Маленькое окошко настроек раскладки. Показывает поле высоты ТОЛЬКО для текущего режима.
/// `height_fit_input`/`height_scroll_input` — раздельные поля (подписку на Blur/Enter держит
/// вызывающий). `on_pick_mode` вызывается при выборе режима. Позиционируется вызывающим.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_layout_popup<F, G, H, I, J, K, L, M, N, O, P2, Q2, R2>(
    id: &str,
    current: StackLayoutMode,
    orientation: StackOrientation,
    rename_input: Option<&Entity<MoonInputState>>,
    height_fit_input: &Entity<MoonInputState>,
    height_scroll_input: &Entity<MoonInputState>,
    orderbook_enabled: bool,
    liquidations_enabled: bool,
    show_zone: bool,
    auto_pin: bool,
    cancel_buy_pos: ChartBtnPos,
    panic_sell_pos: ChartBtnPos,
    price_axis_pos: PriceAxisPos,
    time_axis_visible: bool,
    line_labels: bool,
    cursor_labels: bool,
    p: MoonPalette,
    cx: &App,
    on_pick_mode: F,
    apply_all_label: String,
    on_apply_all: G,
    on_toggle_orderbook: H,
    on_toggle_liquidations: R2,
    on_toggle_show_zone: I,
    on_toggle_auto_pin: J,
    on_toggle_orientation: K,
    on_pick_cancel_pos: L,
    on_pick_panic_pos: M,
    on_pick_price_axis: N,
    on_toggle_time_axis: O,
    on_toggle_line_labels: P2,
    on_toggle_cursor_labels: Q2,
) -> AnyElement
where
    F: Fn(StackLayoutMode, &mut App) + 'static,
    G: Fn(&mut App) + 'static,
    H: Fn(bool, &mut App) + 'static,
    I: Fn(bool, &mut App) + 'static,
    J: Fn(bool, &mut App) + 'static,
    K: Fn(&mut App) + 'static,
    L: Fn(ChartBtnPos, &mut App) + 'static,
    M: Fn(ChartBtnPos, &mut App) + 'static,
    N: Fn(PriceAxisPos, &mut App) + 'static,
    O: Fn(bool, &mut App) + 'static,
    P2: Fn(bool, &mut App) + 'static,
    Q2: Fn(bool, &mut App) + 'static,
    R2: Fn(bool, &mut App) + 'static,
{
    let horizontal = orientation.is_horizontal();
    let sel = POPUP_MODES.iter().position(|m| *m == current).unwrap_or(0);
    let items: Vec<MoonSegmentItem> = POPUP_MODES
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut it = MoonSegmentItem::new("", mode_label(*m)).width(110.0);
            if i == sel {
                it = it.selected(true);
            }
            it
        })
        .collect();
    let seg = MoonSegmentedControl::new(format!("{id}-mode"))
        .accent(MoonAccent::Blue)
        .items(items)
        .on_click(move |ix, _, _, cx| {
            if let Some(m) = POPUP_MODES.get(ix) {
                on_pick_mode(*m, cx);
            }
        })
        .render();

    // Поле + примечание — только для активного режима. При гориз. ориентации значение трактуется
    // как ШИРИНА слота → подписи/хинты берём из *width* ключей (та же логика, диапазон 20..4000).
    let (input, label, hint) = match (current, horizontal) {
        (StackLayoutMode::Fit, false) => (
            height_fit_input,
            t!("chart.layout.height_fit").to_string(),
            t!("chart.layout.height_fit_hint").to_string(),
        ),
        (StackLayoutMode::Fit, true) => (
            height_fit_input,
            t!("chart.layout.width_fit").to_string(),
            t!("chart.layout.width_fit_hint").to_string(),
        ),
        (StackLayoutMode::Scroll, false) => (
            height_scroll_input,
            t!("chart.layout.height_scroll").to_string(),
            t!("chart.layout.height_scroll_hint").to_string(),
        ),
        (StackLayoutMode::Scroll, true) => (
            height_scroll_input,
            t!("chart.layout.width_scroll").to_string(),
            t!("chart.layout.width_scroll_hint").to_string(),
        ),
    };
    // "Высота X  [поле]  px"
    let height_line = h_flex()
        .gap(design::ui_px(cx, 6.0))
        .items_center()
        .child(div().text_color(rgb(p.text)).child(label))
        .child(
            div().w(px(64.0)).child(
                MoonInput::new(SharedString::from(format!("{id}-input")))
                    .state(input)
                    .small(),
            ),
        )
        .child(div().text_color(rgb(p.text_muted)).child("px"));
    // Примечание под полем (многострочное по '\n').
    let hint_block = v_flex().children(hint.split('\n').map(|line| {
        div()
            .text_size(design::t_caption(cx))
            .text_color(rgb(p.text_muted))
            .child(line.to_string())
    }));

    // Чекбокс «Стакан» — вкл/выкл orderbook на графиках вкладки.
    let orderbook_cb = MoonCheckbox::new(SharedString::from(format!("{id}-orderbook")))
        .label(t!("chart.layout.orderbook").to_string())
        .checked(orderbook_enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_orderbook(*ch, app));

    // Чекбокс «Ликвидации» — вкл/выкл кресты трейдов ликвидаций на графиках вкладки.
    let liquidations_cb = MoonCheckbox::new(SharedString::from(format!("{id}-liquidations")))
        .label(t!("chart.layout.liquidations").to_string())
        .checked(liquidations_enabled)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_liquidations(*ch, app));

    // Чекбокс «Отображать зону разделения» — тусклая заливка зоны ордеров при скрытом стакане.
    let show_zone_cb = MoonCheckbox::new(SharedString::from(format!("{id}-show-zone")))
        .label(t!("chart.layout.show_zone").to_string())
        .checked(show_zone)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_show_zone(*ch, app));

    // Чекбокс «Авто-пин при ордере» — закреплять график при выставлении ордера лонг/шорт.
    let auto_pin_cb = MoonCheckbox::new(SharedString::from(format!("{id}-auto-pin")))
        .label(t!("chart.layout.auto_pin").to_string())
        .checked(auto_pin)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_auto_pin(*ch, app));

    // Чекбокс «Ось времени» — вкл/выкл нижние подписи времени на графиках вкладки.
    let time_axis_cb = MoonCheckbox::new(SharedString::from(format!("{id}-time-axis")))
        .label(t!("chart.layout.time_axis").to_string())
        .checked(time_axis_visible)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_time_axis(*ch, app));

    // Чекбокс «Подписи у линий» — вкл/выкл цифры у ордер-линий (размер/%/стоп).
    let line_labels_cb = MoonCheckbox::new(SharedString::from(format!("{id}-line-labels")))
        .label(t!("chart.layout.line_labels").to_string())
        .checked(line_labels)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_line_labels(*ch, app));

    // Чекбокс «Подпись у перекрестия» — вкл/выкл курсорный ридаут (время/цена/%/объём/размер).
    let cursor_labels_cb = MoonCheckbox::new(SharedString::from(format!("{id}-cursor-labels")))
        .label(t!("chart.layout.cursor_labels").to_string())
        .checked(cursor_labels)
        .size(MoonCheckboxSize::Compact)
        .on_change(move |ch: &bool, _w, app| on_toggle_cursor_labels(*ch, app));

    // Селекторы позиции кнопок Cancel Buy / Panic Sell в зоне чарта (— L C R). Названия кнопок —
    // бренд-термины MoonBot, НЕ переводим.
    let cancel_pos_row = pos_selector_row(
        format!("{id}-cancelbuy-pos"),
        "Cancel Buy",
        cancel_buy_pos,
        p,
        cx,
        on_pick_cancel_pos,
    );
    let panic_pos_row = pos_selector_row(
        format!("{id}-panicsell-pos"),
        "Panic Sell",
        panic_sell_pos,
        p,
        cx,
        on_pick_panic_pos,
    );
    // Селектор положения оси цен (— L R): скрыть / слева / справа за стаканом.
    let price_axis_row = axis_selector_row(
        format!("{id}-price-axis-pos"),
        t!("chart.layout.price_axis").to_string(),
        price_axis_pos,
        p,
        cx,
        on_pick_price_axis,
    );

    // Тоггл ориентации стека — рядом с «применить ко всем». «↕» = вертикально (стопка),
    // «↔» = горизонтально (колонки). Клик перестраивает текущее отображение активной вкладки.
    let orientation_btn = MoonButton::new(SharedString::from(format!("{id}-orientation")))
        .label(if horizontal { "↔" } else { "↕" })
        .tooltip(t!("chart.layout.orientation_tip").to_string())
        .size(MoonButtonSize::Micro)
        .variant(if horizontal {
            MoonButtonVariant::Blue
        } else {
            MoonButtonVariant::Ghost
        })
        .selected(horizontal)
        .on_click(move |_, _w, app| on_toggle_orientation(app))
        .render();

    // Иконка «применить ко всем» — справа в строке заголовка, только символ + всплывающая подсказка
    // (текст области: ко всем окнам / только чартам).
    let apply_all_btn = MoonButton::new(SharedString::from(format!("{id}-apply-all")))
        .label("⧉")
        .tooltip(apply_all_label)
        .size(MoonButtonSize::Micro)
        .variant(MoonButtonVariant::Ghost)
        .on_click(move |_, _w, app| on_apply_all(app))
        .render();

    // Поле имени — только для кастомных вкладок (rename_input = Some). Коммит по Blur/Enter
    // держит вызывающий (подписка на инпут).
    let rename_row = rename_input.map(|input| {
        h_flex()
            .gap(design::ui_px(cx, 6.0))
            .items_center()
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .child(t!("chart.tab.rename").to_string()),
            )
            .child(
                div().flex_1().child(
                    MoonInput::new(SharedString::from(format!("{id}-name")))
                        .state(input)
                        .small(),
                ),
            )
    });

    // Контент задаёт ВЫСОТУ сценового контейнера сам (w_full по ширине от `content_size`,
    // высота — по содержимому). Так нет ручного суммирования высоты и пустого места снизу.
    // Фон непрозрачный: если поверх него виден chart text, это z-order баг, а не прозрачность.
    v_flex()
        .id(SharedString::from(format!("{id}-popup")))
        .w_full()
        .p(design::ui_px(cx, 8.0))
        .gap(design::ui_px(cx, 8.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .child(
            // Заголовок слева + иконка «ко всем» прижата к правому краю окна.
            h_flex()
                .w_full()
                .items_center()
                .child(
                    div()
                        .text_size(design::t_caption(cx))
                        .text_color(rgb(p.text_muted))
                        .child(t!("chart.layout.title").to_string()),
                )
                .child(div().flex_1())
                .child(orientation_btn)
                .child(apply_all_btn),
        )
        .children(rename_row)
        // Рамка «Вид»: режим FIT/SCROLL + поле высоты активного режима + описание под ним.
        .child(framed(
            t!("chart.layout.frame_view").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(seg)
                .child(height_line)
                .child(hint_block)
                .into_any_element(),
        ))
        // Рамка «Отображать»: галки видимости (стакан / зона разделения / ось времени).
        .child(framed(
            t!("chart.layout.frame_display").to_string(),
            p,
            cx,
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(orderbook_cb)
                .child(liquidations_cb)
                .child(show_zone_cb)
                .child(time_axis_cb)
                .child(line_labels_cb)
                .child(cursor_labels_cb)
                .into_any_element(),
        ))
        // Остальное (как было): авто-пин + позиции кнопок + ось цен.
        .child(auto_pin_cb)
        .child(cancel_pos_row)
        .child(panic_pos_row)
        .child(price_axis_row)
        .into_any_element()
}

/// Клампинг введённой высоты: Fit допускает 0 (растяжение), иначе [MIN_H, MAX_H]; Scroll — всегда
/// [MIN_H, MAX_H].
pub(super) fn clamp_height(mode: StackLayoutMode, raw: u16) -> u16 {
    match mode {
        StackLayoutMode::Fit if raw == 0 => 0,
        _ => raw.clamp(MIN_H, MAX_H),
    }
}
