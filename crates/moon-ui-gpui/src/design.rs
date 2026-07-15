//! Moonbot terminal design tokens extracted from the Moonbot Terminal design reference.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_ui::{MoonMetrics, MoonPalette, MoonTheme, rgba_from};
use std::sync::Arc;

const M: MoonMetrics = MoonMetrics::TERMINAL;

pub const HEADER_TOP_H: f32 = M.header_top_h;
pub const TOOLBAR_H: f32 = M.toolbar_h;
pub const STATUS_H: f32 = M.status_h;
pub const TABLE_HEAD_H: f32 = M.table_header_h;
pub const TABLE_ROW_H: f32 = M.table_row_h;
pub const HEADER_PAD_X: f32 = 12.0;

/// Transparent macOS titlebars keep native traffic-light buttons over the client
/// area. Keep terminal chrome content and drag hitboxes out of that strip.
pub fn titlebar_leading_inset() -> f32 {
    if cfg!(target_os = "macos") {
        76.0
    } else {
        HEADER_PAD_X
    }
}

pub fn show_custom_window_controls() -> bool {
    !cfg!(target_os = "macos")
}

pub fn platform_window_decorations() -> Option<WindowDecorations> {
    if cfg!(target_os = "linux") {
        Some(WindowDecorations::Client)
    } else {
        None
    }
}

const LOGO_GLOW_SVG_RAW: &str = include_str!("../../../assets/brand/moonbot-logo.svg");
const LOGO_SRC_W: f32 = 199.0;
const LOGO_SRC_H: f32 = 43.0;
const LOGO_GLOW_VIEW_W: f32 = LOGO_SRC_W * 1.2;
const LOGO_GLOW_VIEW_H: f32 = LOGO_SRC_W * 1.2;

pub fn solid(hex: u32) -> Rgba {
    rgb(hex)
}

/// Hex-токен палитры (`0xRRGGBB`) → непрозрачный `Hsla`. Единый хелпер: до
/// рефактора дублировался в `screener/table.rs`, `panels/alerts.rs` и
/// `strategies/mod.rs`.
pub fn moon(hex: u32) -> Hsla {
    rgba_from(hex, 1.0)
}

/// То же, но с альфой.
pub fn moon_alpha(hex: u32, alpha: f32) -> Hsla {
    rgba_from(hex, alpha)
}

/// Палитра/конфиг хранят цвета как `[u8; 3]`; GPUI-API берёт `0xRRGGBB`. Единый
/// источник пары конвертеров: до рефактора `u32_to_rgb` дублировался в detects и
/// connections, а обратный `rgb_to_u32` жил отдельной `fn hex` в корне бинарника.
pub fn u32_to_rgb(c: u32) -> [u8; 3] {
    [
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    ]
}

pub fn rgb_to_u32(c: [u8; 3]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// Hsla (из MoonColorPicker) → sRGB `[u8;3]` — для конфигов/полей, хранящих
/// цвет байтами (fig_style, hex-поля стратегий).
pub fn hsla_to_rgb8(h: Hsla) -> [u8; 3] {
    let c: Rgba = h.into();
    [
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
    ]
}

pub fn mono() -> SharedString {
    SharedString::from("Geist Mono")
}

pub fn ui_font() -> SharedString {
    SharedString::from("Inter")
}

pub fn ui_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).ui(value)
}

pub fn font_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).font(value)
}

pub fn line_value(cx: &App, value: f32) -> f32 {
    MoonTheme::active_tokens(cx).line_height(value)
}

pub fn fit_h_value(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> f32 {
    MoonTheme::active_tokens(cx).fit_height(base_height, base_line_height, base_pad_y)
}

pub fn ui_px(cx: &App, value: f32) -> Pixels {
    px(ui_value(cx, value))
}

pub fn text_px(cx: &App, value: f32) -> Pixels {
    px(font_value(cx, value))
}

pub fn line_px(cx: &App, value: f32) -> Pixels {
    px(line_value(cx, value))
}

/// Базовый кегль текста из темы moonui (`mono_font_size`, по умолчанию 11).
/// Все три ступени ниже считаются от него, поэтому смена базы в `.toml`
/// двигает их разом.
fn base_text(cx: &App) -> f32 {
    MoonTheme::active_tokens(cx).typography.mono_font_size
}

/// Три стандартные ступени кегля терминала — ТОЛЬКО для сырого gpui
/// (`div().text_size(..)`), где нет компонента moonui и масштабировать некому.
/// Считаются от базы темы moonui и проходят через `font()` (см. `text_px`),
/// поэтому реагируют на слайдер «Шрифт» в Настройках.
///
/// У компонентов moonui (`MoonText`, `MoonButtonSegment`, `MoonDataCell`) свой
/// кегль по умолчанию, и они САМИ прогоняют его через `tokens.font()`. Им сюда
/// ничего передавать не надо: не задавать `font_size` вообще, а если нужен не
/// дефолт — передавать базовое число. `t_*`/`font_value(..)` туда передавать
/// НЕЛЬЗЯ — масштаб применится дважды.
///
/// `t_caption` ~9: бейджи, мелкие подписи, счётчики.
pub fn t_caption(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) - 2.0)
}

