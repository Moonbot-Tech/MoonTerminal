//! The Assets row's `Order` button on a SPOT holding — Moonbot's Pending Order window in SELL mode.
//!
//! Moonbot (`MoonBot.exe`, the row's `Order` click handler) forks on the engine: a futures engine
//! sends `DoClosePosition(market, MakeMarketSell=false)` and shows nothing, which `table.rs` does
//! through `limit_close_position`; a spot engine prepares `TDorderForm` for the market with the
//! order type set to SELL — on the market's live order when it has one, on the market itself
//! otherwise. This module is the spot half: an existing order opens the terminal's own Active
//! Order editor, and a bare holding opens the small sell dialog below, whose OK sends
//! `TDoSellOrderCommand` at the trader's price and size.
//!
//! The sell price is seeded the way Moonbot seeds it: the live price moved by the core's main
//! take-profit percent (`x_sell`), so the field opens at the same number the Pending Order window
//! shows in `Fixed`. The size is seeded with the holding, and `Max` puts it back.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonInput, MoonInputState, MoonNotification, MoonPalette,
    MoonWindowExt as _, h_flex, v_flex,
};
use rust_i18n::t;

use super::AssetsView;
use super::table::market_sell_core_is_authorized;
use crate::Backend;
use crate::design::{self, moon};

/// Everything the OK button needs, captured when the dialog opened.
struct SpotOrderDraft {
    core: CoreId,
    market: String,
    coin: String,
    /// The holding at open time; `Max` restores it.
    holding: f64,
    size_input: Entity<MoonInputState>,
    price_input: Entity<MoonInputState>,
}

/// Seed for the sell price: the live price moved by the core's main take-profit percent, which is
/// what Moonbot's Pending Order window shows in its `Fixed` field (`price * (1 + x_sell / 100)`).
///
/// Args:
///     price: Live price of the market in its quote currency.
///     tp_pct: The core's main take-profit percent; a non-finite one counts as zero.
///
/// Returns:
///     `None` unless the price is finite and positive — a seed of zero is worse than an empty
///     field, because zero is exactly the price that made the core invent a quantity.
fn seed_sell_price(price: f64, tp_pct: f64) -> Option<f64> {
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    let pct = if tp_pct.is_finite() { tp_pct } else { 0.0 };
    let seeded = price * (1.0 + pct / 100.0);
    (seeded.is_finite() && seeded > 0.0).then_some(seeded)
}

