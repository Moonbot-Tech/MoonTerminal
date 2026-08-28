//! What a run button does once it is pressed: send the intent, remember that we are waiting, and
//! arrange for the waiting state to expire on its own.
//!
//! Both actions are deliberately unconfirmed. They are reversible by the button beside them, and a
//! confirmation step on a control that lives in a table row would be a modal per row.
//!
//! Only REACHABLE cores are commanded. The per-core command channel outlives a disconnect — a feed
//! thread retries in place — so a command queued for a core that is down is not dropped but
//! replayed whenever it comes back, which for a trading action can be an hour later and nothing
//! like what the press meant.

use std::rc::Rc;
use std::time::Instant;

use gpui::{App, Entity};
use moon_core::session::CoreId;

use super::pending::{PENDING_TIMEOUT, RunAsk};
use crate::Backend;

/// Start or restart ONE core's market runtime.
///
/// Args:
///     backend: Shared terminal state.
///     core: Core to restart.
///     app: Application context used to send and to arm the waiting state.
pub(crate) fn restart(backend: &Entity<Backend>, core: CoreId, app: &mut App) {
    backend.update(app, |backend, cx| {
        // Reachable only, like the trading action below: a core can drop between the frame that
        // drew the button and the click on it, and the command channel outlives the disconnect.
        if !backend.session.core_run_state(core).online {
            log::warn!("core {core}: restart skipped, the core is not connected");
            return;
        }
        match backend.session.restart_now(core) {
            Ok(()) => {
                // Restart is only ever offered by a single-core control.
                backend
                    .run_pending
                    .arm(core, RunAsk::Restart, false, Instant::now());
                log::info!("core {core}: restart requested from a run control");
                expire_later(backend, cx);
            }
            // Not silent, and not a toast either: the one case that produces this is a core whose
            // session is gone, which the same row already shows as offline. Nothing was armed, so
            // no expiry timer is owed — claiming one would block the next real press for 5 s.
            Err(error) => log::warn!("core {core}: restart failed: {error:#}"),
        }
        cx.notify();
    });
}

/// Start or stop the global strategy engine across a whole scope.
///
/// Args:
///     backend: Shared terminal state.
///     cores: Every core the pressed control stands for.
///     on: Whether to start (`true`) or stop (`false`) trading.
///     app: Application context used to send and to arm the waiting state.
pub(crate) fn set_trading(
    backend: &Entity<Backend>,
    cores: &Rc<[CoreId]>,
    on: bool,
    app: &mut App,
) {
    let cores = cores.clone();
    backend.update(app, |backend, cx| {
        // Reachable cores only — see the module note on queued commands outliving a disconnect —
        // and only cores that are NOT already in the asked-for state. A core told to start what it
        // is already running answers nothing (the store suppresses an unchanged repeat), so the
        // control would sit in the waiting face for the whole timeout for no reason.
        let targets: Vec<CoreId> = cores
            .iter()
            .copied()
            .filter(|core| {
                let state = backend.session.core_run_state(*core);
                state.online && !(state.trading == Some(on) && state.trading_confirmed)
            })
            .collect();
        if targets.is_empty() {
            log::info!(
                "trading {} skipped: none of the {} core(s) in scope needs it",
                if on { "start" } else { "stop" },
                cores.len()
            );
            return;
        }
        // Armed for the cores that ACCEPTED the command. Arming the rest would leave a control
        // waiting on an answer to something nobody sent.
        let sent = backend.session.set_trading_many(&targets, on);
        let now = Instant::now();
        // Which control pressed this, so the waiting face appears on that control and not on its
        // neighbour commanding the same cores.
        let from_group = cores.len() > 1;
        for core in &sent {
            backend
                .run_pending
                .arm(*core, RunAsk::Trading(on), from_group, now);
        }
        log::info!(
            "trading {} requested for {}/{} core(s) that needed it, {} in scope",
            if on { "start" } else { "stop" },
            sent.len(),
            targets.len(),
            cores.len()
        );
        // Only when something was actually armed: an expiry timer owed to nothing would block the
        // next real press from getting one.
        if !sent.is_empty() {
            expire_later(backend, cx);
        }
        cx.notify();
    });
}

/// Wake the app once the waiting state can have expired, unless a sweep is already scheduled.
///
/// The register is read-only during render, so an intent the core never answers would keep its
/// button in the waiting face until some unrelated notification happened to repaint it. The wake
/// goes through the backend's own 250 ms notify gate — the rule for every background-originated
/// wake in this app — and only fires when the sweep actually dropped something.
///
/// Args:
///     backend: Shared terminal state holding the register.
///     cx: Backend context owning the timer.
fn expire_later(backend: &mut Backend, cx: &mut gpui::Context<Backend>) {
    // One timer at a time: without the claim, repeated presses each leave a detached task behind
    // and every one of them ends in a backend wake.
    if !backend.run_pending.claim_sweep() {
        return;
    }
    cx.spawn(async move |backend, cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        executor.timer(PENDING_TIMEOUT).await;
        cx.update(|cx| {
            let _ = backend.update(cx, |backend: &mut Backend, cx| {
                if backend.run_pending.sweep(Instant::now()) {
                    backend.mark_backend_dirty(cx);
                }
                // A press made while this timer was running has not reached its own timeout yet,
                // and the sweep just released the claim — so it needs the next timer scheduled
                // here, or its control would wait forever on a core that never answers.
                if !backend.run_pending.is_empty() {
                    expire_later(backend, cx);
                }
            });
        });
    })
    .detach();
}
