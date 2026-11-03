//! Moonbot's "Основные" page, row for row — plus the default entry size and price.
//!
//! Its controls are live where the terminal both projects and may write them: take profit and its
//! level, the trailing stop and its level, V-Stop's level and enable flag, the coin blacklist with
//! its text and its delta filter, the trailing addition, the partial-fill sell, the buy
//! auto-cancel, the cancel-after-Sell switch, the sell placed under the wall, the fresh-coin and
//! delisting holds, the trades-into-deltas switch, and the startup analysis and manual mode above
//! the two columns.
//!
//! The "default entry" group is not on Moonbot's settings page at all: Moonbot sets the entry share,
//! the fixed amount and the buy price from its MAIN window. On a core that runs without that window
//! — a Linux server — the core still sizes and prices every entry that nothing else did (a Telegram
//! or clipboard autobuy) from these fields, so this page is the one place left to set them.
//!
//! They are TWO areas, not one: `GeneralSettings` is what both faces of the gear draw and
//! `OrderRulesSettings` is what only this one does — see the latter's own doc for why a page
//! splits there.
//!
//! The stop-loss pair (`trading.panic_if_price_drop` / `trading.price_drop_level`) is live too, in
//! the expert-only area: the compact popup draws no stop-loss row. It carries the wire's "Mirrors
//! `ClientSettingsCommand::…`" note, as do five fields this page already wrote before it —
//! `trailing_stop`, `g_take_profit`, `vol_drop_level`, `coins_black_list_text`,
//! `use_coins_black_list`; the note says the compact channel (the toolbar's SL) carries the same
//! field, not that this one may not.
//!
//! The two controls left disabled are the Copy/Paste buttons, and their reason is stated where they
//! are drawn.

use gpui::prelude::FluentBuilder;
use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::{CoreConfig, OrderRulesSettings};
use moon_core::util::fmt::round_to;

use crate::design::{self, moon_alpha};
use crate::shell::editors::EditorStore;
use crate::shell::{
    BlacklistReveal, TAKE_PROFIT_BOUNDS, TRAILING_BOUNDS, VSTOP_BOUNDS,
    coin_blacklist_remove_button, parse_num,
};

use super::super::CoreExpertView;
use super::super::widgets::{
    action, caption, columns, field, flag, group, hint, num, radio_live, rows, slider,
};

/// Stop-loss distance in per cent, counting down from zero. The reference states no range, so the
/// track bounds the control only and reaches as deep as the V-Stop track beside it.
const STOP_LOSS_PCT: (f32, f32, f32) = (-50.0, 0.0, 0.1);

/// Trailing addition, as a percentage added per per cent of price move.
///
/// This and the three below bound the CONTROL, not the value, like every other slider in this
/// window: the wire states no range for any of them, nothing clamps what the core is sent beyond
/// what the thumb can reach, and a core holding a value outside one keeps it until that row is
/// dragged — the caption still shows the real number while the thumb sits at the end of the track.
/// The range here takes the same order of magnitude as the trailing stop it adds to.
const TRAILING_ADD: (f32, f32, f32) = (0.0, 10.0, 0.1);
/// Share of ONE order that has to be filled, so it cannot exceed the whole of it.
const PARTIAL_PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);
/// Auto-cancel delay on the wire's own COMPOSITE scale — see
/// `OrderRulesSettings::auto_cancel_delay`, which decodes it. 209 is three hours on that scale,
/// past the 180 minutes the neighbouring `auto_cancel_lower_buy` defaults to.
const AUTO_CANCEL: (f32, f32, f32) = (0.0, 209.0, 1.0);
/// Fresh-coin hold in plain minutes, up to a day.
const FRESH_MINUTES: (f32, f32, f32) = (0.0, 1440.0, 1.0);
/// Default entry as thousandths of the balance: the 1..=1000 Moonbot's own control allows, which
/// the wire's reference states.
const PLAY_WITH: (f32, f32, f32) = (1.0, 1000.0, 1.0);
/// Buy-price offset in the wire's HALF-per-cent steps: -20 % to +50 %. The reference states no
/// range for this field, so the track bounds the control only, like every other slider here —
/// Moonbot's own presets set +3, +7 and +12 %.
const BUY_OFFSET: (f32, f32, f32) = (-40.0, 100.0, 1.0);

