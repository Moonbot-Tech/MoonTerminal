//! What a Control press waits for, and the redraw of its message once the core has answered.
//!
//! A command to a core is an intent: it is queued, and the core reports its new state later. The
//! screen a press answers with is drawn before that report can arrive, so it shows the press as
//! waiting (⏳) and offers no second send. The bot then redraws the same message on its own — once
//! the core has reported the state that was asked for, or once it has waited long enough that the
//! silence is the answer — with what the core says now. Without it the chat kept the picture from
//! before the press until the next one, and a confirmed switch looked like one that did nothing.
//!
//! An ask is answered by the STATE it asked for, reported by the current connection — the rule the
//! desktop's run control keeps (`controls/core_run/pending.rs`), not a revision counter that also
//! moves on a reconnect or on the other switch's report.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::t;
use moon_core::session::{CoreId, CoreRunState, RunSwitch};
use moon_core::telegram::menu_action::ControlAction;
use moon_core::telegram::runtime::{drop_redraws, push_redraw};

use crate::TgHost;
use crate::control;

/// How long a run switch waits for the core before its silence is shown: the desktop's timeout.
pub(super) const RUN_WAIT: Duration = Duration::from_secs(5);

/// How long cancelled buys may take to leave the core's orders: one exchange round trip per
/// market.
const CANCEL_WAIT: Duration = Duration::from_secs(10);

/// The earliest a redraw goes after its press: the press's own answer is edited into the message
/// by the transport thread, and a redraw sent before it would be covered by the older picture.
const REDRAW_AFTER: Duration = Duration::from_secs(1);

/// One thing a press asked of a core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ask {
    /// Trading or AutoDetect switched on or off.
    Run {
        core: CoreId,
        switch: RunSwitch,
        on: bool,
    },
    /// One strategy checked or unchecked.
    Strategy { core: CoreId, id: u64, on: bool },
    /// The core's pending buys cancelled.
    CancelBuys { core: CoreId },
}

/// A message to redraw once its asks are answered.
#[derive(Clone, Debug)]
pub(crate) struct Redraw {
    /// The screen the message shows, drawn again from the core's state.
    screen: ControlAction,
    asks: Vec<Ask>,
    /// When the command was sent: the asks' waits count from it.
    at: Instant,
    /// The last press on the message: its answer is edited in after it, so the redraw goes no
    /// sooner than [`REDRAW_AFTER`] past it.
    pressed: Instant,
}

/// What became of a press's asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// The core reported every state asked for.
    Answered,
    /// The core stayed silent past the wait.
    Silent,
}

/// Record a run switch just sent to `core`: until the core reports `on`, or [`RUN_WAIT`] passes,
/// its card shows the switch waiting.
pub(super) fn arm_run(host: &mut dyn TgHost, core: CoreId, switch: RunSwitch, on: bool) {
    host.state_mut()
        .run_wanted
        .insert((core, switch), (on, Instant::now()));
}

/// The state a run switch of `core` was asked for and has not reported yet, within [`RUN_WAIT`].
pub(super) fn run_waiting(host: &dyn TgHost, core: CoreId, switch: RunSwitch) -> Option<bool> {
    let &(on, at) = host.state().run_wanted.get(&(core, switch))?;
    let state = host.session().core_run_state(core);
    (at.elapsed() < RUN_WAIT && !run_answered(state, switch, on)).then_some(on)
}

/// Whether `state` reports `switch` at `on`, said by the current connection: a value carried over
/// a reconnect describes the link before the press.
fn run_answered(state: CoreRunState, switch: RunSwitch, on: bool) -> bool {
    match switch {
        RunSwitch::Trading => state.trading == Some(on) && state.trading_confirmed,
        // AutoDetect rides the runtime-state command, so the runtime flag confirms it.
        RunSwitch::AutoDetect => state.auto_detect == Some(on) && state.started_confirmed,
    }
}

/// Whether the core has answered `ask`.
fn answered(host: &dyn TgHost, ask: Ask) -> bool {
    match ask {
        Ask::Run { core, switch, on } => {
            run_answered(host.session().core_run_state(core), switch, on)
        }
        Ask::Strategy { core, id, on } => {
            let checked = host.session().store().core(core).and_then(|data| {
                data.strategies
                    .iter()
                    .find(|row| row.id == id)
                    .map(|row| row.checked)
            });
            checked == Some(on) && !control::strategy_waiting(host, core, id)
        }
        Ask::CancelBuys { core } => control::pending_buys(host, core) == 0,
    }
}

/// How long `ask` is waited for.
fn wait_of(ask: Ask) -> Duration {
    match ask {
        Ask::Run { .. } => RUN_WAIT,
        Ask::Strategy { .. } => crate::mini_app::dto::STRATEGY_CONFIRM_WINDOW,
        Ask::CancelBuys { .. } => CANCEL_WAIT,
    }
}

/// Redraw `message` with `screen` once `asks` are answered. A press with no ask, or one answered
/// with a new message, leaves nothing to redraw.
pub(super) fn watch(
    host: &mut dyn TgHost,
    chat: i64,
    message: Option<i64>,
    screen: ControlAction,
    asks: Vec<Ask>,
) {
    let Some(message) = message.filter(|_| !asks.is_empty()) else {
        return;
    };
    host.state_mut().control_redraws.insert(
        (chat, message),
        Redraw {
            screen,
            asks,
            at: Instant::now(),
            pressed: Instant::now(),
        },
    );
}