/// Formats a seed as a plain decimal with no trailing zeroes; zero, negative and non-finite are
/// empty. Eight decimals for ordinary prices, and enough for six significant digits below that —
/// a fixed eight would print a 4e-9 BTC-quoted price as "0", the one value this dialog must never
/// seed.
fn fmt_seed(v: f64) -> String {
    if !v.is_finite() || v <= 0.0 {
        return String::new();
    }
    let decimals = if v < 1e-2 {
        // Digits to the first significant one, plus six of them; capped where f64 stops meaning it.
        ((-v.log10().floor()) as usize + 5).min(18)
    } else {
        8
    };
    let text = format!("{v:.decimals$}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string();
    // Below the cap the decimal prints as "0": still not a seed.
    if text == "0" { String::new() } else { text }
}

/// Parses a typed number, accepting a comma as the decimal point; only finite positives count.
fn parse_positive(s: &str) -> Option<f64> {
    s.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

/// The LIVE quote-denominated price of the market, as the market-sell path reads it: a
/// wallet-derived row prices in USDT, which is not the book this order must sit in. Read at open
/// for the seed and on every render for the "Current:" figure, so a dialog left open does not show
/// a number the book has already left.
fn live_price(backend: &Backend, core: CoreId, market: &str) -> Option<f64> {
    backend
        .session
        .market_source()
        .latest_price(core, market)
        .ok()
        .map(f64::from)
        .filter(|p| p.is_finite() && *p > 0.0)
}

/// The live order Moonbot would open the window on: the LAST live order of this core on this
/// market, preferring one whose entry executed (its sell leg is what the trader came to re-price).
///
/// Args:
///     backend: Read for the core's live order snapshot.
///     core: Core of the Assets row.
///     market: Resolved market of the row.
///
/// Returns:
///     The order's uid, or `None` when the market has no live order.
fn live_order_on_market(backend: &Backend, core: CoreId, market: &str) -> Option<u64> {
    let store = backend.session.store();
    let orders = &store.core(core)?.orders;
    let on_market = || orders.iter().filter(|o| o.market == market);
    on_market()
        .rfind(|o| crate::panels::orders::executed(o))
        .or_else(|| on_market().next_back())
        .map(|o| o.uid)
}

/// Open the spot `Order` flow for a holding row: the Active Order editor when the market has a live
/// order, the sell dialog otherwise.
///
/// Args:
///     view: Assets entity retaining host scope and Backend authority.
///     core: Core captured from the rendered row.
///     market: Resolved market of the row.
///     coin: Display token for the dialog title.
///     holding: Sellable coin quantity of the row, the dialog's size seed.
///     window: Window that owns the unique dialog and its notifications.
///     app: Application context used to build the dialog.
pub(super) fn open_spot_order(
    view: Entity<AssetsView>,
    core: CoreId,
    market: String,
    coin: String,
    holding: f64,
    window: &mut Window,
    app: &mut App,
) {
    let (backend, group, existing, price_seed) = {
        let this = view.read(app);
        let b = this.backend.read(app);
        let group = match &this.scope {
            super::AssetsScope::Group(g) => Some(g.clone()),
            super::AssetsScope::All => None,
        };
        let existing = live_order_on_market(b, core, &market);
        let live = live_price(b, core, &market);
        let tp_pct = b
            .session
            .store()
            .core(core)
            .and_then(|c| c.client_settings.as_ref())
            .map(|s| s.take_profit_main_pct)
            .unwrap_or(0.0);
        let seed = live.and_then(|p| seed_sell_price(p, tp_pct));
        (this.backend.clone(), group, existing, seed)
    };
    if let Some(uid) = existing {
        crate::panels::open_order_edit(backend, group, core, uid, window, app);
        return;
    }

    let input = |window: &mut Window, cx: &mut App, v: f64| {
        cx.new(|cx| MoonInputState::new(window, cx).default_value(fmt_seed(v)))
    };
    let size_input = input(window, app, holding);
    let price_input = input(window, app, price_seed.unwrap_or(0.0));
    let draft = std::rc::Rc::new(SpotOrderDraft {
        core,
        market,
        coin,
        holding,
        size_input,
        price_input,
    });
    window.open_unique_moon_dialog("assets-spot-order", app, move |dialog, _window, cx| {
        let p = MoonPalette::active(cx);
        let body_draft = draft.clone();
        let body_view = view.clone();
        let ok_draft = draft.clone();
        let ok_view = view.clone();
        let title = t!("assets.order_dialog.title", coin = draft.coin.as_str()).to_string();
        dialog
            .w(px(380.0))
            .close_button(true)
            .overlay(true)
            .overlay_closable(true)
            .bg(moon(p.shell_high))
            .border_color(moon(p.border))
            .rounded(design::r_container(cx))
            .text_color(moon(p.text))
            .header(
                div()
                    .w_full()
                    .py_2()
                    .border_b_1()
                    .border_color(moon(p.border))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .on_cancel(|_, _, _| true)
            .content(move |content, _window, cx| {
                content.child(dialog_body(&body_draft, &body_view, cx))
            })
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        MoonButton::new("assets-spot-order-cancel")
                            .ghost()
                            .label(t!("dialogs.cancel").to_string())
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                            })
                            .render(),
                    )
                    .child(
                        MoonButton::new("assets-spot-order-ok")
                            .variant(MoonButtonVariant::Blue)
                            .label("OK")
                            .on_click(move |_, window, cx| match submit(&ok_draft, &ok_view, cx) {
                                Ok(()) => window.close_dialog(cx),
                                Err(error) => {
                                    log::warn!("assets spot order: {error:#}");
                                    window.push_notification(
                                        MoonNotification::warning(error.to_string()),
                                        cx,
                                    );
                                }
                            })
                            .render(),
                    ),
            )
    });
}

