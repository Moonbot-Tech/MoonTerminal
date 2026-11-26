//! The forced cancellation of a stuck order (MoonBot 7.73 N2+): its menu row and the question
//! asked before anything is sent.
//!
//! MoonBot's own order list offers it on the right click, and so does this menu — but only for an
//! order whose worker holds an active BUY or SELL leg, and only on a core whose runtime report
//! proves the build (`moon_core::feed::CoreFeatures`). The consequence is not an ordinary cancel's:
//! after three unconfirmed checks the core stops tracking the order and keeps cancelling it in the
//! background, so the order can remain open on the exchange. The confirmation says exactly that,
//! and the toast after Yes says "sent", because nothing here knows the exchange's answer.

use gpui::{App, Entity, ParentElement as _, Styled as _, Window, div};
use moon_core::feed::ForceCancelGate;
use moon_core::session::CoreId;
use moon_ui::{
    MoonButton, MoonButtonVariant, MoonMenuItem, MoonNotification, MoonTone, MoonWindowExt as _,
    h_flex,
};
use rust_i18n::t;

use super::{CoinMenuCtx, workspace_action_allows_cores};
use crate::{Backend, design};

/// The one dialog id, so a second request while the question is up replaces it.
const DIALOG_ID: &str = "coin-force-cancel-confirm";

/// Read the gate for one order of one core from the current store.
///
/// An order the store no longer holds is not eligible: there is nothing left to force.
///
/// Args:
///     b: Backend whose session store holds the core's orders and features.
///     core: Core that owns the order.
///     uid: The clicked order, or `None` for a token point without an order.
///
/// Returns:
///     The gate `CoreFeatures::force_cancel_gate` gives for that order's status.
pub(super) fn gate(b: &Backend, core: CoreId, uid: Option<u64>) -> ForceCancelGate {
    uid.and_then(|uid| order_state(b, core, uid))
        .map(|(gate, _)| gate)
        .unwrap_or(ForceCancelGate::NotEligible)
}

/// The gate and the worker status of one order, or `None` when the store no longer holds it.
///
/// The status is the leg MoonProto will force: the runtime picks it when it processes the
/// request, not when the user was asked. The row captures it when the menu is built — the leg the
/// user is looking at — and both the click and the answer refuse once the store reports another
/// one, so a buy that filled into `SellSet` meanwhile does not have its new take-profit forced in
/// its place. That narrows the race to the store's own lag behind MoonProto; it cannot close it,
/// because `orders().force_cancel` has no guarded form that would carry the leg to the runtime.
fn order_state(b: &Backend, core: CoreId, uid: u64) -> Option<(ForceCancelGate, String)> {
    let data = b.session.store().core(core)?;
    let order = data.orders.iter().find(|order| order.uid == uid)?;
    let gate = data
        .features
        .force_cancel_gate(&order.status, order.job_is_done);
    Some((gate, order.status.clone()))
}

/// The active row: opens the confirmation and sends nothing by itself.
///
/// Args:
///     ctx: The clicked surface's context.
///     backend: Backend entity the click and the answer re-read.
///     b: The same backend, read while the menu is built, for the leg on screen.
///     uid: The clicked order.
pub(super) fn item(
    ctx: &CoinMenuCtx,
    backend: &Entity<Backend>,
    b: &Backend,
    uid: u64,
) -> MoonMenuItem {
    // The leg the menu was opened on; an order the store lost reads as "" and never matches.
    let leg = order_state(b, ctx.core, uid)
        .map(|(_, status)| status)
        .unwrap_or_default();
    let backend = backend.clone();
    let core = ctx.core;
    let core_name = ctx.core_name.clone();
    let market = ctx.market.clone();
    let workspace_group = ctx.workspace_group.clone();
    MoonMenuItem::with_key(
        "coin-order-force-cancel",
        t!("chart.order_menu.force_cancel").to_string(),
    )
    .tone(MoonTone::Danger)
    .on_click(move |_, window, app| {
        window.close_context_menu(app);
        if order_state(backend.read(app), core, uid)
            != Some((ForceCancelGate::Offered, leg.clone()))
        {
            window.push_notification(
                MoonNotification::warning(t!("chart.order_menu.force_cancel_stale").to_string()),
                app,
            );
            return;
        }
        ask(
            Target {
                backend: backend.clone(),
                core,
                uid,
                leg: leg.clone(),
                core_name: core_name.clone(),
                market: market.clone(),
                workspace_group: workspace_group.clone(),
            },
            window,
            app,
        );
    })
}

