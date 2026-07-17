//! Terminal-specific chrome composition over MoonPalette primitives.
//!
//! This is an adapter layer, not a reusable MoonPalette control: it knows about
//! Backend actions and MoonTerminal header content, while generic visuals still
//! come from MoonPalette tokens/components.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonIconSlot, MoonButtonSize, MoonButtonVariant, MoonMenuItem, MoonMenuSize,
    MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu, MoonSelectorPill,
    MoonSelectorSegment, MoonTag, MoonWindowFrame, h_flex,
};
use rust_i18n::t;

use moon_core::feed::ConnStatus;

use crate::shell::Shell;
use crate::{Backend, design};

pub fn header(
    group: &str,
    backend: Entity<Backend>,
    shell: Entity<Shell>,
    ticker_sel: Option<(moon_core::session::CoreId, String)>,
    core_settings_open: bool,
    core_settings_content: Option<AnyElement>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    // Баланс активного торгового ядра группы (серверные значения в USDT). Нет ядра/данных → нули.
    let (free_usdt, total_usdt) = {
        let b = backend.read(cx);
        b.active_trade_core(group)
            .and_then(|c| b.session.store().core(c))
            .map(|cd| (cd.assets.global.free_usdt, cd.assets.global.total_usdt))
            .unwrap_or((0.0, 0.0))
    };
    h_flex()
        .w_full()
        .h(design::fit_h_px(cx, design::HEADER_TOP_H, 14.0, 9.0))
        .pl(design::ui_px(cx, design::titlebar_leading_inset()))
        .pr(design::ui_px(cx, design::HEADER_PAD_X))
        .gap(design::ui_px(cx, 12.0))
        .bg(rgb(p.shell_high))
        .child(
            MoonWindowFrame::main("terminal-header-brand-drag", 0.0)
                .brand_cluster(cx)
                .flex_none()
                .h_full(),
        )
        // Тикер курса (настраиваемый): «1 BTC = 61 333$ +0.1% +2.0%» (дельты 1ч/24ч с ядра).
        .child(ticker_readout(ticker_sel, &backend, shell.clone(), p, cx))
        // Селектор активного ядра (на месте бывшего названия монеты) + баланс. Интерактивные →
        // НЕ drag-зона (иначе клик по селектору таскал бы окно). Монету (`group · market`) убрали.
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 8.0))
                .items_center()
                .child(core_selector(group, &backend, p, cx))
                .child(core_gear_button(
                    shell,
                    core_settings_open,
                    core_settings_content,
                ))
                .child(balance_label(free_usdt, total_usdt, p, cx))
                // Ручная стратегия: разделитель + тогл MS + пикер + сводка (на месте бывших
                // кнопок Стратегии/Скринер — те переехали в тулбар к Live).
                .child(crate::controls::manual_strategy_controls(
                    group, &backend, p, cx,
                )),
        )
        // Метрики Session/Real/Unreal/Risk из шапки убраны: сервер (moonproto) не отдаёт
        // session-профита (его трансляция запрошена у авторов протокола), а остальное
        // решили не показывать. Остался только баланс у селектора ядра.
        .child(
            MoonWindowFrame::main("terminal-header-spacer-drag", 0.0)
                .drag_handle()
                .h_full()
                .flex_1()
                .flex(),
        )
        .child(
            h_flex()
                .flex_none()
                .gap(design::ui_px(cx, 12.0))
                .items_center()
                // Кнопки «Стратегии»/«Скринер»/«Настройки» переехали в тулбар
                // (правый край строки Live).
                // Часы UTC(±пояс) в правом углу; клик — попап выбора пояса.
                .child(crate::clock::header_clock(&backend, p, cx))
                .when(design::show_custom_window_controls(), |this| {
                    this.child(
                        MoonWindowFrame::main("terminal-header-controls", 0.0)
                            .show_controls(true)
                            .visual_controls(cx),
                    )
                }),
        )
}

