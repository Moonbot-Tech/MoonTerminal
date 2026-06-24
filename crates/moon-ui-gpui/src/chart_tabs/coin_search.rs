//! Per-tab/окно поле ввода монеты (поиск + выпадающий список «COIN - Server»).
//!
//! Поиск идёт по market-юниверсу ядер, относящихся к ВКЛАДКЕ (Main/выносное окно):
//! Main и `Shared` → все ядра группы; `Core(id)` → одно ядро; `Bundle` → ядра связки.
//! Для каждого совпадения строка `«BTC - Bybit1»` (база монеты + имя сервера ядра).
//! Выбор открывает монету на АКТИВНОЙ вкладке (Main → fullscreen-чарт; Add → стек).
//!
//! Общий код для полоски вкладок ([`super::strip`]) и выносных окон ([`super::windows`]).

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex};
use rust_i18n::t;

use crate::Backend;
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::session::CoreId;

/// Сколько совпадений тянуть из MoonProto-поиска на одно ядро.
pub(super) const COIN_SEARCH_LIMIT: usize = 8;

/// TTL «ручной» монеты в Add-стеке: фактически постоянная на сессию (≈1 год), чтобы открытая
/// руками монета не выбывала по auto-TTL детектов. Main открывает без TTL (`open_or_focus`).
pub(super) const MANUAL_COIN_TTL_MS: f64 = 365.0 * 24.0 * 3600.0 * 1000.0;

/// Ядра, чей market-юниверс питает поле монеты этой вкладки. `bucket = None` → Main.
fn cores_for(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoreId> {
    let group_cores = || {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    match bucket {
        None | Some(ChartBucket::Shared) => group_cores(),
        Some(ChartBucket::Core(id)) => vec![*id],
        Some(ChartBucket::Bundle(name)) => {
            let split = b.config.charts_split_by_core;
            b.session
                .sessions()
                .iter()
                .filter(|s| s.group == group)
                .filter(|s| {
                    b.config
                        .servers
                        .iter()
                        .find(|sv| sv.id == s.id)
                        .map(|sv| sv.chart_bucket(split) == ChartBucket::Bundle(name.clone()))
                        .unwrap_or(false)
                })
                .map(|s| s.id)
                .collect()
        }
    }
}

/// Результаты поиска монеты для вкладки: `(ядро, market, имя сервера)`.
pub(super) fn search(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    query: &str,
) -> Vec<(CoreId, String, String)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let cores = cores_for(b, group, bucket);
    let ms = b.session.market_source();
    let mut out = Vec::new();
    for core in cores {
        let server = b
            .session
            .sessions()
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        for market in ms.search_markets(core, query, COIN_SEARCH_LIMIT) {
            out.push((core, market, server.clone()));
        }
    }
    out
}

/// Выпадающий список совпадений (или «нет совпадений»). `on_pick(core, market, window, app)`
/// зовётся по клику строки — владелец сам открывает монету и чистит поле.
pub(super) fn render_popup<F>(
    id: &'static str,
    results: Vec<(CoreId, String, String)>,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
{
    let hover_bg = rgb(p.shell_high);
    // `.id(..)` делает контейнер stateful → доступен `overflow_y_scroll` (gpui сам трекает
    // прокрутку колесом по этому id); иначе длинный список просто обрезался бы по `max_h`.
    let mut list = div()
        .id(id)
        .flex()
        .flex_col()
        .w(px(220.0))
        .max_h(px(280.0))
        .overflow_y_scroll()
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(4.0))
        .py(design::ui_px(cx, 4.0));

    if results.is_empty() {
        return list.child(
            div()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.coin.no_results").to_string()),
        );
    }

    for (i, (core, market, server)) in results.into_iter().enumerate() {
        let quote = moon_core::symbol::resolve_quote(&market);
        let base = moon_core::symbol::base_symbol(&market, &quote).to_string();
        let on_pick = on_pick.clone();
        let market_pick = market.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-row-{i}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .on_mouse_down(MouseButton::Left, move |_, window, app| {
                    on_pick(core, market_pick.clone(), window, app);
                    app.stop_propagation();
                })
                .child(
                    h_flex()
                        .w_full()
                        .gap(design::ui_px(cx, 4.0))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(design::t_body(cx))
                                .text_color(rgb(p.text))
                                .child(base),
                        )
                        .child(
                            div()
                                .text_size(design::t_caption(cx))
                                .text_color(rgb(p.text_muted))
                                .child(format!("- {server}")),
                        )
                        .when(!quote.is_empty(), |row| {
                            row.child(
                                div()
                                    .ml_auto()
                                    .text_size(design::t_caption(cx))
                                    .text_color(rgb(p.text_muted))
                                    .child(quote),
                            )
                        }),
                ),
        );
    }
    list
}
