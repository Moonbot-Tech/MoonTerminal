//! Тогл и пикер «ручной стратегии» (Moonbot manual strategies) в шапке.
//!
//! Состояние живёт в ЯДРЕ (`ClientSettings.use_manual_strategy`/`manual_strategy_id`):
//! тогл/выбор шлют `ClientSettingsEdit::ManualStrategy`, живой отклик — optimistic-кэш
//! `Backend::manual_strat_state` до echo ClientSettings. При включённом режиме sell/стопы
//! ручным ордерам ставит САМО ЯДРО из полей стратегии — TP/S-слоты/SL тулбара к новым
//! ордерам не применяются (тулбар их гасит), а `effective_strat_id` в moon-core уже ведёт
//! ручные ордера по выбранной стратегии.

use gpui::*;
use rust_i18n::t;

use moon_ui::{
    MoonMenuItem, MoonMenuSize, MoonPalette, MoonPopover, MoonPopoverPlacement, MoonPopupMenu,
    MoonSelectorPill, MoonSelectorSegment, MoonToggle, MoonToggleLabelSide, MoonToggleSize,
    h_flex,
};

use moon_core::feed::{ClientSettingsEdit, StrategyRow, StrategySchemaModel};
use moon_core::session::CoreId;

use crate::{Backend, design};

/// Ordinal вида Manual в схеме стратегий Moonbot (см. `strat_kind_name`).
const MANUAL_KIND: u8 = 12;

/// Геометрия пилюли — как у селектора ядра в шапке (ширина — по контенту, имя не режем).
const PILL_H: f32 = 26.0;

