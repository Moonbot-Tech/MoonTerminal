//! Часы в правом углу шапки: «HH:MM:SS (UTC±N)». Время — текущее UTC + выбранное смещение
//! (пояс), тикает раз в секунду вместе с ре-рендером Shell. Клик по элементу открывает
//! `MoonPopover` со списком поясов (UTC-12..+14 + системный, если дробный); выбор персистится
//! в layout (`Backend::set_header_clock_offset_min`, общий на все окна).
//!
//! Правило метки: если выбранное смещение совпадает с системным поясом (отображаемое время =
//! системным часам), метку «(UTC…)» не показываем — на экране просто локальное время.

use gpui::*;
use moon_ui::{
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonTooltipView, h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;

/// Диапазон поясов в меню (целые часы). Дробные (India +5:30, Nepal +5:45) добавляются
/// отдельно, только если таков системный пояс, — иначе их из целочисленного списка не выбрать.
const TZ_MIN_H: i32 = -12;
const TZ_MAX_H: i32 = 14;

/// Текущее UTC в секундах эпохи (кросс-платформенно, без внешних крейтов).
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// «HH:MM:SS» для текущего UTC + смещение `off_min` (минуты). `rem_euclid` держит корректный
/// час при отрицательных смещениях/переходе через полночь.
fn hms(off_min: i32) -> String {
    let day = (now_unix_secs() + off_min as i64 * 60).rem_euclid(86_400);
    let (h, m, s) = (day / 3600, (day % 3600) / 60, day % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Подпись пояса: «UTC», «UTC+3», «UTC-5:30».
fn tz_label(off_min: i32) -> String {
    if off_min == 0 {
        return "UTC".to_string();
    }
    let sign = if off_min > 0 { '+' } else { '-' };
    let a = off_min.abs();
    let (h, m) = (a / 60, a % 60);
    if m == 0 {
        format!("UTC{sign}{h}")
    } else {
        format!("UTC{sign}{h}:{m:02}")
    }
}

/// Смещение системного пояса от UTC в минутах (восток положителен). Нужно для правила
/// «скрыть метку, если выбранное = системному».
#[cfg(windows)]
fn system_offset_min() -> i32 {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // 0.61: обе возвращают SYSTEMTIME по значению. Вызовы соседние → час/минута стабильны.
    let (utc, loc) = unsafe { (GetSystemTime(), GetLocalTime()) };
    let utc_min = utc.wHour as i32 * 60 + utc.wMinute as i32;
    let loc_min = loc.wHour as i32 * 60 + loc.wMinute as i32;
    let mut diff = loc_min - utc_min;
    // Локальная и UTC-дата могут отличаться (напр. UTC 23:30 ↔ локально 01:30) → развернуть.
    if diff > 14 * 60 {
        diff -= 24 * 60;
    } else if diff < -14 * 60 {
        diff += 24 * 60;
    }
    // Реальные пояса кратны 15 мин; округляем — гасит дрожь на границе минуты между вызовами.
    ((diff as f32 / 15.0).round() as i32) * 15
}

/// Не-Windows: определять системный пояс нечем (moon-chart намеренно windows-free) → считаем
/// систему в UTC. Часы работают, метка «(UTC)» просто не скрывается.
#[cfg(not(windows))]
fn system_offset_min() -> i32 {
    0
}

/// Часы правого угла шапки: время + (опционально) метка пояса, клик — попап выбора пояса.
pub fn header_clock(backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> impl IntoElement {
    let off = backend.read(cx).header_clock_offset_min();
    let sys = system_offset_min();
    // Отображаемое = системному → метку пояса прячем (на экране локальное время).
    let show_tz = off != sys;

    // Список смещений меню: целые часы + системный пояс (если дробный и ещё не в списке), чтобы
    // на India/Nepal можно было прийти к «= системному» и погасить метку.
    let mut offsets: Vec<i32> = (TZ_MIN_H..=TZ_MAX_H).map(|h| h * 60).collect();
    if !offsets.contains(&sys) {
        offsets.push(sys);
        offsets.sort_unstable();
    }

    let mut items = Vec::with_capacity(offsets.len());
    for m in offsets {
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("tz-{m}"), tz_label(m))
                // Время в этом поясе (снимок; пока попап открыт — тикает с ре-рендером Shell).
                .right_label(hms(m))
                .selected(m == off)
                .checked(m == off)
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.set_header_clock_offset_min(m);
                        bcx.notify();
                    });
                }),
        );
    }

    let mut row = h_flex()
        .id("header-clock")
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .cursor_pointer()
        .tooltip(|_w, cx| {
            cx.new(|_| MoonTooltipView::new(t!("header.clock_tip").to_string()))
                .into()
        })
        .child(
            div()
                .text_color(rgb(p.text))
                .font_weight(FontWeight::SEMIBOLD)
                .child(hms(off)),
        );
    if show_tz {
        row = row.child(
            div()
                .text_color(rgb(p.text_soft))
                .child(format!("({})", tz_label(off))),
        );
    }

    MoonPopover::new("header-clock-popover")
        .placement(MoonPopoverPlacement::BottomEnd)
        // 156 меню + 2×6 паддинг попапа + 2 рамка.
        .width(170.0)
        .close_on_content_click(true)
        .trigger(row)
        .content(
            MoonPopupMenu::new("header-clock-menu")
                .width(156.0)
                .size(MoonMenuSize::Compact)
                .mono(true)
                // ~11 пунктов видно, дальше скролл (список из 27+ поясов).
                .max_height(300.0)
                .items(items)
                .render(),
        )
        .into_any_element()
}