/// The buy-price offset as a signed per cent, from its half-per-cent steps: 7 prints as `+3.5`.
fn buy_offset_text(x_ask: i32) -> String {
    format!("{:+}", f64::from(x_ask) / 2.0)
}

/// The default entry share as a per cent of the balance, from its thousandths: 333 prints as `33.3`.
fn play_with_text(play_with: i32) -> String {
    format!("{}", f64::from(play_with) / 10.0)
}

/// The auto-cancel delay in words, from the scale `OrderRulesSettings::auto_cancel_delay` decodes.
///
/// Exactly what the wire documents and nothing more. Zero was drawn here as "off" for one revision,
/// which the wire does not say: its rule is "below 30 is seconds", so zero is zero seconds and this
/// prints it that way. Naming a value the protocol leaves unstated is how a row comes to tell a
/// trader the opposite of what the core will do.
fn auto_cancel_text(rules: &OrderRulesSettings) -> String {
    let (amount, minutes) = rules.auto_cancel_delay();
    let key = if minutes {
        "core_expert.gen_auto_cancel_min"
    } else {
        "core_expert.gen_auto_cancel_sec"
    };
    t!(key, v = amount.to_string()).to_string()
}

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![
        (
            "exp-gen-bl-text",
            draft.general.blacklist_text.clone(),
            (|d, t| d.general.blacklist_text = t.to_string()) as fn(&mut CoreConfig, &str),
        ),
        (
            "exp-gen-fixed-balance",
            // The shortest text that round-trips, so the box parses back exactly what it shows.
            format!("{}", draft.order_rules.fixed_trade_balance),
            |d, t| {
                // A negative amount is refused rather than clamped: it is not an order size, and a
                // clamp would leave the typed text on screen while OK carried a different number.
                if let Some(v) = parse_num(t).filter(|v| *v >= 0.0) {
                    d.order_rules.fixed_trade_balance = v;
                }
            },
        ),
    ]
}

/// See [`super::slider_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn slider_specs(
    draft: &CoreConfig,
) -> Vec<(
    &'static str,
    (f32, f32, f32),
    f32,
    fn(&mut CoreConfig, f32),
    Option<&'static str>,
)> {
    let g = &draft.general;
    let r = &draft.order_rules;
    vec![
        (
            "exp-gen-sl",
            STOP_LOSS_PCT,
            r.stop_loss_pct,
            // Rounded to the track's step: 0.1 has no exact `f32`, and the core compares what it
            // echoes against what was sent.
            (|d, v| d.order_rules.stop_loss_pct = (v * 10.0).round() / 10.0)
                as fn(&mut CoreConfig, f32),
            None,
        ),
        (
            "exp-gen-trailing",
            TRAILING_BOUNDS,
            g.trailing_pct,
            |d, v| d.general.trailing_pct = v,
            None,
        ),
        (
            "exp-gen-tp",
            TAKE_PROFIT_BOUNDS,
            g.take_profit_pct as f32,
            |d, v| d.general.take_profit_pct = f64::from(v),
            None,
        ),
        (
            "exp-gen-trailing-add",
            TRAILING_ADD,
            r.trailing_float as f32,
            // Rounded to the track's own step: 0.1 has no exact `f32`, and the raw widening
            // would put 0.30000001192092896 on the wire for a thumb dropped on 0.3. Through the
            // tree's own helper, which also normalises `-0.0` and REFUSES a non-finite input —
            // leaving the draft alone rather than putting a NaN threshold on the wire.
            |d, v| {
                if let Some(rounded) = round_to(f64::from(v), 1) {
                    d.order_rules.trailing_float = rounded;
                }
            },
            None,
        ),
        (
            "exp-gen-vstop",
            VSTOP_BOUNDS,
            g.vol_drop_level as f32,
            |d, v| d.general.vol_drop_level = v.round() as i32,
            None,
        ),
        (
            "exp-gen-partial",
            PARTIAL_PCT,
            r.auto_sell_partial as f32,
            |d, v| d.order_rules.auto_sell_partial = v.round() as i32,
            None,
        ),
        (
            "exp-gen-auto-cancel",
            AUTO_CANCEL,
            r.auto_cancel_buy_order as f32,
            |d, v| d.order_rules.auto_cancel_buy_order = v.round() as i32,
            None,
        ),
        (
            "exp-gen-bl-fresh",
            FRESH_MINUTES,
            r.dont_buy_new_coins as f32,
            |d, v| d.order_rules.dont_buy_new_coins = v.round() as i32,
            None,
        ),
        (
            "exp-gen-play-with",
            PLAY_WITH,
            r.play_with as f32,
            |d, v| d.order_rules.play_with = v.round() as i32,
            None,
        ),
        (
            "exp-gen-buy-offset",
            BUY_OFFSET,
            r.x_ask as f32,
            |d, v| d.order_rules.x_ask = v.round() as i32,
            None,
        ),
    ]
}

