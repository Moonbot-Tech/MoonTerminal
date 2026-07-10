//! Единый распознаватель+исполнитель хоткеев для ВСЕХ окон (главное окно группы,
//! выносные окна чартов). Раньше матчинг клавиш жил россыпью `pressed()` только в
//! `Shell::on_hotkey`, из-за чего половина настроенных хоткеев в рантайме не работала,
//! а выносные окна не ловили ничего. Теперь одно место:
//!
//! - [`resolve`] превращает нажатие + конфиг в семантическое [`HotkeyAction`]
//!   (сравнение `modifiers`+`key`, НЕ полный `==Keystroke` — см. [[keystroke-eq-pitfall]]);
//! - [`apply`] исполняет действие относительно ПЕРЕДАННОГО таргета (активное ядро / рынок
//!   активного чарта окна) — так каждое окно работает со своим контекстом. Масштаб (`Scale`)
//!   `apply` не трогает: он свой у каждого окна (rev-механизм группы vs панель выносного окна),
//!   поэтому вызывающий обрабатывает его сам.
//!
//! Кросс-платформенность: дефолты хранятся как `secondary-*` (gpui резолвит в Ctrl на
//! Windows/Linux и Cmd на macOS), так что назначения из коробки работают на обеих ОС.

use gpui::{App, Context, Entity, KeyDownEvent, Keystroke, Modifiers};
use moon_core::config::HotkeysConfig;
use moon_core::feed::ClientSettingsEdit;
use moon_core::figures::FigureTool;
use moon_core::session::CoreId;
use moon_core::session::order_lines::LineKind;

use crate::Backend;

/// Семантическое действие хоткея (независимо от конкретной клавиши в конфиге).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Выбрать пресет размера ордера F1-F6 активного ядра.
    OrderSize(usize),
    /// Задействовать fixed-sell слот S1-S6 (гасит TP).
    SellPreset(usize),
    /// Выбрать i-ю manual-стратегию активного ядра (порядок пикера MS шапки) и включить
    /// режим ручной стратегии — то же, что клик по пункту пикера.
    ManualStrategy(usize),
    /// Отменить ожидающие buy-ордера рынка активного чарта.
    CancelBuy,
    /// Отменить ВСЕ ожидающие buy-ордера активного ядра (по всем рынкам).
    CancelAllBuys,
    /// Тоггл «паник-селл» по рынку активного чарта.
    PanicSell,
    /// Немедленно закрыть позицию рынка активного чарта по рынку (market sell).
    PanicSellOne,
    /// Объединить sell-ордера рынка активного чарта (по стороне позиции).
    JoinSells,
    /// Сдвинуть ордера рынка активного чарта на один шаг цены (`chart_price_step`)
    /// через `move_order`: `sell=false` — входные buy-линии (только незалитые, тот же
    /// гард, что у перетаскивания), `sell=true` — выходные sell-линии.
    ShiftOrder { sell: bool, up: bool },
    /// Разбить sell-ордер рынка активного чарта на части.
    SplitOrder,
    /// Поставить ручной ордер по цене ПОД КУРСОРОМ на чарте (long/short). Исполняет
    /// вызывающее окно через чарт под курсором (`Backend::hovered_chart`) — цену знает
    /// только сам чарт (пиксель Y → цена пейна).
    NewLong,
    NewShort,
    /// Выбрать/тогать инструмент рисования (слой фигур — глобальное состояние).
    FigTool(FigureTool),
    /// Переключить инструмент рисования на следующий по кругу (+вкл режим рисования).
    SwitchFigure,
    /// Переключить активный (fullscreen) чарт Main-стека окна на следующий. Исполняет
    /// вызывающее окно (свой стек у каждого) — как масштаб.
    SwitchCharts,
    /// Удалить выделенную фигуру.
    FigDelete,
    /// Тоггл chart-алерта выделенной фигуры.
    FigAlert,
    /// Закрыть ЧАРТ в фулскрине Main (встроенный Esc). Как в MoonBot: Esc ВСЕГДА закрывает
    /// график, режим рисования при этом НЕ выключается (он тумблерится карандашом). Исполняет
    /// вызывающее окно (Main-группа), в откреп-окне — no-op.
    CloseActiveChart,
    /// Сбросить позиции ВСЕХ окон на экран (встроенный Ctrl+Shift+F10). Исполняет
    /// вызывающее окно (нужен доступ к списку окон приложения).
    ResetWindows,
    /// Отменить ордер ПОД КУРСОРОМ (встроенный Tab/Del над линией ордера). Исполняет
    /// вызывающее окно через чарт под мышью (`Backend::hovered_chart`) — наведённый ордер
    /// знает только сам чарт (`order_hover`). Нет наведённого ордера → не обработано (клавиша
    /// всплывает: Tab остаётся навигацией фокуса).
    CancelHoveredOrder,
    /// Закрыть ВСЕ графики Main-стеков всех групп (встроенный Shift+Esc). Исполняет
    /// вызывающее окно бампом глобальной ревизии — каждый ChartTabs закрывает свой Main.
    CloseAllCharts,
    /// Масштаб Y — зум внутрь. Исполняет вызывающее окно (свой у каждого).
    ScalePlus,
    /// Масштаб Y — зум наружу. Исполняет вызывающее окно.
    ScaleMinus,
}

