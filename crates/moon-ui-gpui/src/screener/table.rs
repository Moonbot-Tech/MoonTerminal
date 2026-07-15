//! Колонки/строки таблицы скринера: схема колонок, форматирование значений,
//! сортировка и рендер строки (`MoonDataRow`/ячейки). Вынесено вербатим из `mod.rs`.

use gpui::*;
use moon_ui::{MoonDataCell, MoonDataRow, MoonPalette, MoonTone};

use moon_core::market::ScreenerRow;
use moon_core::session::CoreId;

use crate::panels::num;

use super::view::ScreenerView;

pub(super) use crate::design::{moon, moon_alpha};

/// Схема колонки: (ключ, заголовок, ширина, числовая/вправо).
pub(super) type ColDef = (&'static str, &'static str, f32, bool);

/// Колонки таблицы. Заголовки — отраслевые токены Moonbot (не переводятся,
/// как колонки Orders/Report).
pub(super) const COLS: &[ColDef] = &[
    ("market", "Market", 92.0, false),
    ("core", "Core", 76.0, false),
    ("vol24", "Vol.", 66.0, true),
    ("hvol", "H.vol", 66.0, true),
    ("vol1m", "1m vol", 62.0, true),
    ("vol3m", "3m vol", 62.0, true),
    ("vol5m", "5m vol", 62.0, true),
    ("ask", "ASK", 88.0, true),
    ("high1h", "H.High", 88.0, true),
    ("maxord", "Max.Order", 68.0, true),
    ("d24h", "24h Delta", 68.0, true),
    ("d3h", "3h.Delta", 62.0, true),
    ("d1h", "h.Delta", 62.0, true),
    ("d15m", "15m Delta", 68.0, true),
    ("d1m", "1m Delta", 62.0, true),
    ("d72h", "72h Delta", 66.0, true),
    ("funding", "Funding", 66.0, true),
    ("markd", "MarkPrice", 68.0, true),
    ("lev", "Leverage", 104.0, false),
    ("step", "PriceStep", 92.0, true),
    ("orders", "Orders", 54.0, true),
    ("session", "Session", 76.0, true),
    ("pos", "Pos", 66.0, true),
];

/// Строка таблицы: данные moon-core + имя ядра для колонки Core и ядро,
/// с которым открывать чарт (при фильтре по ядру — оно, иначе провайдер).
pub(super) struct Entry {
    pub(super) row: ScreenerRow,
    pub(super) core_name: SharedString,
    pub(super) open_core: CoreId,
}

/// Мин. объём из строки фильтра DVol: «500» → 500, «500k» → 500 000, «2m» → 2 000 000.
pub(super) fn parse_vol(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let lower = s.to_lowercase();
    let (num_part, mult) = match lower.strip_suffix(['k', 'к']) {
        Some(n) => (n, 1_000.0),
        None => match lower.strip_suffix(['m', 'м']) {
            Some(n) => (n, 1_000_000.0),
            None => (lower.as_str(), 1.0),
        },
    };
    num_part.trim().replace(',', ".").parse::<f64>().unwrap_or(0.0) * mult
}

/// Короткий объёмный формат Moonbot: 1 712 345 → «1.7m», 320 100 → «320k».
fn vol_fmt(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}b", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}m", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.0}k", v / 1e3)
    } else if a > 0.0 {
        format!("{v:.0}")
    } else {
        "0".to_string()
    }
}

pub(super) fn sort_entries(entries: &mut [Entry], key: &str, desc: bool) {
    // Строковые колонки сортируем по имени, числовые — по значению; NaN вниз.
    let cmp_str = |a: &Entry, b: &Entry, f: fn(&Entry) -> &str| f(a).cmp(f(b));
    entries.sort_by(|a, b| {
        let ord = match key {
            "market" => cmp_str(a, b, |e| &e.row.market),
            "core" => cmp_str(a, b, |e| &e.core_name),
            "lev" => (lev_sort(a)).total_cmp(&lev_sort(b)),
            _ => num_key(a, key).total_cmp(&num_key(b, key)),
        };
        if desc { ord.reverse() } else { ord }
    });
}

fn lev_sort(e: &Entry) -> f64 {
    if e.row.leverage_x > 0 {
        f64::from(e.row.leverage_x) + 10_000.0
    } else {
        f64::from(e.row.max_leverage)
    }
}

fn num_key(e: &Entry, key: &str) -> f64 {
    let r = &e.row;
    match key {
        "vol24" => r.vol_24h,
        "hvol" => r.vol_1h,
        "vol1m" => r.vol_1m,
        "vol3m" => r.vol_3m,
        "vol5m" => r.vol_5m,
        "ask" => r.ask,
        "high1h" => r.high_1h,
        "maxord" => r.max_order,
        "d24h" => r.d_24h,
        "d3h" => r.d_3h,
        "d1h" => r.d_1h,
        "d15m" => r.d_15m,
        "d1m" => r.d_1m,
        "d72h" => r.d_72h,
        "funding" => r.funding_pct,
        "markd" => r.mark_delta_pct.unwrap_or(0.0),
        "step" => r.price_step,
        "orders" => f64::from(r.orders),
        "session" => r.session_pnl,
        "pos" => r.pos_size,
        _ => 0.0,
    }
}

/// Тон знаковой величины: плюс — зелёный, минус — красный, ноль — приглушённый.
fn signed_tone(v: f64) -> MoonTone {
    if v > 0.0 {
        MoonTone::Positive
    } else if v < 0.0 {
        MoonTone::Danger
    } else {
        MoonTone::Muted
    }
}

