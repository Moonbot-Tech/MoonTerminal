//! Единое контекстное меню монеты (ПКМ). Применяется в разных точках, где пользователь
//! видит монету — линия ордера на чарте, ячейка токена/стратегии в таблице ордеров (и
//! позже Активы/Отчёт). Меню САМО показывает нужные пункты по наличию контекста в
//! [`CoinMenuCtx`]: стратегия (`strat_id`), ордер (`order_uid`/`side`), набор «выбранных
//! ядер» из фильтра панели-источника.
//!
//! Все действия — «прочитать текущий список → дописать монету (дедуп) → отправить целиком»:
//! отдельной команды «добавить одну монету» в moonproto нет ни для глобального ЧС ядра
//! (`set_blacklist`), ни для ЧС стратегии (поле `CoinsBlackList` через `edit_strategies`).

use gpui::*;
use moon_ui::{MoonContextMenuWindowExt as _, MoonMenuItem, MoonTone, MoonWindowExt as _};
use rust_i18n::t;

use moon_core::session::CoreId;

use crate::Backend;

/// Имя поля стратегии moonproto с чёрным списком монет (`FIELD_COINS_BLACK_LIST`). Строка,
/// формат = список монет через запятую (как глобальный ЧС ядра).
const FIELD_COINS_BLACK_LIST: &str = "CoinsBlackList";

/// Ширина всплывающего меню (px). Чуть шире ордерного (170) — влезают подписи «В глобальный
/// ЧС ядра «…»».
const MENU_WIDTH: f32 = 220.0;

/// Сторона ноги ордера в точке клика. На чарте различается по виду линии (Buy/Sell),
/// определяет набор ордерных пунктов: Buy → «Отменить», Sell → «Join all sells» + «Split».
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Откуда открыто меню. Влияет только на мелочи (навигацию «Открыть на графике» не
/// показываем, если уже на чарте).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CoinMenuOrigin {
    OrderTable,
    ChartLine,
}

/// Контекст точки клика по монете. Собирается вызывающим (у каждого места — свой набор
/// доступных данных); меню решает по наличию полей, какие пункты показать.
pub struct CoinMenuCtx {
    pub core: CoreId,
    /// Имя ядра-источника (для подписи и пункта «ЧС ядра «name»»).
    pub core_name: String,
    /// Рынок ордера/монеты (`ADAUSDT`) — для навигации/join/split.
    pub market: String,
    /// Базовая монета (`ADA`) — именно она пишется в списки ЧС.
    pub coin: String,
    /// Набор ядер из фильтра панели-источника (для «ЧС выбранных ядер»). Пункт показывается
    /// только если тут > 1 ядра. Для чарт-линии = `[core]` (пункт не появляется).
    pub selected_cores: Vec<CoreId>,
    /// Стратегия ордера (`strat_id != 0`) — включает секцию стратегии. `None` = ручной/join.
    pub strat_id: Option<u64>,
    /// Имя стратегии (для подписи пункта). `None` → id в подписи.
    pub strat_name: Option<String>,
    /// uid ордера — включает секцию ордера. `None` = точка без ордера (тикер монеты).
    pub order_uid: Option<u64>,
    /// Сторона (для чарт-линии). `None` в таблице → секция ордера = «Редактировать» + «Отменить».
    pub side: Option<OrderSide>,
    /// Направление позиции ордера (для `join_sells`).
    pub short: bool,
    pub origin: CoinMenuOrigin,
}

/// Открыть единое контекстное меню монеты в позиции `pos` (координаты окна, обычно
/// `event.position`).
pub fn open_coin_menu(
    ctx: CoinMenuCtx,
    backend: Entity<Backend>,
    pos: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let items = build_items(&ctx, &backend, cx);
    window.open_moon_context_menu(cx, "coin-context-menu", pos, items, MENU_WIDTH);
}