/// Нажатая клавиша совпала с сочетанием `raw` из конфига. Сравниваем ТОЛЬКО
/// `modifiers`+`key`: событие на Windows несёт ещё `key_char`, которого у распарсенного
/// `Keystroke` нет → полный `==` для Ctrl+буква никогда не совпадал.
fn pressed(raw: &str, ev: &KeyDownEvent) -> bool {
    let raw = raw.trim();
    !raw.is_empty()
        && matches!(
            Keystroke::parse(raw),
            Ok(k) if k.modifiers == ev.keystroke.modifiers && k.key == ev.keystroke.key
        )
}

/// Распознать нажатие как действие по конфигу хоткеев. `None` — не наш хоткей.
/// Порядок веток = приоритет (фигуры/масштаб раньше торговых).
pub fn resolve(ev: &KeyDownEvent, hk: &HotkeysConfig) -> Option<HotkeyAction> {
    use HotkeyAction as A;
    let p = |raw: &str| pressed(raw, ev);

    // Слой рисования — до торговых веток.
    if p(&hk.draw_hline) {
        return Some(A::FigTool(FigureTool::HLine));
    }
    if p(&hk.draw_segment) {
        return Some(A::FigTool(FigureTool::Segment));
    }
    if p(&hk.draw_triangle) {
        return Some(A::FigTool(FigureTool::Triangle));
    }
    if p(&hk.draw_channel) {
        return Some(A::FigTool(FigureTool::Channel));
    }
    if p(&hk.switch_figure) {
        return Some(A::SwitchFigure);
    }
    if p(&hk.fig_delete) {
        return Some(A::FigDelete);
    }
    if p(&hk.fig_alert) {
        return Some(A::FigAlert);
    }
    // Встроенный Shift+Esc — закрыть все графики (до плоского Esc, иначе перехватится им).
    if ev.keystroke.key == "escape"
        && ev.keystroke.modifiers.shift
        && !ev.keystroke.modifiers.control
        && !ev.keystroke.modifiers.alt
        && !ev.keystroke.modifiers.platform
    {
        return Some(A::CloseAllCharts);
    }
    if ev.keystroke.key == "escape" && ev.keystroke.modifiers == Modifiers::default() {
        return Some(A::CloseActiveChart);
    }
    // Встроенные (не конфигурируемые) клавиши.
    if p("ctrl-shift-f10") {
        return Some(A::ResetWindows);
    }
    if (ev.keystroke.key == "tab" || ev.keystroke.key == "delete")
        && ev.keystroke.modifiers == Modifiers::default()
    {
        return Some(A::CancelHoveredOrder);
    }

    // Масштаб.
    if p(&hk.scale_plus) {
        return Some(A::ScalePlus);
    }
    if p(&hk.scale_minus) {
        return Some(A::ScaleMinus);
    }

    // Размеры / sell-пресеты.
    if let Some(i) = hk.order_size.iter().position(|r| p(r)) {
        return Some(A::OrderSize(i));
    }
    if let Some(i) = hk.sell_preset.iter().position(|r| p(r)) {
        return Some(A::SellPreset(i));
    }

    // Управление ордерами рынка активного чарта.
    if p(&hk.cancel_buy) {
        return Some(A::CancelBuy);
    }
    if p(&hk.cancel_all_buys) {
        return Some(A::CancelAllBuys);
    }
    if p(&hk.panic_sell) {
        return Some(A::PanicSell);
    }
    if p(&hk.panic_sell_one) {
        return Some(A::PanicSellOne);
    }
    if p(&hk.join_sells) {
        return Some(A::JoinSells);
    }
    if p(&hk.split_order) || p(&hk.split_order_x) {
        return Some(A::SplitOrder);
    }
    if p(&hk.new_long) {
        return Some(A::NewLong);
    }
    if p(&hk.new_short) {
        return Some(A::NewShort);
    }
    if p(&hk.shift_buy_up) {
        return Some(A::ShiftOrder { sell: false, up: true });
    }
    if p(&hk.shift_buy_down) {
        return Some(A::ShiftOrder { sell: false, up: false });
    }
    if p(&hk.shift_sell_up) {
        return Some(A::ShiftOrder { sell: true, up: true });
    }
    if p(&hk.shift_sell_down) {
        return Some(A::ShiftOrder { sell: true, up: false });
    }
    if p(&hk.switch_charts) {
        return Some(A::SwitchCharts);
    }

    if let Some(i) = hk.manual_strategy.iter().position(|r| p(r)) {
        return Some(A::ManualStrategy(i));
    }
    None
}

