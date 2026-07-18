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

/// Палитра для `MoonColorPicker::colors(..)`: 12 оттенков × 5 яркостей + серая
/// шкала (65 свотчей, строка сетки = 5 градаций одного тона). Свободного
/// HSV-пикера у компонента форка НЕТ — без своей палитры он показывает лишь
/// 10 цветов темы, что читалось как «цвет менять нельзя».
pub fn picker_palette() -> Vec<Hsla> {
    let mut out = Vec::with_capacity(65);
    // (насыщенность, светлота): от светлого к тёмному в каждой строке.
    const SHADES: [(f32, f32); 5] = [
        (0.85, 0.72),
        (0.85, 0.58),
        (0.90, 0.46),
        (0.85, 0.34),
        (0.70, 0.24),
    ];
    for hue_step in 0..12 {
        let h = hue_step as f32 / 12.0;
        for (s, l) in SHADES {
            out.push(hsla(h, s, l, 1.0));
        }
    }
    for l in [0.95, 0.75, 0.50, 0.30, 0.10] {
        out.push(hsla(0.0, 0.0, l, 1.0));
    }
    out
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

/// Ширина строки UI-шрифтом темы данного БАЗОВОГО кегля (масштаб «Шрифт» применяется
/// внутри, как это делает `MoonText` — передавать сюда немасштабированное значение).
/// Считается суммой advance'ов глифов через text_system: без кернинга/лигатур, но для
/// подбора ширины попапов/меню под контент этого достаточно. Использовать для
/// контент-зависимой геометрии (ширина меню по самому длинному пункту), НЕ для
/// пиксель-точной вёрстки текста. `mono` — семейство шрифта: `true` для моно (Geist Mono,
/// как меню/тулбар-контекст панелей), `false` для UI-шрифта — мерить надо тем же, каким
/// текст реально рисуется, иначе оценка ширины расходится с рендером.
pub fn ui_text_width(cx: &App, text: &str, base_font_size: f32, weight: f32, mono: bool) -> f32 {
    let tokens = MoonTheme::active_tokens(cx);
    let size = px(tokens.font(base_font_size));
    let font = Font {
        weight: FontWeight(weight),
        ..font(tokens.font_family(mono))
    };
    let ts = cx.text_system();
    let font_id = ts.resolve_font(&font);
    text.chars()
        .map(|ch| f32::from(ts.layout_width(font_id, size, ch)))
        .sum()
}

/// Ширина `MoonPopupMenu` (size=Compact) под самый длинный пункт: у компонента ширина
/// только фиксированная, поэтому подбираем её по контенту сами. Зеркалит метрики
/// Compact-строки меню (dropdown.rs moonui): МОНО-шрифт (`MoonPopupMenu` рисует пункты
/// `.mono(true)`), кегль 9.5/вес до 600 (selected), pad_x 6×2 + gap 5 + паддинг контейнера
/// 4×2 (ui-масштаб) + колонка галки 12 + рамка 2 (не масштабируются). `min_w` — нижняя
/// граница (прежний фикс-вид для коротких списков), `MENU_MAX_W` после масштабирования —
/// верхняя; если верхняя оказалась меньше `min_w`, нижняя граница имеет приоритет.
pub fn menu_fit_width<'a>(
    cx: &App,
    labels: impl IntoIterator<Item = &'a str>,
    min_w: f32,
) -> f32 {
    let max_label_w = labels
        .into_iter()
        .map(|l| ui_text_width(cx, l, 9.5, 600.0, true))
        .fold(0.0, f32::max);
    (ui_value(cx, 6.0 * 2.0 + 5.0 + 4.0 * 2.0) + 12.0 + 2.0 + max_label_w)
        .ceil()
        // Верхняя граница ≥ нижней обязательно: часть вызовов передаёт НЕмасштабированный
        // `min_w` (шапка/MS — сырые 180/200), и при сильно уменьшенном кегле `font_w(MENU_MAX_W)`
        // мог бы оказаться ниже `min_w` → паника `f32::clamp` (lo > hi). `.max(min_w)` держит
        // инвариант: при вырожденном кегле пол побеждает потолок.
        .clamp(min_w, font_w(cx, MENU_MAX_W).max(min_w))
}