/// `t_body` 11: основной текст, таблицы, моно-значения. База темы.
pub fn t_body(cx: &App) -> Pixels {
    text_px(cx, base_text(cx))
}

/// `t_title` ~14: заголовки и крупные акценты.
pub fn t_title(cx: &App) -> Pixels {
    text_px(cx, base_text(cx) + 3.0)
}

pub fn fit_h_px(cx: &App, base_height: f32, base_line_height: f32, base_pad_y: f32) -> Pixels {
    px(fit_h_value(cx, base_height, base_line_height, base_pad_y))
}

/// Фактические (масштабированные под кегль) высоты строки/шапки MoonDataTable.
/// Зеркалят fit_height-аргументы самого компонента (data_table.rs: строка
/// `fit(row_h, 14.0, 5.5)`, шапка `fit(header_h, 11.0, 7.5)`), чтобы обёртки,
/// считающие «натуральную» высоту таблицы, не отставали от неё при крупном
/// шрифте (иначе строки клипались при +6).
pub fn table_row_h(cx: &App) -> f32 {
    fit_h_value(cx, TABLE_ROW_H, 14.0, 5.5)
}

pub fn table_head_h(cx: &App) -> f32 {
    fit_h_value(cx, TABLE_HEAD_H, 11.0, 7.5)
}

/// Масштаб текущего кегля относительно базового (1.0 при нулевой дельте
/// слайдера «Шрифт»). Геометрия через `ui()` дельту слайдера НЕ видит —
/// поэтому фиксированные ширины, держащие текст (поля значений, попапы,
/// инпуты), надо умножать на этот коэффициент, иначе крупный шрифт
/// обрезается/переносится.
pub fn font_scale(cx: &App) -> f32 {
    let b = base_text(cx);
    font_value(cx, b) / b
}

/// Ширина, растущая вместе с кеглем: `base` при нулевой дельте слайдера.
/// Для контейнеров текста фиксированной ширины (см. `font_scale`).
pub fn font_w_px(cx: &App, base: f32) -> Pixels {
    px(base * font_scale(cx))
}

/// То же, но `f32` — для билдеров moonui, берущих сырые пиксели БЕЗ
/// собственного масштабирования (`menu_width`/`trigger_width`/`MoonButton::width`
/// кладут значение в `px(..)` как есть — проверено по форку).
pub fn font_w(cx: &App, base: f32) -> f32 {
    base * font_scale(cx)
}

/// Скругления — из метрик moonui (`MoonMetrics::TERMINAL`), не свои числа.
/// Сырых `px(N)` в `rounded()` не писать. Пилюли (`SEL_H / 2.0`, `999.0`) — не
/// радиус, а форма, сюда не входят.
///
/// `*_BASE` — базовое (немасштабированное) число для билдеров moonui, которые
/// скейлят сами (`MoonButtonSize::Custom { radius }`); `r_*` — готовые `Pixels`
/// для сырого gpui `.rounded()`. Путать нельзя: масштаб применится дважды.
///
/// ВНИМАНИЕ: у moonui всего ДВЕ ступени радиуса. Для мелких чипов/свотчей своей
/// ступени у неё нет — см. `docs-internal/FORK_BUGS.md` (запрос `radius_sm`).
pub const R_BUTTON_BASE: f32 = M.button_radius;
pub const R_CONTAINER_BASE: f32 = M.container_radius;

/// `r_button` (moonui `button_radius`, 4): кнопки, карточки, попапы, панели.
pub fn r_button(cx: &App) -> Pixels {
    ui_px(cx, R_BUTTON_BASE)
}

