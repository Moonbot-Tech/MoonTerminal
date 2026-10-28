//! What an update control does once it is pressed: fill the shared per-IP queue and let it run.
//!
//! Every function here goes through `session.enqueue_core_updates` or `retry_core_update`,
//! never `session.update_core_version` directly -- the queue itself is what serializes updates
//! one-per-IP, and calling the raw send would burst a command straight past it. This is also why
//! none of these arm anything like `core_run`'s `RunPending`: the queue's own phase, read back
//! through `core_update_phase`, is the waiting state a badge already draws.

use std::rc::Rc;

use gpui::{App, Entity, Window};
use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;
use moon_core::session::core_update::{CoreUpdatePhase, UpdateEnqueueReport};
use moon_ui::{MoonNotification, MoonWindowExt as _};
use rust_i18n::t;

use super::{open_named_dialog, skip_reasons};
use crate::Backend;

/// Enqueue every core in a scope -- one core or many -- for the requested target, filling the
/// same queue a single enqueue would, never a burst of commands.
///
/// One entry point for every size: a row, a server, a section and the footer all reach the queue
/// the same way, and all get the same notice when the press achieved nothing.
///
/// Args:
///     backend: Shared terminal state.
///     cores: Every core the pressed control stands for.
///     target: Release or a named build, applied to every core in the batch.
///     window: Window the press came from, where a refusal notice is shown.
///     app: Application context used to reach the session.
pub(crate) fn update_scope(
    backend: &Entity<Backend>,
    cores: &[CoreId],
    target: UpdateTarget,
    window: &mut Window,
    app: &mut App,
) {
    let report = backend.update(app, |backend, cx| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let report = backend.session.enqueue_core_updates(cores, target, now_ms);
        log::info!(
            "update enqueue for {} core(s) in scope: {} accepted, {} skipped offline/unreachable, {} skipped already tracked",
            cores.len(),
            report.accepted,
            report.skipped_offline,
            report.skipped_already,
        );
        cx.notify();
        report
    });
    notify_nothing_queued(report, window, app);
}

/// Retry one core whose last update attempt ended `Done`.
///
/// A RELEASE attempt is re-sent as it was. A NAMED attempt reopens the build-name field with the
/// old name in it: the usual reason a named attempt failed is the name itself, and re-sending it
/// blind repeated a bad paste until the terminal was restarted. Done in the field goes through
/// [`update_scope`], whose enqueue releases the lane this core's failure stalled.
///
/// The target comes from `session.last_update_target`, which `moon-core` retains for exactly
/// this purpose -- a `Done` phase carries only the outcome, never the target that produced it.
/// `finish_core` keeps the attempt's metadata alive rather than dropping it, precisely so this
/// stays reachable after `history` has evicted the record (a ring capped at 2000, while a failed
/// core's retry affordance stays offered indefinitely). When no target is on record this
/// REFUSES rather than falling back to a plain release build: installing a different build than
/// the user asked for on a HOT, money-handling path is not an acceptable substitute for losing
/// the retry.
///
/// Args:
///     backend: Shared terminal state.
///     core: Core to retry. Must currently be in a `Done` phase.
///     window: Window the press came from -- hosts the name field or a refusal notice.
///     app: Application context used to reach the session.
pub(crate) fn retry_core(
    backend: &Entity<Backend>,
    core: CoreId,
    window: &mut Window,
    app: &mut App,
) {
    let Some(target) = backend.read(app).session.last_update_target(core) else {
        log::warn!("core {core}: retry refused, no recorded target for its last attempt");
        return;
    };
    if let UpdateTarget::Named(previous) = target {
        let name = core_name(backend, core, app);
        open_named_dialog(
            backend.clone(),
            Rc::from([core]),
            Rc::from([name]),
            Some(previous),
            window,
            app,
        );
        return;
    }
    let accepted = backend.update(app, |backend, cx| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let accepted = backend.session.retry_core_update(core, target, now_ms);
        cx.notify();
        accepted
    });
    if accepted {
        log::info!("core {core}: update retried from a row control");
    } else {
        log::warn!("core {core}: retry rejected, not in a Done phase or no longer eligible");
        // Two reasons reach here: a second press landed before the row re-rendered, so the core
        // is tracked again, or the core stopped being eligible. Name the one that applies.
        let tracked = matches!(
            backend.read(app).session.core_update_phase(core),
            Some(phase) if !matches!(phase, CoreUpdatePhase::Done(_))
        );
        notify_nothing_queued(
            UpdateEnqueueReport {
                accepted: 0,
                skipped_offline: usize::from(!tracked),
                skipped_already: usize::from(tracked),
            },
            window,
            app,
        );
    }
}

/// The core's configured name, as every update dialog shows it.
fn core_name(backend: &Entity<Backend>, core: CoreId, app: &App) -> String {
    backend
        .read(app)
        .config
        .servers
        .iter()
        .find(|server| server.id == core)
        .map(|server| server.name.clone())
        .unwrap_or_else(|| core.to_string())
}

/// Say so when a press queued nothing at all.
///
/// Only the WHOLE refusal gets a notice. A partial skip was already announced before the press --
/// by the server button's tooltip, the menu's leading row, or the confirm's list of names -- and
/// repeating it on every press of a server with one core offline would be noise. A press that
/// queued nothing changed nothing on screen, which is exactly the silence the update page used to
/// answer with.
///
/// Args:
///     report: What the queue accepted and skipped.
///     window: Window that shows the notice.
///     app: Application context for the notice.
fn notify_nothing_queued(report: UpdateEnqueueReport, window: &mut Window, app: &mut App) {
    if report.accepted > 0 {
        return;
    }
    let reasons = skip_reasons(report.skipped_offline, report.skipped_already);
    if reasons.is_empty() {
        return;
    }
    window.push_notification(
        MoonNotification::warning(
            t!(
                "core_update.notice.none",
                reasons = reasons.join(" \u{2014} ")
            )
            .to_string(),
        ),
        app,
    );
}