/// Blacklist block on the General page, including the reveal chrome.
///
/// The token strip sits above the editor. The remove button sits on the label row, after the
/// delta-filter checkbox. Both appear only while `reveal`'s coin is still in `list_text`. The
/// amber outline wraps the whole block and fades with `reveal.alpha`.
///
/// Args:
///     blacklist_flag: The «Чёрный список» checkbox.
///     exclude_flag: The delta-filter checkbox.
///     editor: The blacklist field, when the page declared one.
///     below: The fresh-coin hold and the trades-into-deltas switch.
///     list_text: Draft blacklist text the strip is painted from.
///     reveal: Highlight, when a reveal is showing.
///     on_remove: Writes this window's core and rewrites the staged page.
///     p: Active palette. The outline uses its amber.
///     cx: Application context for the type scale.
///
/// Returns:
///     The block the risk-limits group hosts.
#[allow(clippy::too_many_arguments)]
pub(super) fn expert_blacklist_rows(
    blacklist_flag: impl IntoElement,
    exclude_flag: impl IntoElement,
    editor: Option<impl IntoElement>,
    below: impl IntoElement,
    list_text: &str,
    reveal: Option<BlacklistReveal<'_>>,
    on_remove: impl Fn(&mut Window, &mut App) + 'static,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let alpha = reveal.map(|item| item.alpha).unwrap_or(0.0);
    let marked =
        reveal.is_some_and(|item| moon_core::symbol::coin_list::contains(list_text, item.coin));
    let error = reveal.and_then(|item| item.error.map(str::to_string));
    let coin = reveal.map(|item| item.coin.to_string()).unwrap_or_default();
    let remove = marked.then(|| {
        div()
            .id("exp-gen-bl-remove")
            .debug_selector(|| "exp-gen-bl-remove".into())
            .flex_none()
            .child(coin_blacklist_remove_button(
                "exp-gen-bl-remove-btn",
                &coin,
                on_remove,
            ))
    });
    let strip = marked.then(|| {
        div().id("exp-gen-bl-token").w_full().min_w_0().child(
            crate::controls::coin_token::coin_token_strip(
                list_text,
                &coin,
                design::t_body(cx),
                // The blacklist sits in one of two columns, so the strip gets half the width.
                Some(design::font_w_px(
                    cx,
                    crate::shell::core_settings_popup::CONTENT_W / 2.0 - 60.0,
                )),
                cx,
            ),
        )
    });
    let rows = v_flex()
        .id("exp-gen-bl-rows")
        .w_full()
        .gap(design::ui_px(cx, 6.0))
        .child(
            h_flex()
                .id("exp-gen-bl-label")
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 10.0))
                .child(blacklist_flag)
                .children(remove)
                .child(div().flex_1())
                .child(exclude_flag),
        )
        .children(error.map(|text| {
            div()
                .id("exp-gen-bl-error")
                .w_full()
                .text_size(design::t_caption(cx))
                .text_color(design::moon(design::danger_color(p)))
                .child(text)
        }))
        .children(strip)
        .children(editor.map(|editor| div().id("exp-gen-bl-editor").w_full().child(editor)))
        .child(below);
    div()
        .id("exp-gen-risks-body")
        .debug_selector(|| "exp-gen-risks-body".into())
        .w_full()
        .when(alpha > 0.0, |body| {
            body.rounded(design::ui_px(cx, 4.0))
                .border(px(1.5))
                .border_color(moon_alpha(p.amber, alpha))
        })
        .child(rows)
        .into_any_element()
}