/// Отменить ордер ПОД КУРСОРОМ через чарт под мышью (`Backend::hovered_chart`) — общий
/// путь встроенного Tab/Del и фолбэка `FigDelete` без выделенной фигуры (дефолтный
/// `fig_delete`=Del резолвится раньше встроенной ветки и иначе затенял бы отмену навсегда).
/// `false` — нет чарта под мышью или наведённого ордера (клавиша всплывает дальше).
pub fn cancel_hovered_order(backend: &Entity<Backend>, cx: &mut App) -> bool {
    let chart = backend
        .read(cx)
        .hovered_chart
        .clone()
        .and_then(|w| w.upgrade());
    match chart {
        Some(chart) => chart.update(cx, |p, pcx| p.cancel_hovered_order(pcx)),
        None => false,
    }
}

/// Исполнить действие (кроме масштаба — его окно делает само). `target` — рынок активного
/// чарта окна (`(ядро, рынок)`); `active_core` — активное торговое ядро окна. Возвращает
/// `true`, если действие обработано (клавишу надо погасить). `Scale*` возвращает `false`.
pub fn apply(
    action: HotkeyAction,
    b: &mut Backend,
    bcx: &mut Context<Backend>,
    target: Option<(CoreId, String)>,
    active_core: Option<CoreId>,
) -> bool {
    use HotkeyAction as A;
    match action {
        A::FigTool(tool) => {
            // Повтор того же инструмента в активном режиме — выключает; иначе выбирает + вкл.
            if b.fig_draw_mode && b.fig_tool == tool {
                b.fig_draw_mode = false;
            } else {
                b.fig_tool = tool;
                b.fig_draw_mode = true;
            }
            bcx.notify();
            true
        }
        A::SwitchFigure => {
            // Листаем инструмент по кругу и держим режим рисования включённым (как «смена
            // фигуры» в MoonBot). Повтор хоткея → следующий инструмент; выход — Esc/повтор его же.
            b.fig_tool = b.fig_tool.next();
            b.fig_draw_mode = true;
            bcx.notify();
            true
        }
        A::FigAlert => {
            if b.toggle_selected_figure_alert() {
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::FigDelete => {
            if let Some((core, market, id)) = b.fig_selected.clone() {
                b.remove_figure(core, &market, id);
                bcx.notify();
                true
            } else {
                false
            }
        }
        A::OrderSize(i) => match active_core {
            Some(core) => {
                // Выбор + персист в конфиг (восстановление после перезапуска).
                b.set_order_size_sel(core, i);
                b.order_size_rev = b.order_size_rev.wrapping_add(1);
                bcx.notify();
                true
            }
            None => false,
        },
        A::ManualStrategy(i) => match active_core {
            Some(core) => {
                if crate::controls::select_manual_strategy(b, core, i) {
                    bcx.notify();
                    true
                } else {
                    false
                }
            }
            None => false,
        },
        A::SellPreset(i) => match active_core {
            Some(core) => {
                if let Err(error) = b
                    .session
                    .edit_client_settings(core, ClientSettingsEdit::SelectFixedSellSlot(i + 1))
                {
                    log::warn!("hotkey select fixed-sell slot failed: {error}");
                }
                true
            }
            None => false,
        },
        A::CancelBuy => match target {
            Some((core, market)) => {
                b.cancel_buy_orders(core, &market);
                true
            }
            None => false,
        },
        A::CancelAllBuys => match active_core {
            Some(core) => {
                b.cancel_all_buys_for_core(core);
                true
            }
            None => false,
        },
        A::PanicSell => match target {
            Some((core, market)) => {
                b.toggle_panic_sell(core, market);
                bcx.notify();
                true
            }
            None => false,
        },
        A::PanicSellOne => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.market_sell_position(core, market) {
                    log::warn!("hotkey market sell position failed: {error}");
                }
                true
            }
            None => false,
        },
        A::JoinSells => match target {
            Some((core, market)) => {
                let short = b.market_position_short(core, &market);
                if let Err(error) = b.session.join_sells(core, market, short) {
                    log::warn!("hotkey join sells failed: {error}");
                }
                true
            }
            None => false,
        },
        A::SplitOrder => match target {
            Some((core, market)) => {
                if let Err(error) = b.session.split_order(core, market, 2) {
                    log::warn!("hotkey split order failed: {error}");
                }
                true
            }
            None => false,
        },
        A::ShiftOrder { sell, up } => match target {
            Some((core, market)) => shift_orders(b, core, &market, sell, up),
            None => false,
        },
        // Масштаб, постановка по курсору и переключение активного чарта — забота вызывающего
        // окна: масштаб/активный чарт свои у каждого окна (rev-механизм группы), а цену ордера
        // знает только чарт под курсором (`Backend::hovered_chart`).
        A::ScalePlus | A::ScaleMinus | A::NewLong | A::NewShort | A::SwitchCharts
        | A::ResetWindows | A::CancelHoveredOrder | A::CloseAllCharts | A::CloseActiveChart => {
            false
        }
    }
}

