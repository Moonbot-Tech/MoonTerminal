//! Whether a cancel key is actually held, which orders this hold has already addressed, and why a
//! cancel was refused.
//!
//! GPUI's keystroke interceptor fires on key-DOWN only, and a key-up reaches only focus-routed
//! element listeners, apart from the two best-effort capture-phase listeners this design adds.
//! The hold therefore cannot be stored as truth: `poll` is the fail-safe, and on Windows it asks
//! the hardware every time. A latch that stayed ON would cancel live orders on a real exchange
//! with no key pressed at all.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use moon_core::session::CoreId;

/// The physical key a hold is bound to — only keys with an OS probe. Anything else is press-only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HoldKey {
    Tab,
    Delete,
}

impl HoldKey {
    /// Map a keystroke name onto a probeable key.
    ///
    /// Only the lowercase GPUI names `"tab"` and `"delete"` match. A user-bound FigDelete key such
    /// as `"x"` is press-only: it may arm bookkeeping, but `poll` never reports it live.
    pub(crate) fn from_key_name(name: &str) -> Option<Self> {
        match name {
            "tab" => Some(Self::Tab),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// How this evaluation of the cancel key reached the chart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PressKind {
    /// A new arm: `explicit` is a real key-down; `false` is a caller re-evaluating without one.
    Fresh { explicit: bool },
    /// Auto-repeat or a later poll of the same key and window.
    Repeat,
}

/// Non-Windows only: how long after the last key-down/repeat a hold stays live without a probe.
pub(crate) const HOLD_TTL: Duration = Duration::from_millis(1000);

/// Generic over the window identity so tests use `u32`; production uses `gpui::AnyWindowHandle`.
pub(crate) struct CancelHold<W: Copy + PartialEq> {
    arm: Option<Arm<W>>,
}

struct Arm<W> {
    key: Option<HoldKey>,
    window: W,
    last_key_event: Instant,
    addressed: HashSet<(CoreId, u64)>,
}

impl<W: Copy + PartialEq> Default for CancelHold<W> {
    fn default() -> Self {
        Self { arm: None }
    }
}

impl<W: Copy + PartialEq> CancelHold<W> {
    /// Key-down. The OS `repeat` flag is the returned kind: a repeat is always `Repeat`, a first
    /// press is always `Fresh { explicit: true }`. Same key+window extends the arm (keeps
    /// `addressed`); anything else replaces it with an empty set.
    ///
    /// A queued OS repeat after the arm was cleared must not be relabelled `Fresh` — that path
    /// ignores `!live` and would cancel with the key already up.
    pub(crate) fn press(
        &mut self,
        key: Option<HoldKey>,
        window: W,
        repeat: bool,
        now: Instant,
    ) -> PressKind {
        // Two different unbound keys both map to `None`, so press-only arms alias each other. That is
        // safe ONLY because `poll` refuses a `key: None` arm outright; do not make them pollable
        // without giving them an identity first.
        let same_arm = self
            .arm
            .as_ref()
            .is_some_and(|arm| arm.key == key && arm.window == window);
        if repeat && same_arm {
            if let Some(arm) = self.arm.as_mut() {
                arm.last_key_event = now;
            }
            return PressKind::Repeat;
        }
        self.arm = Some(Arm {
            key,
            window,
            last_key_event: now,
            addressed: HashSet::new(),
        });
        if repeat {
            PressKind::Repeat
        } else {
            PressKind::Fresh { explicit: true }
        }
    }

    /// Key-up (best-effort root listener) — clears only if it is the armed key.
    pub(crate) fn release(&mut self, key: HoldKey) {
        if self.arm.as_ref().is_some_and(|arm| arm.key == Some(key)) {
            self.clear();
        }
    }

    /// Drop the arm entirely. The next press starts a new hold.
    pub(crate) fn clear(&mut self) {
        self.arm = None;
    }

    /// Whether an arm exists, including a press-only key that never polls live.
    pub(crate) fn is_armed(&self) -> bool {
        self.arm.is_some()
    }

    /// The probeable key this arm is bound to, or `None` when unarmed or press-only.
    pub(crate) fn armed_key(&self) -> Option<HoldKey> {
        self.arm.as_ref().and_then(|arm| arm.key)
    }

    /// The window this arm was taken in, if any.
    pub(crate) fn armed_window(&self) -> Option<W> {
        self.arm.as_ref().map(|arm| arm.window)
    }

    /// THE fail-safe gate. `active` is "the armed window is the platform's active window";
    /// `probe` is Some(physical state) on Windows, None elsewhere. Any doubt clears the hold.
    ///
    /// Every branch that cannot prove the key is down resolves to "not held". A press-only arm
    /// (`key: None`) never polls live and is not cleared — it exists for repeat/dedupe bookkeeping.
    pub(crate) fn poll(&mut self, now: Instant, active: bool, probe: Option<bool>) -> bool {
        let Some(arm) = self.arm.as_ref() else {
            return false;
        };
        if arm.key.is_none() {
            return false;
        }
        let last_key_event = arm.last_key_event;
        if !active {
            self.clear();
            return false;
        }
        match probe {
            Some(false) => {
                self.clear();
                false
            }
            Some(true) => true,
            None if now.saturating_duration_since(last_key_event) > HOLD_TTL => {
                self.clear();
                false
            }
            None => true,
        }
    }

    /// Per-hold dedupe: `true` the first time a `(core, uid)` is addressed in this hold.
    ///
    /// With no arm at all this still returns `true` — a single press with no hold must still act.
    pub(crate) fn address(&mut self, target: (CoreId, u64)) -> bool {
        match self.arm.as_mut() {
            Some(arm) => arm.addressed.insert(target),
            None => true,
        }
    }
}

/// Windows: real-time physical state, independent of focus and of the message queue.
///
/// `GetAsyncKeyState` rather than `GetKeyState`: the latter reads the thread's message queue and
/// lags behind the hardware. Only the high-order bit is the "currently down" test; the low-order
/// bit is "pressed since the last call" and would report a tapped-and-released key as still held.
#[cfg(windows)]
pub(crate) fn physical_key_down(key: HoldKey) -> Option<bool> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_DELETE, VK_TAB};
    let vk = match key {
        HoldKey::Tab => VK_TAB,
        HoldKey::Delete => VK_DELETE,
    };
    // SAFETY: GetAsyncKeyState reads a process-independent key-state table and takes no pointers.
    Some(unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000 != 0)
}

