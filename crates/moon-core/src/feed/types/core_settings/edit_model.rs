//! Core configuration freshness and edit lifecycle models.

use super::{CoreConfig, FieldMask};

/// Trust classification for a core's manual-config projection, mirroring
/// [`crate::session::store::BalanceState`] (same `has_value()`/`is_current()`/`code()` contracts,
/// same reason for `code()`) but without an `Unpriced` arm: a shared config carries no derived
/// valuation that pricing could invalidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigState {
    /// A projection exists, the connection is ready, and no stale marker remains.
    Live,
    /// A projection exists but the connection is not ready or became stale since the last one.
    Stale,
    /// No projection has ever arrived for this core.
    Awaiting,
}

impl CoreConfigState {
    /// Whether there is a usable projection to render.
    pub fn has_value(self) -> bool {
        matches!(self, CoreConfigState::Live | CoreConfigState::Stale)
    }

    /// Whether the store classifies the projection as current enough to show without a stale
    /// marker.
    pub fn is_current(self) -> bool {
        matches!(self, CoreConfigState::Live)
    }

    /// Stable small integer for hashing this state into a render signature.
    ///
    /// Exists so consumers do not invent their own numbering: the exhaustive match keeps a new
    /// variant a compile error here rather than a silently unhashed state somewhere downstream.
    pub fn code(self) -> u64 {
        match self {
            CoreConfigState::Live => 1,
            CoreConfigState::Stale => 2,
            CoreConfigState::Awaiting => 3,
        }
    }
}

/// Coarse AREA of the projection that differs from what was requested. Names a surface, carries no
/// value — used when the difference cannot be pinned to an exact field, which is every area but
/// the manual money fields below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigArea {
    AutoBuy,
    /// The marked-markets list — see [`crate::feed::CoreConfig::fav_markets`].
    FavMarkets,
    Special,
    Telegram,
    AutoStart,
    BtcBlink,
    General,
    /// The Hotkeys page's mouse-gesture block — see [`crate::feed::GestureSettings`].
    Gestures,
    Interface,
    Leverage,
    Manual,
    /// The part of the General page only the expert window draws — see [`crate::feed::OrderRulesSettings`].
    OrderRules,
    Signals,
}

/// What a shared-config echo disagreed with the terminal about, restricted to the fields THIS edit
/// actually asked to change — never the whole projection; see `feed::live::shared_config`'s module
/// doc and [`crate::feed::live::FieldMask`]. `moon-core` cannot localize (`rust_i18n::i18n!` is
/// declared once in `moon-ui-gpui/src/main.rs`), so this reaches the UI as typed data and is
/// captioned there, the same discipline [`crate::feed::ConnFaultKind`] and [`crate::feed::CoreHotkeyAction`]
/// follow.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreConfigRejection {
    /// One or more coarse sections still differ.
    Areas(Vec<CoreConfigArea>),
}

/// Phase of a core-config edit that has not yet reached a terminal outcome, mirroring
/// [`crate::feed::StrategyEditPhase`]'s shape: a resolution is a one-time fact carried by
/// [`CoreConfigEditEvent::Resolved`], never a phase a retained row sits in, so `Confirmed` is not a
/// variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreConfigEditPhase {
    Pending,
    /// The core never reflected this edit after `MAX_ATTEMPTS` sends. Replaces goal A's
    /// `TimedOut`: upstream's transport retries on a monotonic echo timeout rather than owning its
    /// own wall clock, so the terminal state is exhausting the retry budget, not a bare timeout.
    GaveUp,
}

/// Terminal outcome of one echo comparison for a queued core-config edit.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreConfigEditResult {
    /// The core's snapshot now matches everything requested.
    Confirmed,
    /// The echo still disagrees on fields this edit touched; the queue keeps retrying up to
    /// `MAX_ATTEMPTS` — this is NOT the terminal state.
    NotApplied(CoreConfigRejection),
    /// The retry budget is exhausted; the edit is dropped from the queue.
    GaveUp,
}

/// One core-config edit's retained state, for the toolbar and popup's per-cell notices.
///
/// Boxed everywhere it travels through [`crate::feed::FeedMsg`]: [`crate::feed::ManualSettings`] alone makes
/// [`crate::feed::CoreConfig`] the largest field among this crate's other message payloads, and embedding it
/// unboxed here would make it `FeedMsg`'s own largest arm.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreConfigEditRow {
    pub phase: CoreConfigEditPhase,
    pub submitted_at_ms: i64,
    /// The projection this edit asked the core to hold.
    ///
    /// Authoritative only inside [`Self::touched`]: it is the projection of the PACKET that went
    /// out, whose other areas are whatever the core's snapshot held at that moment. Nothing renders
    /// it; it exists so the store can tell one edit from another.
    pub config: CoreConfig,
    /// Which areas of [`Self::config`] this edit actually asked to change.
    ///
    /// Carried so the store can compare two submissions WITHIN the mask. Without it the only
    /// available comparison was whole-projection equality, and an area the edit never named,
    /// drifting on the core between two attempts, made a retry look like a different edit.
    pub touched: FieldMask,
    /// The most recent rejection this edit received, if any. Retained across a retry's own
    /// `Submitted` event and cleared only by [`CoreConfigEditResult::Confirmed`] or a fresh user
    /// edit — never by a retry of the same edit; this is a store-arm rule, applied in
    /// `session::store`. What counts as "the same edit" is [`Self::touched`] plus equality within
    /// it.
    pub mismatches: Option<CoreConfigRejection>,
}

/// Core-config edit lifecycle event, published alongside [`crate::feed::FeedMsg::CoreConfig`] so
/// the UI can show a submitted-but-unconfirmed edit and its eventual verdict.
#[derive(Debug, Clone)]
pub enum CoreConfigEditEvent {
    /// A queued edit (or coalesced batch) was just sent and is awaiting its echo.
    Submitted(Box<CoreConfigEditRow>),
    /// The most recently submitted edit reached one echo's verdict.
    Resolved(CoreConfigEditResult),
    /// The core's write queue just ran empty: every edit queued for it has left — confirmed by
    /// an echo, dropped because the core already held it (no packet, so no echo), or given up on.
    ///
    /// The one signal that means "the retained snapshot reflects everything sent, or the give-up
    /// says otherwise". A surface that remembers what it sent until the core caught up waits for
    /// THIS, not for a snapshot: an edit the core already satisfies is dropped without a send,
    /// and a surface counting echoes would wait for one that never comes.
    Drained,
}