/// Сборка пунктов по контексту. Читает состояние ядра (`cx`) для галок «уже в ЧС» и гейта
/// пункта стратегии по схеме.
fn build_items(ctx: &CoinMenuCtx, backend: &Entity<Backend>, cx: &App) -> Vec<MoonMenuItem> {
    let b = backend.read(cx);
    let core = ctx.core;
    let coin = ctx.coin.clone();
    let mut items: Vec<MoonMenuItem> = Vec::new();

    // — Навигация ———————————————————————————————————————————————— (нужен рынок; на чарте
    // монета уже открыта — «Открыть» не дублируем; балансовые строки без рынка — пропускаем)
    let has_market = !ctx.market.is_empty();
    if has_market && ctx.origin != CoinMenuOrigin::ChartLine {
        let backend_open = backend.clone();
        let market = ctx.market.clone();
        items.push(
            MoonMenuItem::with_key("coin-open", t!("coin_menu.open").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_open.update(app, |b, bcx| {
                        b.open_request = Some((core, market.clone()));
                        b.open_request_rev = b.open_request_rev.wrapping_add(1);
                        b.open_request_activate = false;
                        bcx.notify();
                    });
                },
            ),
        );
    }
    if has_market {
        let backend_cmp = backend.clone();
        let market = ctx.market.clone();
        items.push(
            MoonMenuItem::with_key("coin-compare", t!("coin_menu.compare").to_string()).on_click(
                move |_, window, app| {
                    window.close_context_menu(app);
                    backend_cmp.update(app, |b, bcx| {
                        b.open_compare_request = Some((core, market.clone()));
                        b.open_compare_request_rev = b.open_compare_request_rev.wrapping_add(1);
                        bcx.notify();
                    });
                },
            ),
        );
    }

    // — Чёрный список ————————————————————————————————————————————
    if !items.is_empty() {
        items.push(MoonMenuItem::separator());
    }

    // (1) Глобальный ЧС текущего ядра.
    let (_, cur_text) = core_blacklist(b, core);
    let in_core = blacklist_contains(&cur_text, &coin);
    {
        let backend_bl = backend.clone();
        let coin_c = coin.clone();
        items.push(
            MoonMenuItem::with_key(
                "coin-bl-core",
                t!("coin_menu.bl_core", core = ctx.core_name.clone()).to_string(),
            )
            .checked(in_core)
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_bl.update(app, |b, _| add_to_core_blacklist(b, core, &coin_c));
            }),
        );
    }

    // (2) Глобальный ЧС всех выбранных ядер (фильтр панели) — только если их > 1.
    if ctx.selected_cores.len() > 1 {
        let cores = ctx.selected_cores.clone();
        let all_in = cores
            .iter()
            .all(|&c| blacklist_contains(&core_blacklist(b, c).1, &coin));
        let backend_m = backend.clone();
        let coin_m = coin.clone();
        items.push(
            MoonMenuItem::with_key(
                "coin-bl-cores",
                t!("coin_menu.bl_cores", n = cores.len()).to_string(),
            )
            .checked(all_in)
            .on_click(move |_, window, app| {
                window.close_context_menu(app);
                backend_m.update(app, |b, _| {
                    for &c in &cores {
                        add_to_core_blacklist(b, c, &coin_m);
                    }
                });
            }),
        );
    }

    // (3) ЧС стратегии ордера — только если стратегия известна И её схема содержит поле
    // `CoinsBlackList` (иначе правка молча потерялась бы — поле не в редакторе вида).
    if let Some(sid) = ctx.strat_id {
        if strategy_has_blacklist_field(b, core, sid) {
            let in_strat = blacklist_contains(&strategy_blacklist(b, core, sid), &coin);
            let label = match ctx.strat_name.as_deref().filter(|n| !n.is_empty()) {
                Some(name) => t!("coin_menu.bl_strategy", name = name.to_string()).to_string(),
                None => t!("coin_menu.bl_strategy", name = sid.to_string()).to_string(),
            };
            let backend_s = backend.clone();
            let coin_s = coin.clone();
            items.push(
                MoonMenuItem::with_key("coin-bl-strat", label)
                    .checked(in_strat)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_s
                            .update(app, |b, _| add_to_strategy_blacklist(b, core, sid, &coin_s));
                    }),
            );
        }
    }

    // — Стратегия ————————————————————————————————————————————————
    if let Some(sid) = ctx.strat_id {
        items.push(MoonMenuItem::separator());
        let backend_g = backend.clone();
        items.push(
            MoonMenuItem::with_key("coin-strat-goto", t!("coin_menu.strategy_goto").to_string())
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    let owner_display = window.display(app).map(|d| d.id());
                    crate::strategies::open_goto(
                        backend_g.clone(),
                        core,
                        sid,
                        Some(window.window_handle()),
                        owner_display,
                        app,
                    );
                }),
        );
    }

    // — Ордер ————————————————————————————————————————————————————
    if let Some(uid) = ctx.order_uid {
        items.push(MoonMenuItem::separator());
        let backend_e = backend.clone();
        items.push(
            MoonMenuItem::with_key("coin-order-edit", t!("chart.order_menu.edit").to_string())
                .on_click(move |_, window, app| {
                    window.close_context_menu(app);
                    crate::panels::open_order_edit(backend_e.clone(), core, uid, window, app);
                }),
        );
        match ctx.side {
            Some(OrderSide::Sell) => {
                let backend_j = backend.clone();
                let market_j = ctx.market.clone();
                let short = ctx.short;
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-join",
                        t!("chart.order_menu.join_sells").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_j.update(app, |b, _| {
                            let _ = b.session.join_sells(core, market_j.clone(), short);
                        });
                    }),
                );
                let backend_sp = backend.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-split",
                        t!("chart.order_menu.split").to_string(),
                    )
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_sp.update(app, |b, _| {
                            let _ = b.session.split_order(core, uid, SPLIT_PARTS);
                        });
                    }),
                );
            }
            // Buy-линия чарта ИЛИ таблица (side=None) → отмена ордера целиком по uid.
            Some(OrderSide::Buy) | None => {
                let backend_c = backend.clone();
                items.push(
                    MoonMenuItem::with_key(
                        "coin-order-cancel",
                        t!("chart.order_menu.cancel").to_string(),
                    )
                    .tone(MoonTone::Danger)
                    .on_click(move |_, window, app| {
                        window.close_context_menu(app);
                        backend_c.update(app, |b, _| {
                            let _ = b.session.cancel_order(core, uid);
                        });
                    }),
                );
            }
        }
    }

    items
}

