//! MoonBot terminal design tokens extracted from the MoonBot Terminal design reference.
//!
//! This is a thin GPUI-div adapter over MoonPalette tokens. Keep it visual-only:
//! no terminal logic, no chart renderer state.

use gpui::*;
use moon_ui::{MoonMetrics, MoonPalette, MoonTheme};
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

/// Три стандартные ступени кегля терминала. ЕДИНСТВЕННЫЙ источник размеров
/// текста — больше нигде не задаём кегли числом и не используем `.text_xs()`.
/// Всё проходит через `font()` (см. `text_px`), поэтому реагирует на слайдер
/// «Шрифт» в Настройках.
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

pub fn logo_glow_sized(cx: &App, width: f32) -> impl IntoElement {
    let p = MoonPalette::active(cx);
    let text_fill = format!("#{text:06X}", text = p.text);
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
    let aura_alpha = if p.is_light() { 0.5 } else { 1.0 };
    let svg = format!(
        r##"<svg width="{view_w}" height="{view_h}" viewBox="0 0 {view_w} {view_h}" fill="none" xmlns="http://www.w3.org/2000/svg">
<defs>
  <radialGradient id="moonbot_aura" cx="50%" cy="50%" r="50%">
    <stop offset="0%" stop-color="#00BCFF" stop-opacity="{aura_0:.3}"/>
    <stop offset="34%" stop-color="#1A76FF" stop-opacity="{aura_1:.3}"/>
    <stop offset="68%" stop-color="#0A5CFF" stop-opacity="{aura_2:.3}"/>
    <stop offset="100%" stop-color="#0A5CFF" stop-opacity="0"/>
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
        aura_0 = 0.30 * aura_alpha,
        aura_1 = 0.19 * aura_alpha,
        aura_2 = 0.07 * aura_alpha,
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