/// Build the page.
///
/// Args:
///     view: Window entity the controls stage through. Not read: this runs inside its render.
///     store: Controls built for this render.
///     draft: Staged page every control reads.
///     bl_reveal: Coin highlighted on the blacklist, when a reveal is showing.
///     p: Active palette.
///     cx: Application context for font-scaled geometry.
///
/// Returns:
///     The page body.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    bl_reveal: Option<BlacklistReveal<'_>>,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let g = &draft.general;
    let r = &draft.order_rules;

    // --- Left: exits, order handling and Moonbot's own settings-transfer block -------------------
    let stops = group(
        "exp-gen-stops",
        t!("core_settings.gen_stops_frame").to_string(),
    )
    .child(
        rows(cx)
            .child(flag(
                "exp-gen-sl-on",
                t!(
                    "core_expert.gen_stop_loss",
                    v = format!("{:.1}", r.stop_loss_pct)
                )
                .to_string(),
                r.stop_loss_on,
                true,
                view,
                |d, on| d.order_rules.stop_loss_on = on,
            ))
            .children(slider(store, "exp-gen-sl", true))
            .child(flag(
                "exp-gen-trailing-on",
                t!(
                    "core_settings.gen_trailing_line",
                    v = format!("{:.2}", g.trailing_pct)
                )
                .to_string(),
                g.trailing_on,
                true,
                view,
                |d, on| d.general.trailing_on = on,
            ))
            .children(slider(store, "exp-gen-trailing", true))
            .child(flag(
                "exp-gen-tp-on",
                t!(
                    "core_settings.gen_tp_line",
                    v = format!("{:.2}", g.take_profit_pct)
                )
                .to_string(),
                g.take_profit_on,
                true,
                view,
                |d, on| d.general.take_profit_on = on,
            ))
            .children(slider(store, "exp-gen-tp", true))
            .child(caption(
                t!(
                    "core_expert.gen_trailing_add",
                    // The shortest text that round-trips, not a fixed decimal count: the track's
                    // own step is 0.1, but a core is free to hold 0.375, and `{:.1}` would print it
                    // as a number that is neither the thumb's nor the wire's.
                    v = r.trailing_float.to_string()
                )
                .to_string(),
                true,
                p,
                cx,
            ))
            .children(slider(store, "exp-gen-trailing-add", true))
            // A flag, not the plain caption Moonbot draws: the enable behind this row is projected
            // AND masked, and the popup's copy of the same row toggles it. Drawn as a caption, the
            // level would be draggable while the rule it belongs to stayed off, with nothing on
            // this face of the gear able to turn it on.
            .child(flag(
                "exp-gen-vstop-on",
                t!(
                    "core_settings.gen_vstop_line",
                    v = g.vol_drop_level.to_string()
                )
                .to_string(),
                g.vstop_on,
                true,
                view,
                |d, on| d.general.vstop_on = on,
            ))
            .children(slider(store, "exp-gen-vstop", true)),
    );

    let order_handling = rows(cx)
        .child(caption(
            t!(
                "core_expert.gen_partial_sell",
                v = r.auto_sell_partial.to_string()
            )
            .to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-gen-partial", true))
        .child(caption(
            t!("core_expert.gen_auto_cancel", v = auto_cancel_text(r)).to_string(),
            true,
            p,
            cx,
        ))
        .children(slider(store, "exp-gen-auto-cancel", true))
        .child(flag(
            "exp-gen-cancel-buy",
            t!("core_expert.gen_cancel_buy_after_sell").to_string(),
            r.cancel_buy_on_sell_fill,
            true,
            view,
            |d, on| d.order_rules.cancel_buy_on_sell_fill = on,
        ))
        .child(flag(
            "exp-gen-wall",
            t!("core_expert.gen_sell_under_wall").to_string(),
            r.sell_auto_move,
            true,
            view,
            |d, on| d.order_rules.sell_auto_move = on,
        ));

    // --- The default entry: Moonbot's main-window controls, see the module doc ----------------
    let entry = group(
        "exp-gen-entry",
        t!("core_expert.gen_entry_frame").to_string(),
    )
    .child(
        rows(cx)
            .child(hint(t!("core_expert.gen_entry_hint").to_string(), p, cx))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 8.0))
                    .child(flag(
                        "exp-gen-fixed-on",
                        t!("core_expert.gen_fixed_balance").to_string(),
                        r.use_manual_balance,
                        true,
                        view,
                        |d, on| d.order_rules.use_manual_balance = on,
                    ))
                    .children(num(store, "exp-gen-fixed-balance", 96.0, true, cx)),
            )
            // Dimmed rather than disabled while the fixed amount is on: the core ignores the share
            // then, but the value is still the core's and still editable for when it is switched
            // back.
            .child(caption(
                t!("core_expert.gen_play_with", v = play_with_text(r.play_with)).to_string(),
                !r.use_manual_balance,
                p,
                cx,
            ))
            .children(slider(store, "exp-gen-play-with", true))
            .child(caption(
                t!("core_expert.gen_buy_offset", v = buy_offset_text(r.x_ask)).to_string(),
                true,
                p,
                cx,
            ))
            .children(slider(store, "exp-gen-buy-offset", true))
            .child(radio_live(
                "exp-gen-ask-current",
                t!("core_expert.gen_ask_current").to_string(),
                r.use_current_ask,
                view,
                |d| d.order_rules.use_current_ask = true,
            ))
            .child(radio_live(
                "exp-gen-ask-30s",
                t!("core_expert.gen_ask_30s").to_string(),
                !r.use_current_ask,
                view,
                |d| d.order_rules.use_current_ask = false,
            )),
    );

    // Moonbot's clipboard transfer of its own settings, drawn dead because nothing here implements
    // it yet — NOT because it cannot be. The window holds the whole snapshot, and moonproto exports
    // the very format those buttons use (`shared_config::to_mbsc_string` / `from_mbsc_string` and
    // the `.mbshare` byte form), so both are a piece of work rather than a limit of the wire.
    let transfer = group(
        "exp-gen-transfer",
        t!("core_expert.gen_transfer_frame").to_string(),
    )
    .child(
        rows(cx)
            .child(hint(t!("core_expert.gen_transfer_hint").to_string(), p, cx))
            .child(
                h_flex()
                    .w_full()
                    .gap(design::ui_px(cx, 8.0))
                    .child(action(
                        "exp-gen-copy",
                        t!("core_expert.gen_copy_clipboard").to_string(),
                        false,
                    ))
                    .child(action(
                        "exp-gen-paste",
                        t!("core_expert.gen_paste").to_string(),
                        false,
                    )),
            ),
    );

    // --- Right: the blacklist and the two switches Moonbot groups with it ------------------------
    let weak = view.downgrade();
    let risks = group("exp-gen-risks", t!("core_settings.frame_risks").to_string()).child(
        expert_blacklist_rows(
            flag(
                "exp-gen-bl-on",
                t!("core_settings.blacklist").to_string(),
                g.blacklist_on,
                true,
                view,
                |d, on| d.general.blacklist_on = on,
            ),
            flag(
                "exp-gen-bl-exclude",
                t!("core_settings.exclude_delta").to_string(),
                g.exclude_blacklisted_from_deltas,
                true,
                view,
                |d, on| d.general.exclude_blacklisted_from_deltas = on,
            ),
            field(store, "exp-gen-bl-text", true),
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 6.0))
                .child(caption(
                    t!(
                        "core_expert.gen_bl_fresh",
                        v = r.dont_buy_new_coins.to_string()
                    )
                    .to_string(),
                    true,
                    p,
                    cx,
                ))
                .children(slider(store, "exp-gen-bl-fresh", true))
                .child(flag(
                    "exp-gen-bl-delisted",
                    t!("core_expert.gen_dont_buy_delisted").to_string(),
                    r.dont_buy_delisted,
                    true,
                    view,
                    |d, on| d.order_rules.dont_buy_delisted = on,
                ))
                .child(flag(
                    "exp-gen-deltas-trades",
                    t!("core_expert.gen_deltas_use_trades").to_string(),
                    r.deltas_by_trades,
                    true,
                    view,
                    |d, on| d.order_rules.deltas_by_trades = on,
                )),
            &g.blacklist_text,
            bl_reveal,
            move |_window, app| {
                let _ = weak.update(app, |this, cx| this.remove_highlighted_blacklist(cx));
            },
            p,
            cx,
        ),
    );

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        // Moonbot's own first row, above the two columns.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(flag(
                    "exp-gen-analyze",
                    t!("core_expert.gen_analyze_on_start").to_string(),
                    r.analyze_on_start,
                    true,
                    view,
                    |d, on| d.order_rules.analyze_on_start = on,
                ))
                .child(flag(
                    "exp-gen-manual-mode",
                    t!("core_expert.gen_manual_mode").to_string(),
                    r.manual_mode,
                    true,
                    view,
                    |d, on| d.order_rules.manual_mode = on,
                )),
        )
        .child(columns(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 10.0))
                .child(stops)
                .child(order_handling)
                .child(entry)
                .child(transfer),
            risks,
            cx,
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests;