/// Тикер курса в шапке: «1 BTC = 61 333$ +0.1% +2.0%». Цена и знаковые дельты за 1ч/24ч —
/// с ядра (`MarketDataSource::market_ticker`, moonproto Coin1hDelta/Coin24hDelta). Клик —
/// попап выбора ядра+монеты (хостится Shell); выбор персистится (layout, uid ядра).
fn ticker_readout(
    sel: Option<(moon_core::session::CoreId, String)>,
    backend: &Entity<Backend>,
    shell: Entity<Shell>,
    p: MoonPalette,
    cx: &App,
) -> impl IntoElement {
    let data = sel.as_ref().and_then(|(core, market)| {
        let b = backend.read(cx);
        let t = b.session.market_source().market_ticker(*core, market)?;
        Some((market.clone(), t))
    });
    let base = sel
        .as_ref()
        .map(|(_, market)| moon_core::symbol::coin_of_market(market).to_string())
        .unwrap_or_else(|| "BTC".to_string());
    let delta_span = |v: f64| {
        let color = if v < 0.0 { danger_text(p) } else { positive_text(p) };
        div()
            .text_color(rgb(color))
            .child(format!("{v:+.1}%"))
            .into_any_element()
    };
    let mut row = h_flex()
        .id("header-ticker")
        .flex_none()
        .items_center()
        .gap(design::ui_px(cx, 5.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .cursor_pointer()
        .child(
            div()
                .text_color(rgb(p.text_soft))
                .child(format!("1 {base} =")),
        );
    match data {
        Some((_, t)) => {
            row = row
                .child(
                    div()
                        .text_color(rgb(p.text))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(fmt_ticker_price(t.last)),
                )
                .child(delta_span(t.delta_1h_pct))
                .child(delta_span(t.delta_24h_pct));
        }
        None => {
            row = row.child(div().text_color(rgb(p.text_muted)).child("—"));
        }
    }
    let backend = backend.clone();
    row.tooltip(|_w, cx| {
        cx.new(|_| moon_ui::MoonTooltipView::new(t!("header.ticker_tip").to_string()))
            .into()
    })
    // Клик — открыть чарт монеты тикера на Main (как клик по тикеру в Ордерах/Активах);
    // дабл-клик — попап выбора монеты/ядра. При дабл-клике первый клик успевает открыть
    // чарт — безобидно (окно не поднимаем, activate=false).
    .on_click(move |ev: &ClickEvent, window, cx| {
        if ev.click_count() >= 2 {
            shell.update(cx, |s, cx| s.toggle_ticker_popup(window, cx));
            return;
        }
        let Some((core, market)) = sel.clone() else {
            // Источник не разрезолвлен (нет ядра/рынка) — открывать нечего, ведём в попап.
            shell.update(cx, |s, cx| s.toggle_ticker_popup(window, cx));
            return;
        };
        backend.update(cx, |b, bcx| {
            b.open_request = Some((core, market));
            b.open_request_rev = b.open_request_rev.wrapping_add(1);
            b.open_request_activate = false;
            bcx.notify();
        });
    })
}

/// Цена тикера: тысячи с пробелом и «$» в конце; дробная часть по величине
/// (≥1000 → целое, ≥1 → 2 знака, иначе 4 значащих).
fn fmt_ticker_price(v: f64) -> String {
    if v >= 1000.0 {
        let s = format!("{v:.0}");
        let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
        for (i, ch) in s.chars().enumerate() {
            if i > 0 && (s.len() - i) % 3 == 0 {
                out.push(' ');
            }
            out.push(ch);
        }
        format!("{out}$")
    } else if v >= 1.0 {
        format!("{v:.2}$")
    } else {
        format!("{v:.4}$")
    }
}

fn positive_text(p: MoonPalette) -> u32 {
    if p.is_light() { p.green_text } else { p.green }
}

fn danger_text(p: MoonPalette) -> u32 {
    if p.is_light() { p.red_text } else { p.red }
}

/// Селектор «активного торгового ядра» группы. Список ядер группы; текущий выбор =
/// `Backend::active_trade_core` (авто-следование за фуллскрин-чартом + sticky-override
/// при ручном выборе). Все торговые контролы тулбара/шапки читают это же ядро.
fn core_selector(group: &str, backend: &Entity<Backend>, p: MoonPalette, cx: &App) -> AnyElement {
    // Геометрия пилюли: высота фикс., полное скругление = ½ высоты (ширина — по контенту, как в
    // каноне MoonSelectorPill). `SEL_NAME_MAX` — лимит символов имени (обрезка справа, у пилюли
    // нет overflow-клипа).
    const SEL_H: f32 = 26.0;
    const SEL_NAME_MAX: usize = 13;

    let b = backend.read(cx);
    let cores = b.group_cores(group);
    let active = b.active_trade_core(group);
    let store = b.session.store();

    // Нет ядер в группе — статичная заглушка вместо пустого дропдауна.
    if cores.is_empty() {
        return MoonTag::new()
            .outline()
            .rounded_full()
            .child(design::status_dot(p.text_muted, cx))
            .label(t!("header.no_cores").to_string())
            .into_any_element();
    }

    let active_ready = active
        .and_then(|id| store.core(id))
        .map(|c| c.status == ConnStatus::Ready)
        .unwrap_or(false);
    let dot_color = if active_ready {
        positive_text(p)
    } else {
        danger_text(p)
    };
    let active_name = active
        .and_then(|id| cores.iter().find(|(cid, _)| *cid == id))
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| "—".to_string());
    // У пилюли нет overflow-клипа → длинное имя обрезаем САМИ по символам, оставляя ЛЕВУЮ часть
    // (как просили: «обрезать справа», левый край на месте). Короткие имена остаются как есть;
    // многоточие не добавляем (имя — лишь подпись активного ядра, точный список — в попапе).
    let active_name: String = active_name.chars().take(SEL_NAME_MAX).collect();

    let mut items = Vec::with_capacity(cores.len());
    for (id, name) in &cores {
        let id = *id;
        let backend = backend.clone();
        let group = group.to_string();
        items.push(
            MoonMenuItem::with_key(format!("core-{id}"), name.clone())
                .selected(active == Some(id))
                .checked(active == Some(id))
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        b.set_trade_core_override(&group, id);
                        bcx.notify();
                    });
                }),
        );
    }

    // Каноничный визуал `MoonSelectorPill` (точка статуса со свечением + каретка-иконка) как
    // триггер `MoonPopover`, контент — `MoonPopupMenu` со списком ядер. Всё напрямую из moonui:
    // ни ручной стилизации триггера, ни хака размеров. Popover сам держит open-стейт (внутренний
    // `use_keyed_state`) и тогглит по клику; `close_on_content_click` закрывает после выбора ядра.
    //
    // Фон пилюли = `p.panel`; у `MoonSelectorPill` есть явный бордер `p.border` → «таблетка»
    // читается даже на фоне шапки `shell_high` (в отличие от старого Panel-кейса без рамки).
    MoonPopover::new("header-core-selector")
        .placement(MoonPopoverPlacement::BottomStart)
        .width(192.0) // 180 ширина меню + 2×6 паддинг попапа
        .close_on_content_click(true)
        .trigger(
            MoonSelectorPill::new("header-core-pill")
                .height(SEL_H)
                .radius(SEL_H / 2.0)
                .leading_dot(dot_color)
                .segment(
                    MoonSelectorSegment::new(active_name)
                        .color(p.text)
                        .weight(500.0),
                )
                .render(),
        )
        .content(
            MoonPopupMenu::new("header-core-menu")
                .width(180.0)
                .size(MoonMenuSize::Compact)
                .items(items)
                .render(),
        )
        .into_any_element()
}