/// Хоткеи shift_buy/sell_up/down: сдвинуть живые линии стороны `sell` рынка на один шаг
/// цены (`chart_price_step` рынка) через `move_order` — та же команда и те же гарды, что
/// у перетаскивания линии мышью (вход двигаем только пока не залит). Шаг рынка неизвестен
/// или двигать нечего → `false` (не выдумываем шаг).
fn shift_orders(b: &mut Backend, core: CoreId, market: &str, sell: bool, up: bool) -> bool {
    let Some(step) = b.session.market_source().price_step(core, market) else {
        return false;
    };
    let kind = if sell { LineKind::Sell } else { LineKind::Buy };
    let mut moves: Vec<(u64, f64)> = Vec::new();
    if let Some(core_data) = b.session.store().core(core) {
        for order in core_data
            .order_lines
            .iter_market(market)
            .filter(|order| order.closed_ms.is_none())
        {
            // Залитый вход — историческая отметка, реплейсить нечего (см. drag в trade.rs).
            if !sell && order.fill_pct > 0.0 {
                continue;
            }
            if let Some(price) = order.lines[kind as usize]
                .current_price()
                .filter(|p| p.is_finite() && *p > 0.0)
            {
                moves.push((order.uid, f64::from(price)));
            }
        }
    }
    if moves.is_empty() {
        return false;
    }
    for (uid, price) in moves {
        let next = if up { price + step } else { price - step };
        if next <= 0.0 {
            continue;
        }
        if let Err(error) = b.session.move_order(core, uid, next) {
            log::warn!("hotkey shift order failed: uid={uid} price={next:.8}: {error}");
        }
    }
    true
}