/// Outer `MoonPopover::width` that hosts content of intrinsic width `content_w`.
///
/// MoonUI renders `.w(px(width)).p(px(tokens.ui(6.0))).border(px(1.0))`. GPUI treats that
/// width as the border box, leaving `width - 2*ui(6) - 2` for content. The padding tracks
/// the UI scale, while MoonUI's hard-coded 1px border does not.
///
/// `content_w` must already be in the scale its content actually uses — wrap it in `font_w`
/// for our own font-scaled blocks, pass it raw for a moonui widget with a fixed px width.
///
/// Interim (FORK_BUGS): delete when `MoonPopover` grows a fit-to-content mode upstream.
pub fn popover_outer_width(cx: &App, content_w: f32) -> f32 {
    content_w + 2.0 * ui_value(cx, 6.0) + 2.0
}

/// Outer width of a default-size `MoonCalendar` — the value to hand [`popover_outer_width`].
///
/// For default `Size::Medium`, MoonUI's month/year grid is 264px wide, while the day grid is
/// `7*size_9 + 6*gap_0p5` = 16.5rem. The calendar root adds `p_3` (0.75rem per side) and a
/// 1px border per side. `MoonRoot` sets the window rem from `cx.theme().font_size`, which
/// `MoonTheme` synchronizes to `base_font_size()`, so the rem-based day grid can exceed 264px.
///
/// Interim (FORK_BUGS), same removal trigger as [`popover_outer_width`].
pub fn calendar_outer_width(cx: &App) -> f32 {
    // Month/year grid (calendar.rs, Size::Medium) vs the day grid's 7*2.25rem + 6*0.125rem;
    // the root adds p_3 (0.75rem) per side plus the 1px border.
    const MONTH_GRID_W: f32 = 264.0;
    const DAY_GRID_REMS: f32 = 16.5;
    let rem = MoonTheme::active_tokens(cx).base_font_size();
    MONTH_GRID_W.max(DAY_GRID_REMS * rem) + 1.5 * rem + 2.0
}

/// Width of a `MoonPopupMenu` (Compact) row carrying a `MoonMenuItem::right_label`.
///
/// [`menu_fit_width`] reserves one item gap. A row with a right label has four children —
/// check slot, label, flex spacer, and right label — so MoonUI applies three gaps. This helper
/// conservatively measures the main label at the compact size of 9.5 and weight 600; MoonUI
/// renders the right label at size 9.0 and weight 400.
///
/// Takes the already-chosen widest pair rather than an iterator: measuring one string costs an
/// uncached `layout_line` per CHARACTER on `&App`, so the caller picks the candidate. In a mono
/// menu that is simply the longest label by character count.
pub fn menu_fit_width_2col(cx: &App, label: &str, right_label: &str, min_w: f32) -> f32 {
    let content = ui_text_width(cx, label, 9.5, 600.0, true)
        + ui_text_width(cx, right_label, 9.0, 400.0, true);
    (ui_value(cx, 6.0 * 2.0 + 5.0 * 3.0 + 4.0 * 2.0) + 12.0 + 2.0 + content)
        .ceil()
        .clamp(min_w, font_w(cx, MENU_MAX_W).max(min_w))
}

/// Горизонтальный воздух вокруг метки кнопки-триггера `MoonDropdown`: у `MoonButton`
/// pad_x=0 (FORK_BUGS), поэтому поля добавляем сами. Масштабируется под кегль. Визуальная
/// настройка.
const TRIGGER_PAD_X: f32 = 14.0;

/// Потолок ширины кнопки-триггера `MoonDropdown` в плотном тулбаре панели: длинная подпись не
/// должна раздвигать кнопку так, что соседние контролы уезжают за обрезаемый край панели —
/// полное имя читается в раскрытом списке. Визуальная настройка (немасштабированная база
/// для `font_w`).
const TRIGGER_MAX_W: f32 = 260.0;

/// Пол ширины кнопки-триггера селектора ЯДЕР (`core_combo` + `screener::source_combo`) —
/// прежний фикс-вид: ниже кнопка не ужимается, выше растёт под контент до `TRIGGER_MAX_W`.
/// Общий для обоих вызовов `dropdown_content_widths`; у полей панели «Лог» свои полы.
/// Немасштабированная база для `font_w`. Визуальная настройка.
pub const CORES_TRIGGER_MIN_W: f32 = 118.0;

