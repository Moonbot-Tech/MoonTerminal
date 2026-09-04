//! Moonbot's "Основные" page, row for row.
//!
//! Nine of its controls are live, because the terminal both projects and may write them: take
//! profit and its level, the trailing stop and its level, V-Stop's level and enable flag, the coin
//! blacklist with its text and its delta filter.
//!
//! Everything else on the page is drawn and disabled for ONE reason: the field is in the safe-share
//! section but outside `moon_core::feed::GeneralSettings`, so there is nothing to seed the row from
//! and nothing OK could carry. That includes the stop-loss pair
//! (`trading.panic_if_price_drop` / `trading.price_drop_level`).
//!
//! Those two carry the wire's "Mirrors `ClientSettingsCommand::…`" note, and that note is NOT what
//! holds them back: five of the fields this page already writes carry it too — `trailing_stop`,
//! `g_take_profit`, `vol_drop_level`, `coins_black_list_text`, `use_coins_black_list`. The note
//! says the compact channel carries the same field, not that this one may not.
//!
//! The disabled rows still print their captions in Moonbot's wording, with an em dash where the
//! value would be: a row that showed `0.00%` would state a setting this terminal has not read.

use gpui::*;
use moon_ui::{MoonPalette, h_flex, v_flex};
use rust_i18n::t;

use moon_core::feed::CoreConfig;

use crate::design;
use crate::shell::editors::EditorStore;
use crate::shell::{TAKE_PROFIT_BOUNDS, TRAILING_BOUNDS, VSTOP_BOUNDS};

use super::super::CoreExpertView;
use super::super::widgets::{action, caption, columns, field, flag, group, hint, rows, slider};

/// Bounds of the dead sliders that count UP, and of the ones that count down from zero.
///
/// They bound a CONTROL that writes nothing, so a range only has to resemble Moonbot's. Both are
/// declared with a value of `0`, which each range reads as "no effect" — the least eventful thing a
/// thumb can say about a row whose real value this terminal has not read.
const DEAD_PCT: (f32, f32, f32) = (0.0, 100.0, 1.0);
const DEAD_DROP_PCT: (f32, f32, f32) = (-10.0, 0.0, 0.1);

/// Value shown where Moonbot prints a number this terminal has not read.
const NO_VALUE: &str = "—";

/// See [`super::field_specs`].
#[allow(clippy::type_complexity)]
pub(super) fn field_specs(
    draft: &CoreConfig,
) -> Vec<(&'static str, String, fn(&mut CoreConfig, &str))> {
    vec![(
        "exp-gen-bl-text",
        draft.general.blacklist_text.clone(),
        (|d, t| d.general.blacklist_text = t.to_string()) as fn(&mut CoreConfig, &str),
    )]
}

/// See [`super::slider_specs`].
///
/// The dead rows are declared alongside the live ones because `MoonSlider` cannot draw without a
/// state; theirs stages nothing.
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
    let dead = (|_: &mut CoreConfig, _: f32| {}) as fn(&mut CoreConfig, f32);
    vec![
        ("exp-gen-sl", DEAD_DROP_PCT, 0.0, dead, None),
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
        ("exp-gen-trailing-add", DEAD_PCT, 0.0, dead, None),
        (
            "exp-gen-vstop",
            VSTOP_BOUNDS,
            g.vol_drop_level as f32,
            |d, v| d.general.vol_drop_level = v.round() as i32,
            None,
        ),
        ("exp-gen-partial", DEAD_PCT, 0.0, dead, None),
        ("exp-gen-auto-cancel", DEAD_PCT, 0.0, dead, None),
        ("exp-gen-bl-fresh", DEAD_PCT, 0.0, dead, None),
    ]
}

/// Build the page.
pub(super) fn body(
    view: &Entity<CoreExpertView>,
    store: &EditorStore,
    draft: &CoreConfig,
    p: MoonPalette,
    cx: &App,
) -> AnyElement {
    let g = &draft.general;

    // --- Left: exits, order handling and Moonbot's own settings-transfer block -------------------
    let stops = group(
        "exp-gen-stops",
        t!("core_settings.gen_stops_frame").to_string(),
    )
    .child(
        rows(cx)
            .child(flag(
                "exp-gen-sl-on",
                t!("core_expert.gen_stop_loss", v = NO_VALUE).to_string(),
                false,
                false,
                view,
                |_, _| {},
            ))
            .children(slider(store, "exp-gen-sl", false))
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
                t!("core_expert.gen_trailing_add", v = NO_VALUE).to_string(),
                false,
                p,
                cx,
            ))
            .children(slider(store, "exp-gen-trailing-add", false))
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
            t!("core_expert.gen_partial_sell", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-gen-partial", false))
        .child(caption(
            t!("core_expert.gen_auto_cancel", v = NO_VALUE).to_string(),
            false,
            p,
            cx,
        ))
        .children(slider(store, "exp-gen-auto-cancel", false))
        .child(flag(
            "exp-gen-cancel-buy",
            t!("core_expert.gen_cancel_buy_after_sell").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ))
        .child(flag(
            "exp-gen-wall",
            t!("core_expert.gen_sell_under_wall").to_string(),
            false,
            false,
            view,
            |_, _| {},
        ));

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
    let risks = group("exp-gen-risks", t!("core_settings.frame_risks").to_string()).child(
        rows(cx)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(design::ui_px(cx, 10.0))
                    .child(flag(
                        "exp-gen-bl-on",
                        t!("core_settings.blacklist").to_string(),
                        g.blacklist_on,
                        true,
                        view,
                        |d, on| d.general.blacklist_on = on,
                    ))
                    .child(div().flex_1())
                    .child(flag(
                        "exp-gen-bl-exclude",
                        t!("core_settings.exclude_delta").to_string(),
                        g.exclude_blacklisted_from_deltas,
                        true,
                        view,
                        |d, on| d.general.exclude_blacklisted_from_deltas = on,
                    )),
            )
            .children(field(store, "exp-gen-bl-text", true))
            .child(caption(
                t!("core_expert.gen_bl_fresh", v = NO_VALUE).to_string(),
                false,
                p,
                cx,
            ))
            .children(slider(store, "exp-gen-bl-fresh", false))
            .child(flag(
                "exp-gen-deltas-trades",
                t!("core_expert.gen_deltas_use_trades").to_string(),
                false,
                false,
                view,
                |_, _| {},
            )),
    );

    v_flex()
        .w_full()
        .gap(design::ui_px(cx, 10.0))
        // Moonbot's own first row, above the two columns: a startup analysis switch and the green
        // help link beside it.
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap(design::ui_px(cx, 12.0))
                .child(flag(
                    "exp-gen-analyze",
                    t!("core_expert.gen_analyze_on_start").to_string(),
                    false,
                    false,
                    view,
                    |_, _| {},
                )),
        )
        .child(columns(
            v_flex()
                .w_full()
                .gap(design::ui_px(cx, 10.0))
                .child(stops)
                .child(order_handling)
                .child(transfer),
            risks,
            cx,
        ))
        .into_any_element()
}
