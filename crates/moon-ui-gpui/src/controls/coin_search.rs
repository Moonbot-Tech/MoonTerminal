//! Shared token-search widget that searches each core's market universe and displays a
//! `COIN - Server` dropdown.
//!
//! Each result is shown as a row such as `BTC - Bybit1`, combining the base token with the core's
//! server name. The widget does not define selection behavior; its owner supplies `on_pick`. Chart
//! tabs open a market, the header sets the rate ticker, and Report sets its token filter text.
//!
//! Consumers are the chart-tab strip and detached windows through the
//! [`crate::chart_tabs::coin_search`] shim, the header rate ticker in `shell/ticker.rs`, and the
//! Report token filter in `panels/report`.

use std::collections::HashSet;

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{
    MoonButton, MoonButtonSize, MoonButtonVariant, MoonCheckbox, MoonCheckboxSize, MoonPalette,
    h_flex,
};
use rust_i18n::t;

use crate::Backend;
use crate::design;
use moon_core::config::ChartBucket;
use moon_core::market::MarketLabel;
use moon_core::session::CoreId;

/// Maximum number of MoonProto search results requested per core.
pub(crate) const COIN_SEARCH_LIMIT: usize = 8;

/// Returns the cores whose market universes feed this token field. None searches the full group,
/// the same as a shared bucket.
///
/// Returned in canonical order: the search popup groups its hits per core and renders them
/// in the order given, so this order is what the user reads as `COIN — Server` rows.
fn cores_for(b: &Backend, group: &str, bucket: Option<&ChartBucket>) -> Vec<CoreId> {
    let group_cores = || {
        b.session
            .sessions()
            .iter()
            .filter(|s| s.group == group)
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    let order = crate::core_order::CoreOrder::new(&b.config);
    let mut ids = match bucket {
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
    };
    order.sort_by(&mut ids, |id| *id);
    ids
}

/// Maps a Russian-layout character to the Latin character on the same physical QWERTY key. Market
/// tickers use Latin characters, so Russian-layout input usually indicates an unchanged keyboard
/// layout. Converting it lets a physically typed `BTC` query find `BTC`. Case is preserved, and an
/// unknown character is returned unchanged.
fn ru_key_to_en(ch: char) -> char {
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    let mapped = match lower {
        'й' => 'q',
        'ц' => 'w',
        'у' => 'e',
        'к' => 'r',
        'е' => 't',
        'н' => 'y',
        'г' => 'u',
        'ш' => 'i',
        'щ' => 'o',
        'з' => 'p',
        'х' => '[',
        'ъ' => ']',
        'ф' => 'a',
        'ы' => 's',
        'в' => 'd',
        'а' => 'f',
        'п' => 'g',
        'р' => 'h',
        'о' => 'j',
        'л' => 'k',
        'д' => 'l',
        'ж' => ';',
        'э' => '\'',
        'я' => 'z',
        'ч' => 'x',
        'с' => 'c',
        'м' => 'v',
        'и' => 'b',
        'т' => 'n',
        'ь' => 'm',
        'б' => ',',
        'ю' => '.',
        'ё' => '`',
        _ => return ch,
    };
    if ch.is_uppercase() {
        mapped.to_ascii_uppercase()
    } else {
        mapped
    }
}

/// Converts Russian-layout input to the corresponding English QWERTY keys when the query contains
/// Russian letters; otherwise borrows the original string to avoid allocating for Latin input.
pub(crate) fn normalize_layout(query: &str) -> std::borrow::Cow<'_, str> {
    if query
        .chars()
        .any(|c| ('а'..='я').contains(&c) || ('А'..='Я').contains(&c) || c == 'ё' || c == 'Ё')
    {
        std::borrow::Cow::Owned(query.chars().map(ru_key_to_en).collect())
    } else {
        std::borrow::Cow::Borrowed(query)
    }
}

/// One search hit: the market to open, and how to LABEL it.
///
/// The label is resolved here, once per hit, rather than in the row renderer: the renderer has no
/// core, and without one a Hyperliquid spot market can only be shown as its index (`@156`) and
/// three COIN-M contracts of one coin all read `SOL`.
#[derive(Clone)]
pub(crate) struct CoinHit {
    pub(crate) core: CoreId,
    /// Market key, used to open and to match the selection. Never displayed.
    pub(crate) market: String,
    /// Core name shown beside the coin.
    pub(crate) server: String,
    /// Coin token and quote as the CORE names them; see `MarketDataSource::market_label`.
    pub(crate) label: MarketLabel,
}

/// Returns token-search results, each carrying its resolved label.
pub(crate) fn search(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    query: &str,
) -> Vec<CoinHit> {
    let query = normalize_layout(query.trim());
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
        let markets = ms.search_markets(core, query, COIN_SEARCH_LIMIT);
        // One lock and one snapshot for this core's whole page of hits.
        let refs: Vec<&str> = markets.iter().map(String::as_str).collect();
        let labels = ms.market_labels(core, &refs);
        out.extend(
            markets
                .into_iter()
                .zip(labels)
                .map(|(market, label)| CoinHit {
                    core,
                    market,
                    server: server.clone(),
                    label,
                }),
        );
    }
    out
}

