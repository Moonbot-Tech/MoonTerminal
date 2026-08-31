//! What an update control does once it is pressed: fill the shared per-IP queue and let it run.
//!
//! Every function here goes through `session.enqueue_core_update(s)` or `retry_core_update`,
//! never `session.update_core_version` directly -- the queue itself is what serializes updates
//! one-per-IP, and calling the raw send would burst a command straight past it. This is also why
//! none of these arm anything like `core_run`'s `RunPending`: the queue's own phase, read back
//! through `core_update_phase`, is the waiting state a badge already draws.

use std::rc::Rc;

use gpui::{App, Entity};
use moon_core::feed::UpdateTarget;
use moon_core::session::CoreId;

use crate::Backend;

/// Enqueue one core for the requested update target.
///
/// Args:
///     backend: Shared terminal state.
///     core: Core to enqueue.
///     target: Release or a named build.
///     app: Application context used to reach the session.
pub(crate) fn update_core(
    backend: &Entity<Backend>,
    core: CoreId,
    target: UpdateTarget,
    app: &mut App,
) {
    backend.update(app, |backend, cx| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        if backend.session.enqueue_core_update(core, target, now_ms) {
            log::info!("core {core}: update enqueued from a row control");
        } else {
            // Not silent, and not a toast either: the row that offered this button already read
            // the core as offerable, so a rejection here means the core's state moved between the
            // frame that drew the button and the click on it -- the same race `core_run::restart`
            // accepts for the same reason.
            log::warn!("core {core}: update enqueue rejected, core no longer eligible");
        }
        cx.notify();
    });
}

/// Enqueue every core in a scope for the requested target -- filling the same queue a single
/// enqueue would, never a burst of commands.
///
/// Args:
///     backend: Shared terminal state.
///     cores: Every core the pressed control stands for.
///     target: Release or a named build, applied to every core in the batch.
///     app: Application context used to reach the session.
pub(crate) fn update_scope(
    backend: &Entity<Backend>,
    cores: &Rc<[CoreId]>,
    target: UpdateTarget,
    app: &mut App,
) {
    let cores = cores.clone();
    backend.update(app, |backend, cx| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let report = backend.session.enqueue_core_updates(&cores, target, now_ms);
        log::info!(
            "update enqueue for {} core(s) in scope: {} accepted, {} skipped offline/unreachable, {} skipped already tracked",
            cores.len(),
            report.accepted,
            report.skipped_offline,
            report.skipped_already,
        );
        cx.notify();
    });
}

/// Enqueue the fleet for a plain release update -- every core, or only the ones behind.
///
/// Args:
///     backend: Shared terminal state.
///     only_behind: `true` selects through `session.cores_behind()`; `false` takes every core the
///         enqueue gate accepts.
///     app: Application context used to reach the session.
pub(crate) fn update_fleet(backend: &Entity<Backend>, only_behind: bool, app: &mut App) {
    backend.update(app, |backend, cx| {
        let now_ms = moon_core::util::now_unix_ms_i64();
        let cores: Vec<CoreId> = if only_behind {
            backend.session.cores_behind()
        } else {
            backend.session.sessions().iter().map(|s| s.id).collect()
        };
        let report =
            backend
                .session
                .enqueue_core_updates(&cores, UpdateTarget::Release, now_ms);
        log::info!(
            "fleet update enqueue ({}): {} accepted, {} skipped offline/unreachable, {} skipped already tracked",
            if only_behind { "behind only" } else { "every core" },
            report.accepted,
            report.skipped_offline,
            report.skipped_already,
        );
        cx.notify();
    });
}

/// Retry one core whose last update attempt ended `Done`, using the target its last attempt
/// used.
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
///     app: Application context used to reach the session.
pub(crate) fn retry_core(backend: &Entity<Backend>, core: CoreId, app: &mut App) {
    backend.update(app, |backend, cx| {
        let Some(target) = backend.session.last_update_target(core) else {
            log::warn!("core {core}: retry refused, no recorded target for its last attempt");
            return;
        };
        let now_ms = moon_core::util::now_unix_ms_i64();
        if backend.session.retry_core_update(core, target, now_ms) {
            log::info!("core {core}: update retried from a row control");
        } else {
            log::warn!("core {core}: retry rejected, not in a Done phase or no longer eligible");
        }
        cx.notify();
    });
}