/// На сколько частей делит «Split order» (как на чарт-линии, Moonbot по умолчанию — 2).
const SPLIT_PARTS: i32 = 2;

// ————————————————————— helpers: чтение/запись ЧС —————————————————————

/// Текущий глобальный ЧС ядра: `(включён, текст-список)`. Нет снимка настроек → `(false, "")`.
fn core_blacklist(b: &Backend, core: CoreId) -> (bool, String) {
    b.session
        .store()
        .core(core)
        .and_then(|cd| cd.client_settings.as_ref())
        .map(|cs| (cs.use_blacklist, cs.blacklist_text.clone()))
        .unwrap_or((false, String::new()))
}

/// Дописать монету в глобальный ЧС ядра и ВКЛЮЧИТЬ список (иначе добавление не влияет на
/// торговлю — команда шлёт флаг+текст целиком). Идемпотентно: если монета уже есть,
/// шлём тот же текст, но всё равно включаем ЧС.
fn add_to_core_blacklist(b: &Backend, core: CoreId, coin: &str) {
    let (_, text) = core_blacklist(b, core);
    let new = blacklist_add(&text, coin);
    if let Err(err) = b.session.set_blacklist(core, true, new) {
        log::warn!("coin_menu: add {coin} to core {core} blacklist failed: {err:#}");
    }
}

/// Текущее значение поля `CoinsBlackList` стратегии (read-only снимок). Поля, равные
/// дефолту схемы, сервер НЕ шлёт → тогда пусто (и мы просто создаём список из одной монеты).
fn strategy_blacklist(b: &Backend, core: CoreId, sid: u64) -> String {
    b.session
        .store()
        .core(core)
        .and_then(|cd| cd.strategies.iter().find(|s| s.id == sid))
        .and_then(|s| {
            s.fields
                .iter()
                .find(|(k, _)| k == FIELD_COINS_BLACK_LIST)
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_default()
}

/// Содержит ли схема вида стратегии (по её `kind_ordinal`) поле `CoinsBlackList`. Если нет —
/// правка поля молча не применилась бы, поэтому пункт скрываем.
fn strategy_has_blacklist_field(b: &Backend, core: CoreId, sid: u64) -> bool {
    let Some(cd) = b.session.store().core(core) else {
        return false;
    };
    let Some(row) = cd.strategies.iter().find(|s| s.id == sid) else {
        return false;
    };
    let Some(schema) = cd.schema.as_ref() else {
        return false;
    };
    schema
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)
        .is_some_and(|k| {
            k.sections
                .iter()
                .any(|s| s.fields.iter().any(|f| f.name == FIELD_COINS_BLACK_LIST))
        })
}

/// Дописать монету в ЧС стратегии (поле `CoinsBlackList`) через общий редактор полей.
fn add_to_strategy_blacklist(b: &Backend, core: CoreId, sid: u64, coin: &str) {
    let cur = strategy_blacklist(b, core, sid);
    let new = blacklist_add(&cur, coin);
    let edits = vec![(sid, vec![(FIELD_COINS_BLACK_LIST.to_string(), new)])];
    if let Err(err) = b.session.edit_strategies(core, edits) {
        log::warn!("coin_menu: add {coin} to strategy {sid}@{core} blacklist failed: {err:#}");
    }
}

/// Есть ли монета в списке (запятая-разделённом, без учёта регистра/пробелов).
fn blacklist_contains(text: &str, coin: &str) -> bool {
    text.split(',').any(|s| s.trim().eq_ignore_ascii_case(coin))
}

/// Дописать монету в запятая-разделённый список (дедуп без регистра). Уже есть → без
/// изменений. Пустой список → одна монета.
fn blacklist_add(text: &str, coin: &str) -> String {
    if blacklist_contains(text, coin) {
        return text.to_string();
    }
    let base = text.trim().trim_end_matches(',').trim_end();
    if base.is_empty() {
        coin.to_string()
    } else {
        format!("{base},{coin}")
    }
}

#[cfg(test)]
mod tests;