/// Кнопка ⚙ настроек ядра + попап настроек (MoonPopover, позиционируется к кнопке —
/// прежний absolute-оверлей с захардкоженными координатами уезжал при изменении состава
/// шапки). Open контролирует Shell (`set_core_settings_open`: сидирует поля при открытии);
/// закрытие по клику вне — сам popover. Кнопка icon-only → квадрат с полями вокруг иконки.
/// Иконка чуть левее центра — баг MoonButton (пустой flex-child при 0 сегментов ломает
/// icon-only режим), записан в FORK_BUGS; чинится в форке, своё не катаем.
fn core_gear_button(
    shell: Entity<Shell>,
    open: bool,
    content: Option<AnyElement>,
) -> impl IntoElement {
    MoonPopover::new("core-gear-popover")
        .placement(MoonPopoverPlacement::BottomStart)
        // 248 (контент) + 2×6 паддинг попапа + 2 рамка.
        .width(262.0)
        .open(open)
        .on_open_change(move |open, window, cx| {
            shell.update(cx, |s, cx| s.set_core_settings_open(open, window, cx));
        })
        .trigger(
            MoonButton::new("core-gear")
                .leading_icon(MoonButtonIconSlot::new("icons/settings-2.svg"))
                .size(MoonButtonSize::Action)
                .variant(MoonButtonVariant::Panel)
                .render(),
        )
        .content(content.unwrap_or_else(|| div().into_any_element()))
}

fn balance_label(free_usdt: f64, total_usdt: f64, p: MoonPalette, cx: &App) -> impl IntoElement {
    h_flex()
        .gap(px(0.0))
        .font_family(design::mono())
        .text_size(design::t_body(cx))
        .text_color(rgb(p.text_soft))
        .child("Balance: ")
        .child(
            div()
                .text_color(rgb(p.text))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{free_usdt:.2}")),
        )
        .child(
            div()
                .text_color(rgb(p.text_muted))
                .child(format!(" /{total_usdt:.0} USDT")),
        )
}

// Бывший чип `header_action` (иконка+подпись, обход pad_x=0 у MoonButton) удалён:
// кнопки Стратегии/Скринер/Настройки переехали в тулбар на MoonButton
// (controls::toolbar::open_window_button).