/// Кластер «ручная стратегия» шапки: разделитель + тогл MS + пилюля-пикер + сводка
/// параметров выбранной стратегии. Нет активного ядра группы → не рисуем ничего
/// (разделитель внутри — исчезает вместе с контролом).
pub fn manual_strategy_controls(
    group: &str,
    backend: &Entity<Backend>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let b = backend.read(cx);
    let Some(core) = b.active_trade_core(group) else {
        return div().into_any_element();
    };
    let (on, id) = b.manual_strat_state(core);
    let core_data = b.session.store().core(core);
    // Manual-стратегии активного ядра — список пикера.
    let manuals: Vec<(u64, String)> = core_data
        .map(|cd| {
            cd.strategies
                .iter()
                .filter(|s| s.kind_ordinal == MANUAL_KIND)
                .map(|s| (s.id, s.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let sel_row = core_data.and_then(|cd| cd.strategies.iter().find(|s| s.id == id && id != 0));
    let schema = core_data.and_then(|cd| cd.schema.as_ref());
    // Сводка ключевых параметров стратегии (как в MB: Buy/Sell/SL/TS) — только при вкл.
    let summary = (on && sel_row.is_some())
        .then(|| sel_row.map(|r| strat_summary(r, schema)))
        .flatten();

    // Имя в пилюле: выбранная стратегия / «—» (не выбрана) / «?» (id есть, стратегии нет —
    // удалена или снимок ещё не пришёл).
    let display: String = match (sel_row, id) {
        (Some(r), _) => r.name.clone(),
        (None, 0) => t!("header.ms_none").to_string(),
        (None, _) => "?".to_string(),
    };
    let dot_color = if on && sel_row.is_some() {
        if p.is_light() { p.green_text } else { p.green }
    } else if on {
        // Включено, но стратегия не разрезолвилась — сигналим красным.
        if p.is_light() { p.red_text } else { p.red }
    } else {
        p.text_muted
    };

    // Пункты меню пикера. Пустой список — некликабельная заглушка.
    let empty_label = t!("header.ms_empty").to_string();
    let mut items = Vec::with_capacity(manuals.len().max(1));
    if manuals.is_empty() {
        items.push(MoonMenuItem::with_key("ms-empty", empty_label.clone()));
    }
    for (sid, name) in &manuals {
        let sid = *sid;
        let backend = backend.clone();
        items.push(
            MoonMenuItem::with_key(format!("ms-{sid}"), name.clone())
                .selected(id == sid)
                .checked(id == sid)
                // Выбор стратегии из списка сразу ВКЛЮЧАЕТ режим (как выбор в меню MB).
                .on_click(move |_, _, cx| {
                    backend.update(cx, |b, bcx| {
                        send_manual(b, core, true, sid);
                        bcx.notify();
                    });
                }),
        );
    }

    let toggle_backend = backend.clone();
    let mut row = h_flex()
        .flex_none()
        .gap(design::ui_px(cx, 8.0))
        .items_center()
        // Разделитель от баланса (как divider групп тулбара).
        .child(design::vline(16.0, p))
        .child(
            MoonToggle::new("header-ms-toggle")
                .label("MS")
                .label_side(MoonToggleLabelSide::Left)
                .checked(on)
                .size(MoonToggleSize::Compact)
                // Нечего включать, пока стратегия не выбрана пикером.
                .disabled(id == 0)
                .on_change(move |ch: &bool, _w, app| {
                    let v = *ch;
                    toggle_backend.update(app, |b, bcx| {
                        let (_, cur_id) = b.manual_strat_state(core);
                        if cur_id == 0 {
                            return;
                        }
                        // Выключение сохраняет ID — повторный тогл вернёт ту же стратегию.
                        send_manual(b, core, v, cur_id);
                        bcx.notify();
                    });
                }),
        )
        .child({
            // Ширина меню — по самому длинному имени стратегии (или заглушке «пусто»).
            let menu_w = design::menu_fit_width(
                cx,
                manuals
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .chain(manuals.is_empty().then(|| empty_label.as_str())),
                200.0,
            );
            MoonPopover::new("header-ms-selector")
                .placement(MoonPopoverPlacement::BottomStart)
                .width(design::popover_outer_width(cx, menu_w))
                .close_on_content_click(true)
                .trigger(
                    MoonSelectorPill::new("header-ms-pill")
                        .height(PILL_H)
                        .radius(PILL_H / 2.0)
                        .leading_dot(dot_color)
                        .segment(
                            MoonSelectorSegment::new(display)
                                .color(if on { p.text } else { p.text_soft })
                                .weight(500.0),
                        )
                        .render(),
                )
                .content(
                    MoonPopupMenu::new("header-ms-menu")
                        .width(menu_w)
                        .size(MoonMenuSize::Compact)
                        .items(items)
                        .render(),
                )
        });
    if let Some(summary) = summary {
        row = row.child(
            div()
                .text_size(design::t_caption(cx))
                .font_family(design::mono())
                .text_color(rgb(p.text_soft))
                .child(summary),
        );
    }
    row.into_any_element()
}

/// Хоткей «Ручная стратегия N»: выбрать `ix`-ю manual-стратегию ядра (тот же порядок,
/// что в списке пикера MS) и включить режим — ровно то, что делает клик по пункту меню.
/// `false` — у ядра нет manual-стратегии с таким номером (клавиша всплывает дальше).
pub(crate) fn select_manual_strategy(b: &mut Backend, core: CoreId, ix: usize) -> bool {
    let sid = b.session.store().core(core).and_then(|cd| {
        cd.strategies
            .iter()
            .filter(|s| s.kind_ordinal == MANUAL_KIND)
            .nth(ix)
            .map(|s| s.id)
    });
    match sid {
        Some(sid) => {
            send_manual(b, core, true, sid);
            true
        }
        None => false,
    }
}

/// Шлёт правку ручной стратегии в ядро + optimistic-кэш (живой отклик до echo).
fn send_manual(b: &mut Backend, core: CoreId, on: bool, id: u64) {
    b.set_manual_strat_local(core, on, id);
    if let Err(e) = b
        .session
        .edit_client_settings(core, ClientSettingsEdit::ManualStrategy { on, id })
    {
        log::warn!("manual strategy edit failed: {e:#}");
    }
}

/// Сводка ключевых параметров стратегии (как строка MB «Buy: +0.00% Sell: +0.50% SL: ON
/// TS: OFF»). Поле, равное дефолту схемы, сервер НЕ шлёт — читаем с фолбэком на дефолт
/// схемы вида. Не разрезолвившиеся Buy/Sell пропускаем (имя поля может отличаться у
/// сборки MB), SL/TS показываем всегда.
fn strat_summary(row: &StrategyRow, schema: Option<&StrategySchemaModel>) -> String {
    let field = |name: &str| strat_field(row, schema, name);
    let mut parts: Vec<String> = Vec::with_capacity(4);
    // Buy/Sell показываем ВСЕГДА (как строка MB): поле, равное дефолту, не приходит ни в
    // снимке, ни (бывает) в секциях схемы — тогда это 0 = «по текущей цене» (подтверждено
    // логом ядра «signal price +0%»).
    let buy = field("BuyPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Buy {}", fmt_pct(&buy)));
    let sell = field("SellPrice").unwrap_or_else(|| "0".to_string());
    parts.push(format!("Sell {}", fmt_pct(&sell)));
    parts.push(format!("SL {}", on_off(field("UseStopLoss"))));
    parts.push(format!("TS {}", on_off(field("UseTrailing"))));
    parts.join(" · ")
}

/// Значение поля стратегии: снимок → дефолт схемы её вида → None.
fn strat_field(
    row: &StrategyRow,
    schema: Option<&StrategySchemaModel>,
    name: &str,
) -> Option<String> {
    if let Some((_, v)) = row.fields.iter().find(|(n, _)| n == name) {
        return Some(v.clone());
    }
    schema?
        .kinds
        .iter()
        .find(|k| k.ordinal == row.kind_ordinal)?
        .sections
        .iter()
        .flat_map(|s| s.fields.iter())
        .find(|f| f.name == name)?
        .default
        .clone()
}

/// Форматирует процентное поле со знаком («0.5» → «+0.50%»); не число — как есть.
fn fmt_pct(v: &str) -> String {
    v.trim()
        .parse::<f64>()
        .map(|f| format!("{f:+.2}%"))
        .unwrap_or_else(|_| v.to_string())
}

/// Булево поле («Yes»/«No» из `fmt_field`) → «ON»/«OFF»; отсутствует → «OFF».
fn on_off(v: Option<String>) -> &'static str {
    match v {
        Some(s) if s.trim().eq_ignore_ascii_case("yes") => "ON",
        _ => "OFF",
    }
}