/// One labelled input row: caption, the input, and an optional trailing element.
fn field_row(
    p: MoonPalette,
    cx: &App,
    label: String,
    input: impl IntoElement,
    trailing: Option<AnyElement>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(110.0))
                .flex_none()
                .text_color(moon(p.text_muted))
                .child(label),
        )
        .child(div().w(px(150.0)).child(input))
        .when_some(trailing, |el, t| el.child(t))
        .text_size(design::t_body(cx))
}

fn dialog_body(
    draft: &std::rc::Rc<SpotOrderDraft>,
    view: &Entity<AssetsView>,
    cx: &mut App,
) -> AnyElement {
    let p = MoonPalette::active(cx);
    // Re-read per render: the figure is labelled "Current" and must be.
    let current_price = live_price(view.read(cx).backend.read(cx), draft.core, &draft.market);
    let max_draft = draft.clone();
    let max = MoonButton::new("assets-spot-order-max")
        .ghost()
        .label(t!("assets.order_dialog.max").to_string())
        .on_click(move |_, window, cx| {
            let holding = fmt_seed(max_draft.holding);
            max_draft
                .size_input
                .update(cx, |s, c| s.set_value(holding, window, c));
        })
        .render()
        .into_any_element();
    // MIXED NODE: `assets.order_dialog.current` welds the localized "Current:" label to the price
    // figure in one text node, the way the Active Order editor does — stays mono.
    let current = current_price.map(|price| {
        div()
            .font_family(design::mono())
            .text_color(moon(p.text_muted))
            .child(t!("assets.order_dialog.current", p = crate::panels::num(price)).to_string())
            .into_any_element()
    });
    v_flex()
        .w_full()
        .gap_2()
        .py_2()
        .child(field_row(
            p,
            cx,
            t!("assets.order_dialog.side").to_string(),
            div()
                .font_family(design::mono())
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(moon(p.accent))
                .child(format!("SELL {}", draft.market)),
            None,
        ))
        .child(field_row(
            p,
            cx,
            t!("assets.order_dialog.size", coin = draft.coin.as_str()).to_string(),
            MoonInput::new("assets-spot-order-size")
                .state(&draft.size_input)
                .size(design::INPUT_SIZE)
                .mono(true),
            Some(max),
        ))
        .child(field_row(
            p,
            cx,
            t!("assets.order_dialog.price").to_string(),
            MoonInput::new("assets-spot-order-price")
                .state(&draft.price_input)
                .size(design::INPUT_SIZE)
                .mono(true),
            current,
        ))
        .into_any_element()
}

/// Validate the draft and send the limit sell; every refusal is an error the OK handler shows.
///
/// A group-owned dialog revalidates its captured core against the current effective scope first,
/// exactly as the Market Sell confirmation does: navigation may have moved the panel off the core
/// while the dialog sat open.
fn submit(
    draft: &std::rc::Rc<SpotOrderDraft>,
    view: &Entity<AssetsView>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let size = parse_positive(&draft.size_input.read(cx).value())
        .ok_or_else(|| anyhow::anyhow!(t!("assets.order_dialog.bad_size").to_string()))?;
    let price = parse_positive(&draft.price_input.read(cx).value())
        .ok_or_else(|| anyhow::anyhow!(t!("assets.order_dialog.bad_price").to_string()))?;
    view.update(cx, |this, cx| {
        let b = this.backend.read(cx);
        let effective_scope = this.effective_scope(b);
        if !market_sell_core_is_authorized(
            &this.scope,
            effective_scope.as_ref().map(|scope| scope.ids()),
            draft.core,
        ) {
            anyhow::bail!(t!("assets.market_sell_scope_changed").to_string());
        }
        b.session
            .limit_sell_token(draft.core, draft.market.clone(), size, price)?;
        cx.notify();
        Ok(())
    })
}

#[cfg(test)]
mod tests;
