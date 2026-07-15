//! Per-tab/окно поле ввода монеты (поиск + выпадающий список «COIN - Server») —
//! ШИМ над общим виджетом [`crate::controls::coin_search`].
//!
//! Сам виджет (поиск по market-юниверсу + рендер списка) переехал в `controls/coin_search.rs`:
//! он общий для полоски вкладок, выносных окон, тикера курса в шапке и фильтра монеты
//! «Отчёта». Здесь остаётся только чарт-специфика ([`MANUAL_COIN_TTL_MS`]) и ре-экспорт,
//! чтобы вызовы `coin_search::search` / `coin_search::render_popup` в `chart_tabs`
//! (и в `shell/ticker.rs`) продолжали работать без правок.
//!
//! Поиск идёт по market-юниверсу ядер, относящихся к ВКЛАДКЕ (Main/выносное окно):
//! Main и `Shared` → все ядра группы; `Core(id)` → одно ядро; `Bundle` → ядра связки.
//! Выбор открывает монету на АКТИВНОЙ вкладке (Main → fullscreen-чарт; Add → стек) —
//! это задаёт хозяин через `on_pick` (см. [`super::common::coin_pick_handler`]).

/// Сколько совпадений тянуть из MoonProto-поиска на одно ядро.
pub(super) use crate::controls::coin_search::COIN_SEARCH_LIMIT;
/// Поиск монет и рендер выпадающего списка — общий виджет (`pub(crate)`: реюзает тикер
/// курса в шапке, `shell/ticker.rs`).
pub(crate) use crate::controls::coin_search::{render_popup, search};

/// TTL «ручной» монеты в Add-стеке: фактически постоянная на сессию (≈1 год), чтобы открытая
/// руками монета не выбывала по auto-TTL детектов. Main открывает без TTL (`open_or_focus`).
/// Чарт-специфика — в общий виджет не уезжает.
pub(super) const MANUAL_COIN_TTL_MS: f64 = 365.0 * 24.0 * 3600.0 * 1000.0;