/// `r_container` (moonui `container_radius`, 8): диалоги, модалки, контейнеры.
pub fn r_container(cx: &App) -> Pixels {
    ui_px(cx, R_CONTAINER_BASE)
}

/// Брендовый тёмно-синий словесного знака «Moonbot» (брендбук) — для СВЕТЛОЙ темы.
/// Раньше буквы красились в `p.text` (near-black на светлом фоне) → логотип выглядел
/// чёрным, а не фирменным navy. На тёмной теме navy не читался бы — там оставляем `p.text`.
const LOGO_WORDMARK_NAVY: u32 = 0x0C2C4A;

pub fn logo_glow_sized(cx: &App, width: f32) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let text = if p.is_light() {
        LOGO_WORDMARK_NAVY
    } else {
        p.text
    };
    let text_fill = format!("#{text:06X}");
    let logo =
        LOGO_GLOW_SVG_RAW.replace(r##"fill="#E7E7E7""##, &format!(r##"fill="{text_fill}""##));
    let paths = logo
        .split_once(r#"<g clip-path="url(#clip0_3800_3393)">"#)
        .and_then(|(_, rest)| rest.split_once("</g>"))
        .map(|(paths, _)| paths)
        .unwrap_or("");
    let cx = LOGO_GLOW_VIEW_W * 0.5;
    let cy = LOGO_GLOW_VIEW_H * 0.5;
    let r = LOGO_GLOW_VIEW_W * 0.5;
    let logo_x = (LOGO_GLOW_VIEW_W - LOGO_SRC_W) * 0.5;
    let logo_y = (LOGO_GLOW_VIEW_H - LOGO_SRC_H) * 0.5;
    let (aura_0_color, aura_1_color, aura_2_color, aura_0, aura_1, aura_2) = if p.is_light() {
        (
            "#BFF5C9",
            "#AEEFC1",
            "#98E8B2",
            0.30 * 0.5 / 3.0,
            0.19 * 0.5 / 3.0,
            0.07 * 0.5 / 3.0,
        )
    } else {
        ("#00BCFF", "#1A76FF", "#0A5CFF", 0.30, 0.19, 0.07)
    };
    let svg = format!(
        r##"<svg width="{view_w}" height="{view_h}" viewBox="0 0 {view_w} {view_h}" fill="none" xmlns="http://www.w3.org/2000/svg">
<defs>
  <radialGradient id="moonbot_aura" cx="50%" cy="50%" r="50%">
    <stop offset="0%" stop-color="{aura_0_color}" stop-opacity="{aura_0:.3}"/>
    <stop offset="34%" stop-color="{aura_1_color}" stop-opacity="{aura_1:.3}"/>
    <stop offset="68%" stop-color="{aura_2_color}" stop-opacity="{aura_2:.3}"/>
    <stop offset="100%" stop-color="{aura_2_color}" stop-opacity="0"/>
  </radialGradient>
</defs>
<circle cx="{cx}" cy="{cy}" r="{r}" fill="url(#moonbot_aura)"/>
<g transform="translate({logo_x} {logo_y})">{paths}</g>
</svg>"##,
        view_w = LOGO_GLOW_VIEW_W,
        view_h = LOGO_GLOW_VIEW_H,
        cx = cx,
        cy = cy,
        r = r,
        logo_x = logo_x,
        logo_y = logo_y,
        aura_0_color = aura_0_color,
        aura_1_color = aura_1_color,
        aura_2_color = aura_2_color,
        aura_0 = aura_0,
        aura_1 = aura_1,
        aura_2 = aura_2,
        paths = paths,
    );
    let frame_w = width * (LOGO_GLOW_VIEW_W / 199.0);
    img(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        svg.into_bytes(),
    )))
    .w(px(frame_w))
    .h(px(frame_w * (LOGO_GLOW_VIEW_H / LOGO_GLOW_VIEW_W)))
}

pub fn vline(height: f32, p: MoonPalette) -> impl IntoElement {
    div().w(px(1.0)).h(px(height)).bg(rgb(p.border))
}

pub fn status_dot(color: u32, cx: &App) -> impl IntoElement {
    div()
        .w(ui_px(cx, 5.0))
        .h(ui_px(cx, 5.0))
        .rounded(ui_px(cx, 999.0))
        .bg(solid(color))
}