/// Non-Windows: no hardware probe; `poll` falls through to `HOLD_TTL`.
#[cfg(not(windows))]
pub(crate) fn physical_key_down(_key: HoldKey) -> Option<bool> {
    None
}

/// Why a cancel was not sent. `NoTarget` is logged only on an explicit fresh press.
///
/// Per-order variants carry the target so a caller cannot log a refusal without the identity,
/// and cannot silently drop a `(refusal, None)` pair.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum CancelRefusal {
    NoTarget,
    CoreNotAllowed {
        core: CoreId,
        uid: u64,
    },
    NoOrderRow {
        core: CoreId,
        uid: u64,
    },
    NotCancellable {
        core: CoreId,
        uid: u64,
        status: String,
    },
}

/// Whether `status` is an entry that MoonProto will accept a Cancel Buy for.
///
/// Exactly `"None"` and `"BuySet"`. `SellSet` is a sell-side worker; `BuyDone` is a filled entry
/// whose exit is not yet set — both are refused rather than sent and dropped silently.
pub(crate) fn is_cancellable_status(status: &str) -> bool {
    matches!(status, "None" | "BuySet")
}

/// Classify a cancel attempt. Authorization outranks status: a disallowed core is
/// `CoreNotAllowed` even when the row looks cancellable.
pub(crate) fn classify_cancel(
    target: Option<(CoreId, u64)>,
    core_allowed: bool,
    status: Option<&str>,
) -> Result<(CoreId, u64), CancelRefusal> {
    let Some((core, uid)) = target else {
        return Err(CancelRefusal::NoTarget);
    };
    if !core_allowed {
        return Err(CancelRefusal::CoreNotAllowed { core, uid });
    }
    let Some(status) = status else {
        return Err(CancelRefusal::NoOrderRow { core, uid });
    };
    if !is_cancellable_status(status) {
        return Err(CancelRefusal::NotCancellable {
            core,
            uid,
            status: status.to_string(),
        });
    }
    Ok((core, uid))
}

/// Which refusals are reported for this press kind: `NoTarget` only on an explicit fresh press;
/// the per-order refusals always (they are already deduped per hold by `address`).
pub(crate) fn reports(refusal: &CancelRefusal, press: PressKind) -> bool {
    match refusal {
        CancelRefusal::NoTarget => matches!(press, PressKind::Fresh { explicit: true }),
        CancelRefusal::CoreNotAllowed { .. }
        | CancelRefusal::NoOrderRow { .. }
        | CancelRefusal::NotCancellable { .. } => true,
    }
}

#[cfg(test)]
mod tests;