pub(super) fn screener_row(
    e: &Entry,
    view: &Entity<ScreenerView>,
    p: MoonPalette,
    cols: &[&ColDef],
) -> MoonDataRow {
    let r = &e.row;
    let pct = |v: f64| format!("{v:.1}%");
    // Беззнаковые derived-дельты: 0 → мьют, иначе обычный текст.
    let delta_cell = |v: f64| {
        if v != 0.0 {
            MoonDataCell::text(pct(v))
        } else {
            MoonDataCell::text("0.0%").tone(MoonTone::Muted)
        }
    };
    let vol_cell = |v: f64| {
        if v > 0.0 {
            MoonDataCell::text(vol_fmt(v))
        } else {
            MoonDataCell::text("0").tone(MoonTone::Muted)
        }
    };
    let cells: Vec<MoonDataCell> = cols
        .iter()
        .map(|&&(key, ..)| match key {
            "market" => MoonDataCell::element(market_cell(e, view, p)),
            "core" => MoonDataCell::text(e.core_name.clone()).tone(MoonTone::Muted),
            "vol24" => vol_cell(r.vol_24h),
            "hvol" => vol_cell(r.vol_1h),
            "vol1m" => vol_cell(r.vol_1m),
            "vol3m" => vol_cell(r.vol_3m),
            "vol5m" => vol_cell(r.vol_5m),
            "ask" => MoonDataCell::text(num(r.ask)),
            "high1h" => {
                if r.high_1h > 0.0 {
                    MoonDataCell::text(num(r.high_1h))
                } else {
                    MoonDataCell::text("—").tone(MoonTone::Muted)
                }
            }
            "maxord" => vol_cell(r.max_order),
            "d24h" => MoonDataCell::text(format!("{:+.1}%", r.d_24h)).tone(signed_tone(r.d_24h)),
            "d3h" => delta_cell(r.d_3h),
            "d1h" => MoonDataCell::text(format!("{:+.1}%", r.d_1h)).tone(signed_tone(r.d_1h)),
            "d15m" => delta_cell(r.d_15m),
            "d1m" => delta_cell(r.d_1m),
            "d72h" => delta_cell(r.d_72h),
            "funding" => {
                // Как в Moonbot: минусовый фандинг зелёный, плюсовый оранжевый.
                let tone = if r.funding_pct < 0.0 {
                    MoonTone::Positive
                } else if r.funding_pct > 0.0 {
                    MoonTone::Negative
                } else {
                    MoonTone::Muted
                };
                MoonDataCell::text(format!("{:+.2}%", r.funding_pct)).tone(tone)
            }
            "markd" => match r.mark_delta_pct {
                Some(v) => MoonDataCell::text(format!("{v:+.1}%")).tone(signed_tone(v)),
                None => MoonDataCell::text("—").tone(MoonTone::Muted),
            },
            "lev" => {
                let text = if r.leverage_x > 0 {
                    let mode = match r.isolated {
                        Some(true) => " Isolated",
                        Some(false) => " Cross",
                        None => "",
                    };
                    format!("{} / {}{}", r.leverage_x, r.max_leverage, mode)
                } else if r.max_leverage > 0 {
                    format!("{}", r.max_leverage)
                } else {
                    "—".to_string()
                };
                MoonDataCell::text(text).tone(if r.leverage_x > 0 {
                    MoonTone::Default
                } else {
                    MoonTone::Muted
                })
            }
            "step" => {
                // «0.01% / 0.001» — шаг цены в % от ask и абсолютом (как Moonbot).
                if r.price_step > 0.0 && r.ask > 0.0 {
                    MoonDataCell::text(format!(
                        "{:.2}% / {}",
                        r.price_step / r.ask * 100.0,
                        num(r.price_step)
                    ))
                    .tone(MoonTone::Muted)
                } else {
                    MoonDataCell::text("—").tone(MoonTone::Muted)
                }
            }
            "orders" => {
                if r.orders > 0 {
                    MoonDataCell::text(r.orders.to_string()).tone(MoonTone::Accent)
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            "session" => {
                if r.session_pnl != 0.0 {
                    MoonDataCell::text(format!("{:+.2}$", r.session_pnl))
                        .tone(signed_tone(r.session_pnl))
                } else {
                    MoonDataCell::text("0$").tone(MoonTone::Muted)
                }
            }
            "pos" => {
                if r.pos_size != 0.0 {
                    MoonDataCell::text(num(r.pos_size))
                } else {
                    MoonDataCell::text("0").tone(MoonTone::Muted)
                }
            }
            _ => MoonDataCell::text(""),
        })
        .collect();
    MoonDataRow::new(cells)
}

/// Ячейка монеты: кликабельна целиком, клик открывает чарт на Main активной вкладки.
fn market_cell(e: &Entry, view: &Entity<ScreenerView>, p: MoonPalette) -> impl IntoElement + 'static {
    let market = e.row.market.clone();
    let core = e.open_core;
    let view = view.clone();
    div()
        .id(SharedString::from(format!("scr-mkt-{core}-{market}")))
        .w_full()
        .h_full()
        .flex()
        .items_center()
        .cursor_pointer()
        // Кегль/шрифт наследуются от стиля ячейки (каскад moonui, фикс `9a33dbf`).
        .text_color(rgb(MoonTone::Accent.color(p)))
        .font_weight(FontWeight::MEDIUM)
        .child(e.row.coin.clone())
        .on_click(move |_, _window, app| {
            view.update(app, |this, cx| {
                this.backend.update(cx, |b, bcx| {
                    b.open_request = Some((core, market.clone()));
                    b.open_request_rev = b.open_request_rev.wrapping_add(1);
                    b.open_request_activate = false;
                    bcx.notify();
                });
            });
        })
}