/// The disabled row on a core that has not proved it acts on the request, with that reason.
pub(super) fn needs_core_item() -> MoonMenuItem {
    MoonMenuItem::with_key(
        "coin-order-force-cancel-needs-core",
        t!("chart.order_menu.force_cancel_needs_core").to_string(),
    )
    .disabled(true)
}

/// Everything the confirmation needs, captured when the row was clicked.
#[derive(Clone)]
struct Target {
    backend: Entity<Backend>,
    core: CoreId,
    uid: u64,
    /// Worker status when the menu was built: `BuySet` or `SellSet`.
    leg: String,
    core_name: String,
    market: String,
    workspace_group: Option<String>,
}

/// Ask before forcing; only Force sends, and only while the order is still eligible.
///
/// The dialog can stay open while the order moves on — it fills, the core reconnects, Auto
/// switches the group — so the answer re-checks the workspace authority, the gate and the leg at
/// the moment of the click, the way every other mutating row re-checks its core.
fn ask(target: Target, window: &mut Window, app: &mut App) {
    window.open_unique_moon_dialog(DIALOG_ID, app, move |dialog, _, cx| {
        let body = t!(
            "chart.order_menu.force_cancel_body",
            market = target.market.as_str(),
            core = target.core_name.as_str()
        )
        .to_string();
        let target = target.clone();
        crate::panels::order_dialog_chrome(
            dialog,
            440.0,
            t!("chart.order_menu.force_cancel_title").to_string(),
            cx,
        )
        // Scaled with the UI zoom, like the text inside it.
        .w(design::ui_px(cx, 440.0))
        .content(move |content, _, _| content.child(div().w_full().child(body.clone())))
        .footer(
            h_flex()
                .w_full()
                .gap(design::ui_px(cx, 8.0))
                .justify_end()
                .child(
                    MoonButton::new("coin-force-cancel-keep")
                        .outline()
                        .label(t!("chart.order_menu.force_cancel_keep").to_string())
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .render(),
                )
                .child(
                    MoonButton::new("coin-force-cancel-send")
                        .variant(MoonButtonVariant::Danger)
                        .label(t!("chart.order_menu.force_cancel_send").to_string())
                        .on_click(move |_, window, app| {
                            window.close_dialog(app);
                            confirm(&target, window, app);
                        })
                        .render(),
                ),
        )
    });
}

/// Send the forced cancellation for an accepted question and tell the user what happened.
fn confirm(target: &Target, window: &mut Window, app: &mut App) {
    let Target {
        backend,
        core,
        uid,
        leg,
        core_name,
        workspace_group,
        ..
    } = target;
    let (core, uid) = (*core, *uid);
    let outcome = backend.update(app, |b, _| {
        if !workspace_action_allows_cores(b, workspace_group.as_deref(), &[core]) {
            return Outcome::Stale;
        }
        if order_state(b, core, uid) != Some((ForceCancelGate::Offered, leg.clone())) {
            return Outcome::Stale;
        }
        match b.session.force_cancel_order(core, uid) {
            Ok(()) => Outcome::Sent,
            Err(err) => {
                log::warn!("coin_menu: force cancel {uid}@{core} failed: {err:#}");
                Outcome::Failed
            }
        }
    });
    let note = match outcome {
        Outcome::Sent => MoonNotification::info(
            t!(
                "chart.order_menu.force_cancel_sent",
                core = core_name.as_str()
            )
            .to_string(),
        ),
        Outcome::Stale => {
            MoonNotification::warning(t!("chart.order_menu.force_cancel_stale").to_string())
        }
        Outcome::Failed => MoonNotification::warning(
            t!(
                "chart.order_menu.force_cancel_failed",
                core = core_name.as_str()
            )
            .to_string(),
        ),
    };
    window.push_notification(note, app);
}

/// What became of an accepted question.
enum Outcome {
    /// Queued for the core; its own order updates say what the exchange did.
    Sent,
    /// The order or the workspace changed while the question was up, so nothing was sent.
    Stale,
    /// The core's command channel refused it.
    Failed,
}