/// Грубый верхний предел ширины меню селектора: не даёт вырожденно-длинному имени ядра
/// раздуть меню на пол-экрана (`MoonDropdown` двигает поповер, но НЕ ужимает его). Это
/// ФИКСИРОВАННЫЙ бортик, НЕ фактическая ширина окна — в очень узком detached-окне при крупном
/// кегле меню всё ещё может выйти за край (как и у шапки, общий предел `menu_fit_width`).
/// Реальные имена ядер сюда не дотягивают. Немасштабированная база для `font_w`.
const MENU_MAX_W: f32 = 560.0;

/// Ширины `MoonDropdown`, растущего под контент — ЕДИНЫЙ расчёт для комбобоксов ядер
/// (`core_combo`, `screener::source_combo`) и полей источника/файла панели «Лог». Возвращает
/// (метка триггера с кареткой, ширина триггера, ширина меню). `cur` — текущий выбор (без
/// каретки), `menu_labels` — ВСЕ подписи пунктов меню, `min_trigger_w` / `min_menu_w` — нижние
/// границы (прежний фикс-вид, у каждого вызова свои). Триггер: пол `min_trigger_w`, потолок
/// `TRIGGER_MAX_W` (при переполнении подпись усекается с «…»); меню между `min_menu_w` и общим
/// верхним пределом `MENU_MAX_W`.
pub fn dropdown_content_widths<'a>(
    cx: &App,
    cur: &str,
    menu_labels: impl IntoIterator<Item = &'a str>,
    min_trigger_w: f32,
    min_menu_w: f32,
) -> (String, f32, f32) {
    let (label, trigger_w) =
        fit_dropdown_trigger(cx, cur, font_w(cx, min_trigger_w), font_w(cx, TRIGGER_MAX_W));
    let menu_w = menu_fit_width(cx, menu_labels, font_w(cx, min_menu_w));
    (label, trigger_w, menu_w)
}

/// Метка (с кареткой ` ▾`) и ширина кнопки-триггера `MoonDropdown` под текущий выбор `cur`.
/// Ширина растёт по контенту между `min_w` (прежний фикс-вид) и `max_w` (потолок). Метка
/// одиночного сегмента `MoonButton` наследует МОНО-шрифт родителя-панели (Geist Mono) —
/// меряем им же (не-моно оценка занижала бы ширину, и длинный текст всё равно резался бы).
/// При превышении потолка метка усекается с «…»: префикс набирается по ФАКТИЧЕСКОЙ ширине
/// символов (устойчиво к fallback-глифам вне Geist Mono — CJK/emoji равноширинными не бывают),
/// каретка сохраняется, полное имя остаётся в раскрытом списке. `cur` передаётся БЕЗ каретки.
fn fit_dropdown_trigger(cx: &App, cur: &str, min_w: f32, max_w: f32) -> (String, f32) {
    const CARET: &str = " \u{25be}"; // пробел + ▾
    let tw = |s: &str| ui_text_width(cx, s, 10.5, 400.0, true);
    let full = format!("{cur}{CARET}");
    let natural = (ui_value(cx, TRIGGER_PAD_X) + tw(&full)).ceil();
    if natural <= max_w {
        return (full, natural.max(min_w));
    }
    // Над потолком → усечь имя, сохранив «… ▾». Имена ядер — произвольный Unicode; для глифов
    // вне Geist Mono равноширинность не гарантирована, поэтому набираем префикс по фактической
    // ширине КАЖДОГО символа, пока «префикс + … ▾» умещается в бюджет — метка не переливает
    // кнопку ни при каком имени.
    let suffix = format!("\u{2026}{CARET}"); // … + пробел + ▾
    let budget = max_w - ui_value(cx, TRIGGER_PAD_X) - tw(&suffix);
    let mut head = String::new();
    let mut used = 0.0f32;
    let mut buf = [0u8; 4];
    for ch in cur.chars() {
        let w = tw(ch.encode_utf8(&mut buf));
        if used + w > budget {
            break;
        }
        used += w;
        head.push(ch);
    }
    (format!("{}{}", head.trim_end(), suffix), max_w)
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
/// ступени у неё нет (третья ступень `radius_sm` запрошена у авторов MoonUI).
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