/// Renders a result dropdown or an empty-result message, with multi-selection checkboxes and an
/// Open in New Tab button when enabled. Clicking outside a checkbox calls the owner-defined
/// `on_pick`; a checkbox calls `on_toggle`; the footer button calls `on_open_new` for the accumulated
/// selection. `selected` contains the currently checked markets. In single-selection mode,
/// `on_toggle` and `on_open_new` are never called.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_popup<F, G, H>(
    id: &'static str,
    results: Vec<CoinHit>,
    selected: &HashSet<(CoreId, String)>,
    multi_select: bool,
    p: MoonPalette,
    cx: &App,
    on_pick: F,
    on_toggle: G,
    on_open_new: H,
) -> Stateful<Div>
where
    F: Fn(CoreId, String, &mut Window, &mut App) + Clone + 'static,
    G: Fn(CoreId, String, &mut App) + Clone + 'static,
    H: Fn(&mut App) + Clone + 'static,
{
    let hover_bg = rgb(p.shell_high);
    let selected_count = selected.len();
    // `.id(..)` makes the container stateful so `overflow_y_scroll` can let GPUI track wheel
    // scrolling by ID. Without it, max_h would simply clip a long list.
    let mut list = div()
        .id(SharedString::from(format!("{id}-list")))
        .flex()
        .flex_col()
        .w_full()
        .max_h(px(280.0))
        .overflow_y_scroll()
        .py(design::ui_px(cx, 4.0));

    if results.is_empty() {
        list = list.child(
            div()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .text_size(design::t_caption(cx))
                .text_color(rgb(p.text_muted))
                .child(t!("chart.coin.no_results").to_string()),
        );
    }

    for (i, hit) in results.into_iter().enumerate() {
        let CoinHit {
            core,
            market,
            server,
            label,
        } = hit;
        let (base, quote) = (label.coin, label.quote);
        let on_pick = on_pick.clone();
        let market_pick = market.clone();
        let checked = selected.contains(&(core, market.clone()));
        let on_toggle = on_toggle.clone();
        let market_toggle = market.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("{id}-row-{i}")))
                .w_full()
                .px(design::ui_px(cx, 8.0))
                .py(design::ui_px(cx, 4.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg))
                .child(
                    h_flex()
                        .w_full()
                        .gap(design::ui_px(cx, 6.0))
                        .items_center()
                        // Clicking a multi-select checkbox does not open the market. The wrapper
                        // below needs no stop_propagation because MoonCheckbox does not trigger the
                        // row's on_pick handler.
                        .when(multi_select, |row| {
                            row.child(
                                MoonCheckbox::new(SharedString::from(format!("{id}-cb-{i}")))
                                    .checked(checked)
                                    .size(MoonCheckboxSize::Compact)
                                    .on_change(move |_v: &bool, _w, app| {
                                        on_toggle(core, market_toggle.clone(), app);
                                        app.stop_propagation();
                                    }),
                            )
                        })
                        // Clicking the row text selects one market through on_pick.
                        .child(
                            h_flex()
                                .flex_1()
                                .gap(design::ui_px(cx, 4.0))
                                .items_baseline()
                                .on_mouse_down(MouseButton::Left, move |_, window, app| {
                                    on_pick(core, market_pick.clone(), window, app);
                                    app.stop_propagation();
                                })
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
                ),
        );
    }

    // Show the Open in New Tab footer only in multi-select mode and enable it for a nonempty
    // selection. Keep it outside the scroller so it remains visible, with the selected count in
    // its label.
    let footer = multi_select.then(|| {
        let label = if selected_count > 0 {
            format!("{} ({selected_count})", t!("chart.coin.open_new_tab"))
        } else {
            t!("chart.coin.open_new_tab").to_string()
        };
        div()
            .w_full()
            .px(design::ui_px(cx, 6.0))
            .py(design::ui_px(cx, 6.0))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                MoonButton::new(SharedString::from(format!("{id}-open-new")))
                    .label(label)
                    .size(MoonButtonSize::Toolbar)
                    .variant(if selected_count > 0 {
                        MoonButtonVariant::Blue
                    } else {
                        MoonButtonVariant::Soft
                    })
                    .disabled(selected_count == 0)
                    .on_click(move |_, _w, app| {
                        on_open_new(app);
                        app.stop_propagation();
                    })
                    .render(),
            )
    });

    div()
        .id(id)
        .flex()
        .flex_col()
        .w(design::font_w_px(cx, 240.0))
        .bg(rgb(p.panel_high))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(design::r_button(cx))
        // Intercept mouse_down across the popup. A checkbox reacts on_change rather than
        // mouse_down, so the event would otherwise reach the dismiss layer underneath and close
        // the list. A pick row has its own earlier mouse-down handler with stop_propagation.
        .on_mouse_down(MouseButton::Left, |_, _window, app| app.stop_propagation())
        .child(list)
        .children(footer)
}