/// What a press on `message` does to the redraw it waits for. A press that shows the very screen
/// the redraw will draw — a ⏳ button — leaves it waiting: that press only looks again. A command
/// answered on the same screen takes over the asks still unanswered, so a second switch pressed
/// while the first waits does not drop the first. Any other press withdraws it ([`forget`]).
///
/// Returns:
///     The asks the press's own redraw carries on.
pub(super) fn press(
    host: &mut dyn TgHost,
    chat: i64,
    message: i64,
    action: ControlAction,
) -> Vec<Ask> {
    let waiting = host
        .state()
        .control_redraws
        .get(&(chat, message))
        .map(|redraw| (redraw.screen, redraw.asks.clone()));
    if let Some((screen, _)) = waiting
        && screen == action
    {
        // This press's own answer is edited in after it: the redraw waits for that again.
        if let Some(redraw) = host.state_mut().control_redraws.get_mut(&(chat, message)) {
            redraw.pressed = Instant::now();
        }
        return Vec::new();
    }
    let carried = match waiting {
        Some((screen, asks)) if super::redrawn_as(action) == Some(screen) => asks
            .into_iter()
            .filter(|&ask| !answered(host, ask))
            .collect(),
        _ => Vec::new(),
    };
    forget(host, chat, message);
    carried
}

/// Forget the redraw of `message`, waiting or queued: a press on it has just answered with a
/// newer screen.
pub(super) fn forget(host: &mut dyn TgHost, chat: i64, message: i64) {
    host.state_mut().control_redraws.remove(&(chat, message));
    let Some(store) = crate::notify::tick::current_store(host) else {
        return;
    };
    let mut store = crate::notify::tick::lock_store(&store);
    let queued = store
        .file
        .outbox
        .iter()
        .any(|row| row.chat == chat && row.edit == Some(message) && row.redraw.is_some());
    if queued && let Err(error) = store.update(|file| drop_redraws(file, chat, message)) {
        log::warn!("telegram control: queued redraw of chat {chat} not dropped: {error:#}");
    }
}

/// Redraw every message whose asks are answered or out of time; the owner loop's tick.
pub(crate) fn tick(host: &mut dyn TgHost) {
    if host.state().run_wanted.is_empty() && host.state().control_redraws.is_empty() {
        return;
    }
    // A run switch's waiting face ends with its answer or its time; the entry goes then.
    let stale: Vec<(CoreId, RunSwitch)> = host
        .state()
        .run_wanted
        .iter()
        .filter(|(key, wanted)| {
            let ((core, switch), (on, at)) = (**key, **wanted);
            at.elapsed() >= RUN_WAIT
                || run_answered(host.session().core_run_state(core), switch, on)
        })
        .map(|(&key, _)| key)
        .collect();
    for key in stale {
        host.state_mut().run_wanted.remove(&key);
    }
    let due: Vec<((i64, i64), ControlAction, Outcome)> = host
        .state()
        .control_redraws
        .iter()
        .filter_map(|(&key, redraw)| {
            if redraw.pressed.elapsed() < REDRAW_AFTER {
                return None;
            }
            let waited = redraw.at.elapsed();
            if redraw.asks.iter().all(|&ask| answered(host, ask)) {
                return Some((key, redraw.screen, Outcome::Answered));
            }
            let longest = redraw.asks.iter().map(|&ask| wait_of(ask)).max()?;
            (waited >= longest).then_some((key, redraw.screen, Outcome::Silent))
        })
        .collect();
    if due.is_empty() {
        return;
    }
    let store = crate::notify::tick::current_store(host);
    for ((chat, message), screen, outcome) in due {
        host.state_mut().control_redraws.remove(&(chat, message));
        // A section hidden since, or a chat no longer the owner, gets no card back.
        if !super::shown(host) || !control::is_owner(host, chat) {
            continue;
        }
        let Some(store) = store.as_ref() else {
            log::debug!("telegram control: no notifications file, chat {chat} not redrawn");
            continue;
        };
        let said = match outcome {
            Outcome::Answered => t!("telegram.control.confirmed").to_string(),
            Outcome::Silent => t!("telegram.control.unconfirmed").to_string(),
        };
        let (html, keyboard) = super::html(super::draw(host, screen, said));
        let now_utc = i64::try_from(moon_core::util::time::now_unix_secs()).unwrap_or(i64::MAX);
        let queued = crate::notify::tick::lock_store(store)
            .update(|file| push_redraw(file, chat, message, html, keyboard, now_utc));
        match queued {
            Ok(true) => log::info!("telegram control: chat {chat} redrawn, {outcome:?}"),
            Ok(false) => log::warn!("telegram control: redraw of chat {chat} refused"),
            Err(error) => {
                log::warn!("telegram control: redraw of chat {chat} not queued: {error:#}")
            }
        }
    }
}

/// The redraws waiting, by chat and message.
pub(crate) type Redraws = HashMap<(i64, i64), Redraw>;

#[cfg(test)]
mod tests;
