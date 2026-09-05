//! Reads and writes the core's full safe-share configuration for one core.
//!
//! MoonProto retains the last `SharedConfig` snapshot itself and requests it in the background
//! after `Ready`, so reading costs no extra request. Writing sends one COMPLETE snapshot, which
//! makes the same serialization problem the compact settings channel has: a second write built on a
//! stale base silently reverts the first. This module solves it the same way
//! [`super::client_settings::ClientSettingsSequence`] does — every write is rebuilt from the freshly
//! retained snapshot, and the next one waits for the core's echo.
//!
//! Adding a settings tab means adding its fields to [`CoreConfig`] and to `apply_core_config`;
//! the queue, the barrier, and the projection contract below stay unchanged. `apply_core_config`
//! additionally takes a [`FieldMask`] naming which of those fields THIS write may touch — an
//! addition to that contract, not a violation of it: a symmetric whole-projection write let one
//! queued edit silently restore another's field, and let a popup OK reach fields it never rendered.

use std::collections::VecDeque;
use std::time::Instant;

use moonproto::MoonClient;
use moonproto::shared_config::SharedConfig;

use crate::feed::{
    AutoBuySettings, AutoStartSettings, BtcBlinkSettings, CoreConfig, CoreConfigArea,
    CoreConfigEditEvent, CoreConfigEditPhase, CoreConfigEditResult, CoreConfigEditRow,
    CoreConfigRejection, CoreHotkeyAction, CoreHotkeyLayout, CoreStratButtons, GeneralSettings,
    GestureSettings, InterfaceSettings, LeverageSettings, ManualSettings, OrderRulesSettings,
    SignalsSettings, SpecialSettings, TelegramSettings, day_fraction_to_minutes,
    minutes_to_day_fraction,
};

/// Sends of one edit that may go unconfirmed before it is dropped.
///
/// The barrier alone cannot end a disagreement: if the core clamps a value the terminal sent, the
/// echo never matches what was queued, and an unbounded retry would send the full config forever.
/// Three attempts distinguish a lost packet from a value the core refuses to store.
const MAX_ATTEMPTS: u8 = 3;

/// How long a sent packet may wait for its echo before the barrier is lifted and the send retried.
///
/// Without this the attempt budget is unreachable for the one failure it most needs to cover: a
/// packet the core never answers at all parks the queue — and every later OK — for the rest of the
/// connection. The core normally re-broadcasts within a round trip, so seconds of grace are enough
/// to tell "slow" from "gone".
const ECHO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Which areas of [`CoreConfig`] one queued edit actually asked to change, set by the CALLER at
/// enqueue time — the one moment the user's intent is known. Never derived by comparison: a
/// send-time diff against the latest retained snapshot picks up a concurrent core-side change as
/// "touched" and writes it back, the exact opposite of what a mask is for.
///
/// `apply_core_config` writes only the areas named here, so two edits queued before either's echo
/// arrives cannot restore each other's fields, and the gear popup's mask — naming only the five
/// rendered sections — cannot reach the manual block at all, checkbox on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMask {
    /// Moonbot's autobuy page, which reaches into `signals`, its `signal_config` sub-record and one
    /// field of `trading`. One area because it is one PAGE.
    auto_buy: bool,
    auto_start: bool,
    btc_blink: bool,
    general: bool,
    /// The mouse-gesture block of Moonbot's Hotkeys page: `trading.multi_orders` and one field of
    /// `trading` beside it. The rest of that page mirrors the manual block, which no mask reaches.
    gestures: bool,
    /// Moonbot's own interface page, which reaches into `trading`, `visual`, `signals` AND `ui`.
    /// One area rather than four because it is one PAGE: what a surface draws is what it may write.
    interface: bool,
    /// The rows of Moonbot's General page the COMPACT popup does not draw — the one place the
    /// "an area is a page" rule splits a page in two, because the rule it serves is that a surface
    /// writes only what it drew. See `feed::OrderRulesSettings`.
    order_rules: bool,
    leverage: bool,
    /// The `signals` section's two price-approach alerts. The FIRST field the terminal writes
    /// outside `trading`/`visual`; the send carries every section either way, so this narrows only
    /// when those six fields are overwritten, exactly as the four above do for theirs.
    signals: bool,
    /// Moonbot's "Специальные" page: the engine switches, logging and the screenshot block.
    special: bool,
    /// Moonbot's Telegram page: the signal channels, their rules, and the cloud blacklist flag.
    telegram: bool,
    /// `trading.ignore_strat_sell_price`, the one manual-block field the terminal still WRITES.
    ///
    /// It is core behaviour, not a value the terminal can hold locally: it decides whether the core
    /// applies a manual strategy's own sell price or the global TP/S the toolbar edits. Everything
    /// else in the manual block is read-only here — see the module doc.
    ignore_strat_sell_price: bool,
}

impl FieldMask {
    /// No fields touched. Base value for building a narrow mask with the `with_*` methods below.
    pub const EMPTY: Self = Self {
        auto_buy: false,
        auto_start: false,
        btc_blink: false,
        general: false,
        gestures: false,
        interface: false,
        order_rules: false,
        leverage: false,
        signals: false,
        special: false,
        telegram: false,
        ignore_strat_sell_price: false,
    };

    /// The five sections the COMPACT gear popup renders, and nothing else.
    ///
    /// Not "everything the terminal renders" any more: the expert window builds its own mask out of
    /// the pages its user actually edited (`ExpertTab::add_sections` in `moon-ui-gpui`), and one of
    /// them reaches a sixth section this popup does not draw. Each surface names what it drew.
    ///
    /// The manual block is deliberately absent from BOTH: an OK may never change a manual-trading
    /// field, checkbox on or off — see `send_core_config`, the one applier they share.
    pub const RENDERED_SECTIONS: Self = Self {
        // NOT the autobuy page: the compact popup does not draw it.
        auto_buy: false,
        auto_start: true,
        btc_blink: true,
        general: true,
        // NOT the Hotkeys page: the compact popup does not draw it.
        gestures: false,
        // NOT the interface page: the compact popup does not draw it. The expert window names it
        // itself, through `with_interface`.
        interface: false,
        // NOT the seven General-page rows below the popup's own: it draws the exits and the
        // blacklist, and naming these would let its OK stamp them back from a frozen draft.
        order_rules: false,
        leverage: true,
        signals: true,
        // NOT the Special page: the compact popup does not draw it.
        special: false,
        // NOT the Telegram page: the compact popup does not draw it.
        telegram: false,
        ignore_strat_sell_price: false,
    };

    /// Whether this mask names the `general` section.
    ///
    /// For a caller that keeps a second, CLIENT-side copy of one of that section's fields: it must
    /// move only when the section itself does, or the two halves drift apart the first time a
    /// surface sends a mask without `general` in it.
    pub const fn writes_general(self) -> bool {
        self.general
    }

    /// Name the `general` section: the exits, the risk limits and the blacklist.
    pub const fn with_general(mut self) -> Self {
        self.general = true;
        self
    }

    /// Name the `auto_start` section: what the core turns on, its loss caps and its watchdogs.
    pub const fn with_auto_start(mut self) -> Self {
        self.auto_start = true;
        self
    }

    /// Name the `signals` section's alert sounds.
    pub const fn with_signals(mut self) -> Self {
        self.signals = true;
        self
    }

    /// Name the `btc_blink` section: the BTC-rate highlight and its alarm.
    pub const fn with_btc_blink(mut self) -> Self {
        self.btc_blink = true;
        self
    }

    /// Name Moonbot's "Специальные" page — the engine switches, logging and screenshots.
    pub const fn with_special(mut self) -> Self {
        self.special = true;
        self
    }

    /// Name Moonbot's Telegram page — its signal channels and the rules over them.
    pub const fn with_telegram(mut self) -> Self {
        self.telegram = true;
        self
    }

    /// Name Moonbot's autobuy page — its signal sources and its message filter.
    pub const fn with_auto_buy(mut self) -> Self {
        self.auto_buy = true;
        self
    }

    /// Whether two projections agree on every area this mask names.
    ///
    /// The same comparison the echo path uses to decide "confirmed", "rejected" and "already
    /// satisfied" — shared with the SESSION store, which needs it to tell a retry of one edit from
    /// a different edit. Comparing whole projections there had the defect it had here: the areas an
    /// edit never named are free to drift, and a drift is not a different edit.
    ///
    /// Not a predicate about the mask alone, unlike its `writes_*` neighbours: it takes two
    /// configurations and answers about THEM.
    pub(crate) fn agrees_within(self, a: &CoreConfig, b: &CoreConfig) -> bool {
        rejection_within_mask(a, b, self).is_none()
    }

    /// Whether every area `other` names is also named here.
    ///
    /// A send's mask is the UNION of everything queued, so it NARROWS as entries leave — a batch
    /// whose head is confirmed re-sends the rest under a smaller mask. That is still the same work
    /// in flight, which is why the store asks for containment rather than equality.
    pub(crate) fn contains(self, other: Self) -> bool {
        self.union(other) == self
    }

    /// Whether this mask names the `order_rules` area.
    ///
    /// For the same caller [`Self::writes_general`] serves: `deltas_by_trades` also has a
    /// client-side half, and it must move only when its area does.
    pub const fn writes_order_rules(self) -> bool {
        self.order_rules
    }

    /// Name the General page's rows below the compact popup's own.
    pub const fn with_order_rules(mut self) -> Self {
        self.order_rules = true;
        self
    }

    /// Name the mouse-gesture block of Moonbot's Hotkeys page.
    pub const fn with_gestures(mut self) -> Self {
        self.gestures = true;
        self
    }

    /// Name Moonbot's interface page — its own windows, charts and order-book zones.
    pub const fn with_interface(mut self) -> Self {
        self.interface = true;
        self
    }

    /// Name the core's "ignore a manual strategy's own sell price" flag.
    pub fn with_ignore_strat_sell_price(mut self) -> Self {
        self.ignore_strat_sell_price = true;
        self
    }

    fn union(self, other: Self) -> Self {
        Self {
            auto_buy: self.auto_buy || other.auto_buy,
            auto_start: self.auto_start || other.auto_start,
            btc_blink: self.btc_blink || other.btc_blink,
            general: self.general || other.general,
            gestures: self.gestures || other.gestures,
            interface: self.interface || other.interface,
            order_rules: self.order_rules || other.order_rules,
            leverage: self.leverage || other.leverage,
            signals: self.signals || other.signals,
            special: self.special || other.special,
            telegram: self.telegram || other.telegram,
            ignore_strat_sell_price: self.ignore_strat_sell_price || other.ignore_strat_sell_price,
        }
    }
}

/// Per-core serializer for safe-share configuration writes.
pub(in crate::feed) struct SharedConfigSequence {
    queue: VecDeque<QueuedEdit>,
    waiting_for_echo: bool,
    /// When the packet being waited on was sent, for [`ECHO_TIMEOUT`].
    ///
    /// Monotonic on purpose: a wall clock stepping forward would fake a timeout and charge an
    /// attempt against a write still in flight, and stepping back would extend the stall.
    sent_at: Option<Instant>,
    /// Projection expected back from the core, the union of the confirmed entries' masks, and how
    /// many queue entries it confirms.
    pending_confirmation: Option<(CoreConfig, FieldMask, usize)>,
    /// Whether the "queued edits, no base snapshot" stall has been reported. Latched to one line
    /// per stall, not one per feed-loop iteration; cleared once a base is available again.
    missing_snapshot_logged: bool,
    /// Whether the "waiting behind the compact settings channel" stall has been reported, latched
    /// per queued edit for the same reason. Cleared by the next `enqueue`.
    gated_logged: bool,
}

/// One queued write and how many times it has been sent without a matching echo.
struct QueuedEdit {
    /// The complete projection the popup or toolbar wants the core to hold.
    config: CoreConfig,
    /// Which of `config`'s fields this edit actually asked to change; see [`FieldMask`].
    touched: FieldMask,
    attempts: u8,
}

/// Pure next action selected from a retained snapshot.
enum SequenceAction {
    /// Nothing to send right now.
    Idle,
    /// Send this complete config and confirm the listed prefix on echo.
    Send {
        config: Box<SharedConfig>,
        edit_count: usize,
        /// Union of the confirmed entries' masks, carried through to the echo comparison.
        touched: FieldMask,
    },
}

impl SharedConfigSequence {
    /// Create an empty serializer for a newly connected client.
    pub(in crate::feed) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            waiting_for_echo: false,
            sent_at: None,
            pending_confirmation: None,
            missing_snapshot_logged: false,
            gated_logged: false,
        }
    }

    /// Retain queued work but forget connection-local send state before a reconnect attempt.
    ///
    /// The queue survives deliberately: an edit made while the core was dropping out is the user's
    /// intent, and the reconnected core republishes its snapshot, which is exactly the base the
    /// next plan needs.
    pub(in crate::feed) fn prepare_reconnect(&mut self) {
        self.waiting_for_echo = false;
        self.sent_at = None;
        self.pending_confirmation = None;
        // A reconnect starts a new stall episode, worth its own line if that snapshot never lands.
        self.missing_snapshot_logged = false;
        // The attempt budget exists to end a disagreement with a core that refuses a value, not to
        // punish a dropped link: sends lost to a reconnect must not spend it.
        for queued in &mut self.queue {
            queued.attempts = 0;
        }
    }

    /// Queue the popup's or toolbar's complete projection without dropping it when no snapshot has
    /// arrived yet. `touched` names the fields this edit actually asked to change; see
    /// [`FieldMask`].
    pub(super) fn enqueue(&mut self, config: CoreConfig, touched: FieldMask) {
        if touched == FieldMask::EMPTY {
            // Not refused — an edit that names nothing is satisfied by any snapshot, so the queue
            // drops it and reports `Confirmed` without a send, which is the honest answer to "write
            // nothing". Logged because reaching here means a CALLER lost its section list, and that
            // reads as a successful save.
            log::warn!(
                "shared config edit queued with an empty section mask: nothing will be sent"
            );
        }
        self.gated_logged = false;
        self.queue.push_back(QueuedEdit {
            config,
            touched,
            attempts: 0,
        });
    }

    /// Allow the next plan after a `SharedConfigUpdated` echo.
    pub(super) fn observe_update(&mut self) {
        self.waiting_for_echo = false;
    }

    /// Report ONCE that queued safe-share writes cannot go out because the compact settings
    /// channel is still busy.
    ///
    /// The two channels share one order: a compact packet the core never reflects keeps that queue
    /// non-idle for the rest of the connection (its own KNOWN LIMIT), and everything queued here —
    /// a gear-popup OK, the manual-strategy sell-price flag — then waits behind it with no send, no
    /// echo, and no timeout of its own. Silence there looks exactly like a control that does
    /// nothing when clicked, so it gets a line the first time it happens.
    pub(super) fn note_gated(&mut self, server_id: u64) {
        if self.queue.is_empty() || self.gated_logged {
            return;
        }
        self.gated_logged = true;
        log::warn!(
            "core {} has {} shared-config edit(s) waiting: the compact settings channel is not idle",
            crate::feed::core_label(server_id),
            self.queue.len()
        );
    }

    /// Give up waiting for one packet's echo: lift the barrier AND drop the confirmation.
    ///
    /// Dropping it is the point. It describes a packet whose echo never arrived, so leaving it
    /// makes the next plan compare the sent value against the pre-write snapshot — a mismatch
    /// inside the mask by construction — which resolves as `NotApplied` and reports a core that
    /// answered nothing as one that refused. The entry stays queued; the budget still ends it.
    fn observe_echo_timeout(&mut self) {
        self.waiting_for_echo = false;
        self.pending_confirmation = None;
    }

    /// Record a successful send so no later plan can build on the pre-edit snapshot.
    ///
    /// Called ONLY on success: a refused send must leave the barrier down, or the edit waits for an
    /// echo that can never arrive.
    fn observe_send_success(
        &mut self,
        config: &SharedConfig,
        edit_count: usize,
        touched: FieldMask,
    ) {
        self.waiting_for_echo = true;
        self.sent_at = Some(Instant::now());
        self.pending_confirmation = Some((core_config_from_proto(config), touched, edit_count));
        self.charge_attempts(edit_count);
    }

    /// Count one send against the entries it carried.
    fn charge_attempts(&mut self, edit_count: usize) {
        for queued in self.queue.iter_mut().take(edit_count) {
            queued.attempts = queued.attempts.saturating_add(1);
        }
    }

    /// Drop everything queued for a core that is no longer the one that queued it.
    ///
    /// Used when a DIFFERENT MoonBot process answers on the same connection: the pending settings
    /// describe the instance that went away, and applying them to its replacement would write a
    /// page the user never saw for this core.
    pub(in crate::feed) fn forget_queue(&mut self) {
        self.queue.clear();
        self.waiting_for_echo = false;
        self.sent_at = None;
        self.pending_confirmation = None;
        // As in `prepare_reconnect`: a different MoonBot ends the latch's episode.
        self.missing_snapshot_logged = false;
    }

    /// Drive queued edits against the client's retained snapshot.
    ///
    /// Does nothing until the core's first full snapshot arrives: `build_shared_config` refuses to
    /// invent defaults, and sending one would replace a configured core with an empty config.
    ///
    /// `events` collects the edit-lifecycle events this drive produced, in order; the caller sends
    /// them as `FeedMsg::CoreConfigEdit`. This function has no wall clock of its own, so a
    /// `Submitted` row's `submitted_at_ms` is left at `0` for the caller to stamp.
    pub(super) fn drive(
        &mut self,
        client: &MoonClient,
        server_id: u64,
        events: &mut Vec<CoreConfigEditEvent>,
    ) {
        // Both checks precede `build_shared_config`, which CLONES the retained snapshot: the feed
        // loop drives this on every iteration, and a core that stops echoing would otherwise make
        // that clone repeat for the rest of the session.
        if self.queue.is_empty() {
            return;
        }
        if self.waiting_for_echo {
            // A core that answers nothing at all would otherwise hold this queue — and every later
            // OK — for the whole connection, with the attempt budget unreachable behind the barrier.
            if self.sent_at.is_none_or(|at| at.elapsed() < ECHO_TIMEOUT) {
                return;
            }
            log::warn!(
                "core {} shared config echo timed out after {ECHO_TIMEOUT:?}; retrying",
                crate::feed::core_label(server_id)
            );
            self.observe_echo_timeout();
        }
        let config = match client.settings().build_shared_config() {
            Ok(config) => {
                self.missing_snapshot_logged = false;
                config
            }
            // Queued work with no base to write onto is the one failure with no send line, no
            // echo and no give-up — the edit just waits, looking exactly like a core that ignored
            // it. ONCE per stall: `drive` runs on every feed-loop iteration.
            Err(error) => {
                if !self.missing_snapshot_logged {
                    self.missing_snapshot_logged = true;
                    log::warn!(
                        "core {} holds {} queued shared config edit(s) with no base to write onto: \
                         {error}",
                        crate::feed::core_label(server_id),
                        self.queue.len()
                    );
                }
                return;
            }
        };
        match self.next_action(&config, server_id, events) {
            SequenceAction::Idle => {}
            SequenceAction::Send {
                config,
                edit_count,
                touched,
            } => {
                match client.settings().send_shared_config(&config) {
                    Ok(()) => {
                        let wanted = core_config_from_proto(&config);
                        self.observe_send_success(&config, edit_count, touched);
                        events.push(CoreConfigEditEvent::Submitted(Box::new(
                            CoreConfigEditRow {
                                phase: CoreConfigEditPhase::Pending,
                                submitted_at_ms: 0,
                                config: wanted,
                                touched,
                                mismatches: None,
                            },
                        )));
                        log::info!(
                            "core {} shared config sent ({edit_count} edits, {touched:?})",
                            crate::feed::core_label(server_id)
                        );
                    }
                    Err(error) => {
                        // Charged like a real send: the plan is rebuilt on every feed-loop wake, so
                        // a permanently refused send would otherwise clone and re-serialize the
                        // whole configuration forever.
                        self.charge_attempts(edit_count);
                        log::warn!(
                            "core {} shared config send failed: {error}",
                            crate::feed::core_label(server_id)
                        );
                    }
                }
            }
        }
    }

    /// Select the next action and discard edits the core already reflects.
    fn next_action(
        &mut self,
        config: &SharedConfig,
        server_id: u64,
        events: &mut Vec<CoreConfigEditEvent>,
    ) -> SequenceAction {
        if self.waiting_for_echo {
            return SequenceAction::Idle;
        }
        if let Some((expected, touched, edit_count)) = self.pending_confirmation.take() {
            let actual = core_config_from_proto(config);
            // Scoped to `touched`, not the whole projection. Anything this write did not name is
            // free to have moved between the send and the echo — a trader in Moonbot's own dialogs
            // — and comparing it would leave a landed edit unconfirmed, re-sending the whole
            // snapshot until the budget ran out and the edit was dropped as `GaveUp`.
            match rejection_within_mask(&expected, &actual, touched) {
                None => {
                    for _ in 0..edit_count {
                        self.queue.pop_front();
                    }
                    events.push(CoreConfigEditEvent::Resolved(
                        CoreConfigEditResult::Confirmed,
                    ));
                }
                Some(rejection) => {
                    // Logged, not only evented: a core that keeps its own value leaves no other
                    // trace until the budget runs out. NOT phrased as a refusal — `observe_update`
                    // lifts the barrier on ANY `SharedConfigUpdated`, so a first mismatch can be a
                    // pre-write snapshot. The give-up line is where a refusal becomes a verdict.
                    log::warn!(
                        "core {} shared config echo did not carry the requested value (retrying): \
                         {rejection:?}",
                        crate::feed::core_label(server_id)
                    );
                    // Not dequeued: the entries stay queued and MAX_ATTEMPTS below still ends it.
                    events.push(CoreConfigEditEvent::Resolved(
                        CoreConfigEditResult::NotApplied(rejection),
                    ));
                }
            }
        }
        loop {
            let Some(head) = self.queue.front() else {
                return SequenceAction::Idle;
            };
            if edit_satisfied(config, &head.config, head.touched) {
                // The quietest of the three ways an edit leaves the queue: no send line precedes
                // it, so "the core already holds this" and "it was never sent" read alike.
                log::info!(
                    "core {} shared config edit satisfied by the core's own snapshot ({:?})",
                    crate::feed::core_label(server_id),
                    head.touched
                );
                self.queue.pop_front();
                // CONFIRMED, because it is. It also catches the case `observe_echo_timeout` opens:
                // a late echo reaches the queue HERE, and `CoreData::core_config_edit` clears on
                // nothing else, so a succeeded write would leave the cell pending for the session.
                //
                // Suppressed after EITHER terminal verdict in the same pass. There is one row per
                // core (`CoreData::core_config_edit`), `Confirmed` sets it to `None`, and both a
                // rejection and a give-up live IN that row — so another entry's success would erase
                // the news the user most needs. The give-up was excluded here until the comparison
                // above was narrowed to the mask, which turned "this entry is already satisfied"
                // from a near-unreachable case into a common one.
                //
                // The cost is real and chosen: this entry's own success then goes unannounced, and
                // the row keeps the other's verdict until the next edit is submitted over it. With
                // ONE row per core those are the only two options, and a failure a trader never
                // sees is the worse of them — they would read a write that did not land as saved.
                if !events.iter().any(|event| {
                    matches!(
                        event,
                        CoreConfigEditEvent::Resolved(
                            CoreConfigEditResult::NotApplied(_) | CoreConfigEditResult::GaveUp
                        )
                    )
                }) {
                    events.push(CoreConfigEditEvent::Resolved(
                        CoreConfigEditResult::Confirmed,
                    ));
                }
                continue;
            }
            if head.attempts >= MAX_ATTEMPTS {
                // Before the mask this could only dump the whole packet, because nothing knew
                // which of ~530 fields the core still disagreed on. Now the mask names exactly
                // what THIS edit touched, so the log names that instead.
                log::error!(
                    "core {} shared config write gave up after {MAX_ATTEMPTS} sends: the core's \
                     configuration still differs on {:?}, so it is NOT applied",
                    crate::feed::core_label(server_id),
                    head.touched
                );
                events.push(CoreConfigEditEvent::Resolved(CoreConfigEditResult::GaveUp));
                self.queue.pop_front();
                continue;
            }
            let mut next = config.clone();
            let edit_count = self.queue.len();
            let mut touched = FieldMask::EMPTY;
            for queued in &self.queue {
                apply_core_config(&mut next, &queued.config, queued.touched);
                touched = touched.union(queued.touched);
            }
            return SequenceAction::Send {
                config: Box::new(next),
                edit_count,
                touched,
            };
        }
    }
}

/// Whether the core's snapshot already carries everything this write would set IN THE AREAS IT
/// NAMES.
///
/// Restricted to the areas `touched` names, like the confirmation that shares its comparison: the
/// rest of `wanted` is the surface's own copy, frozen when it seeded, and a field that drifted
/// there says nothing about whether THIS edit still has work to do. Comparing it made an OK that
/// changed nothing send the whole snapshot whenever anything else on the core had moved since.
fn edit_satisfied(config: &SharedConfig, wanted: &CoreConfig, touched: FieldMask) -> bool {
    touched.agrees_within(wanted, &core_config_from_proto(config))
}

/// What the echo disagreed with the terminal about, restricted to the fields `touched` actually
/// names — never the whole projection, so a concurrent core-side change to an untouched field
/// cannot read as this edit's rejection. `None` means every touched field matches, which is also
/// what CONFIRMS a write and what tells the queue an edit needs no send at all: the three questions
/// are one comparison, and they were not always asked the same way.
fn rejection_within_mask(
    expected: &CoreConfig,
    actual: &CoreConfig,
    touched: FieldMask,
) -> Option<CoreConfigRejection> {
    // Destructured rather than read through `touched.`: a bit added to the mask must then be
    // NAMED here or the pattern does not compile (E0027). That is the half worth having — leaving
    // a named bit unused is only a warning, so the pattern makes the omission impossible to miss
    // rather than impossible to make. It matters more than it reads: this function is now also
    // what decides an edit is already satisfied, so a bit with no arm would make every edit naming
    // it drop without ever being sent.
    let FieldMask {
        auto_buy,
        auto_start,
        btc_blink,
        general,
        gestures,
        interface,
        order_rules,
        leverage,
        signals,
        special,
        telegram,
        ignore_strat_sell_price,
    } = touched;
    let mut areas = Vec::new();
    if auto_buy && expected.auto_buy != actual.auto_buy {
        areas.push(CoreConfigArea::AutoBuy);
    }
    if auto_start && expected.auto_start != actual.auto_start {
        areas.push(CoreConfigArea::AutoStart);
    }
    if btc_blink && expected.btc_blink != actual.btc_blink {
        areas.push(CoreConfigArea::BtcBlink);
    }
    if general && expected.general != actual.general {
        areas.push(CoreConfigArea::General);
    }
    if gestures && expected.gestures != actual.gestures {
        areas.push(CoreConfigArea::Gestures);
    }
    if interface && expected.interface != actual.interface {
        areas.push(CoreConfigArea::Interface);
    }
    if order_rules && expected.order_rules != actual.order_rules {
        areas.push(CoreConfigArea::OrderRules);
    }
    if leverage && expected.leverage != actual.leverage {
        areas.push(CoreConfigArea::Leverage);
    }
    if signals && expected.signals != actual.signals {
        areas.push(CoreConfigArea::Signals);
    }
    if special && expected.special != actual.special {
        areas.push(CoreConfigArea::Special);
    }
    if telegram && expected.telegram != actual.telegram {
        areas.push(CoreConfigArea::Telegram);
    }
    if ignore_strat_sell_price
        && expected.manual.ignore_strat_sell_price != actual.manual.ignore_strat_sell_price
    {
        areas.push(CoreConfigArea::Manual);
    }
    (!areas.is_empty()).then_some(CoreConfigRejection::Areas(areas))
}

/// Project the settings the terminal renders out of a full safe-share snapshot.
///
/// `pub(crate)` rather than `pub(super)` for one reason: [`CoreConfig`] deliberately has no
/// `Default`, so this is the only way to build one, and the store's own tests need a projection to
/// drive its edit-row rules with. It stays crate-internal — the UI receives projections, it does
/// not make them.
pub(crate) fn core_config_from_proto(cfg: &SharedConfig) -> CoreConfig {
    let a = &cfg.trading.auto_start;
    let a2 = &cfg.trading.auto_start_2;
    let b = &cfg.visual.blink_config;
    let t = &cfg.trading;
    let m = &cfg.trading.auto_manage_lev;
    let sig = &cfg.signals;
    let hotkeys = &cfg.ui.hotkeys_config;
    let strat_buttons = &cfg.trading.manual_strats_config;
    let v = &cfg.visual;
    let u = &cfg.ui;
    let sc = &cfg.signals.signal_config;
    let shots = &cfg.trading.send_shots_config;
    let oc = &cfg.trading.orders_control;
    let mo = &cfg.trading.multi_orders;
    CoreConfig {
        special: SpecialSettings {
            log_level: t.log_level,
            auto_delete_logs: t.auto_delete_logs,
            chart_clean_up_time: t.chart_clean_up_time,
            max_orders: t.max_orders,
            unlimited_orders: t.unlimited_orders,
            random_price: t.random_price,
            correct_order_price: t.correct_order_price,
            use_book_ticker: t.use_book_ticker,
            m_avg_use_vol_weight: t.m_avg_use_vol_weight,
            auto_buy_bnb: t.auto_buy_bnb,
            auto_buy_bnb_level: t.auto_buy_bnb_level,
            auto_buy_bnb_volume: t.auto_buy_bnb_volume,
            auto_reduce_order: t.auto_reduce_order,
            auto_close_zero_pos: t.auto_close_zero_pos,
            auto_lower_lev: t.auto_lower_lev,
            use_websocket_api: t.use_websocket_api,
            iceberg_step: t.iceberg_step,
            sell_x2_level: t.sell_x2_level,
            no_trades_markets_text: t.no_trades_markets_text.clone(),
            multi_commands: t.multi_commands,
            h_pos_black_list_text: t.h_pos_black_list_text.clone(),
            liq_control: oc.liq_control,
            ignore_replacing_bug: oc.ignore_replacing_bug,
            ignore_protection: oc.ignore_protection,
            orders_control_active: oc.active,
            h_pos_report: oc.h_pos_report,
            h_pos_auto_sell: oc.h_pos_auto_sell,
            send_shots: shots.may_send,
            profit_abs: shots.profit_abs,
            profit_pers: shots.profit_pers,
            profit_session: shots.profit_session,
            send_negative: shots.send_negative,
            send_public: shots.send_public,
            time_scale: shots.time_scale,
            price_scale: shots.price_scale,
        },
        telegram: TelegramSettings {
            pump_channel: sig.pump_channel.clone(),
            pump_channels: sig.pump_channels.clone(),
            multi_channels: sig.multi_channels,
            more_then_1_channel: sig.more_then_1_channel,
            listen_moon_channel: sig.listen_moon_channel,
            use_moon_bl: t.use_moon_bl,
        },
        auto_buy: AutoBuySettings {
            monitor_clipboard: sig.monitor_clipboard,
            clipboard_auto_buy: sig.clipboard_auto_buy,
            lower_case_token_cbd: sig.lower_case_token_cbd,
            look_full_link_cbd: sig.look_full_link_cbd,
            advanced_filter_clipboard: sig.advanced_filter_clipboard,
            telegram_auto_buy: sig.telegram_auto_buy,
            lower_case_token_tlg: sig.lower_case_token_tlg,
            look_full_link_tlg: sig.look_full_link_tlg,
            advanced_filter: sig.advanced_filter,
            dont_buy_reply: sig.dont_buy_reply,
            msg_keywords_long: sig.msg_keywords_long.clone(),
            msg_keywords_short: sig.msg_keywords_short.clone(),
            msg_black_words: sig.msg_black_words.clone(),
            msg_token_tags: sig.msg_token_tags.clone(),
            lower_price_words: sig.lower_price_words.clone(),
            use_keywords: sc.use_keywords,
            buy_key_dist: sc.buy_key_dist,
            use_black_words: sc.use_black_words,
            use_words_count: sc.use_words_count,
            words_count: sc.words_count,
            use_lower_price_words: sc.use_lower_price_words,
            x_lower_price: sc.x_lower_price,
            x_found_price: sc.x_found_price,
            buy_if_price_found: sc.buy_if_price_found,
            use_price: sc.use_price,
            use_stops: sc.use_stops,
            only_1_token: sc.only_1_token,
            use_token_tags: sc.use_token_tags,
            tokens_no_tags: sc.tokens_no_tags,
            token_links: sc.token_links,
            special_formats: sc.special_formats,
            auto_cancel_lower_buy: t.auto_cancel_lower_buy,
        },
        interface: InterfaceSettings {
            buy_on_enter: t.buy_on_enter,
            dbl_click_panic_sell: t.dbl_click_panic_sell,
            chart_split_zones: t.chart_split_zones,
            draw_stop: t.draw_stop,
            pending_orders_spread: t.pending_orders_spread,
            pending_orders_spread_h_delta: t.pending_orders_spread_h_delta,
            hide_forum_label: v.hide_forum_label,
            scrolling_charts: v.scrolling_charts,
            startup_load_charts: v.startup_load_charts,
            hide_right_chart_panel: v.hide_right_chart_panel,
            left_chart_info: v.left_chart_info,
            show_iceberg: v.show_iceberg,
            show_orders_captions: v.show_orders_captions,
            orders_captions_lower: v.orders_captions_lower,
            hide_pnl: v.hide_pnl,
            hide_buy_button: v.hide_buy_button,
            hide_cashback_button: v.hide_cashback_button,
            remember_chart_buttons: v.remember_chart_buttons,
            scale_tool: v.show_filters.scale_tool,
            icon_selection: v.icon_selection,
            price_line_width: v.colors.price_line_width,
            panic_sell_opacity: v.panic_sell_opacity,
            book_cumulative_opacity: v.book_cumulative_opacity,
            book_orders_opacity: v.book_orders_opacity,
            book_orders_width: v.book_orders_width,
            play_signal_sound: cfg.signals.play_signal_sound,
            confirm_close: u.confirm_close,
            hide_demo_button: u.hide_demo_button,
        },
        signals: SignalsSettings {
            play_sell_alert: sig.play_sell_alert,
            sell_alert_level: sig.sell_alert_level,
            signal_sound_2: sig.signal_sound_2,
            play_buy_alert: sig.play_buy_alert,
            buy_alert_level: sig.buy_alert_level,
            buy_signal_sound: sig.buy_signal_sound,
        },
        auto_start: AutoStartSettings {
            auto_start: a.auto_start,
            auto_detect_on: a.auto_detect_on,
            strategies_on: a.strategies_on,
            remember_state: a.remember_state,
            auto_update: a.auto_update,
            dont_wait_sells: a.dont_wait_sells,
            work_time: a.work_time,
            work_time_from_min: day_fraction_to_minutes(a.work_time_from),
            work_time_to_min: day_fraction_to_minutes(a.work_time_to),
            auto_stop_if_loss: a.auto_stop_if_loss,
            auto_stop_loss: a.auto_stop_loss,
            stop_trades: a.stop_trades,
            sell_if_loss: a.sell_if_loss,
            auto_stop_if_loss_hours: a.auto_stop_if_loss_hours,
            auto_stop_hours_val: a.auto_stop_hours_val,
            stop_hours: a.stop_hours,
            stop_hours_trades: a.stop_hours_trades,
            ignore_emulator: a.ignore_emulator,
            reset_session: a2.reset_session,
            rs_hours: a2.rs_hours,
            max_session_cap: a2.max_session_cap,
            panic_btc: a.panic_btc,
            panic_btc_delta: a.panic_btc_delta,
            panic_btc_delta_up: a.panic_btc_delta_up,
            panic_market: a.panic_market,
            panic_market_delta: a.panic_market_delta,
            restart_on_market: a2.restart_on_market,
            btc_higher_then: a2.btc_higher_then,
            btc_lower_then: a2.btc_lower_then,
            market_higher_then: a2.market_higher_then,
            auto_stop_on_errors: a.auto_stop_on_errors,
            errors_level: a.errors_level,
            sell_all_on_errors: a.sell_all_on_errors,
            restart_after_err: a.restart_after_err,
            restart_err_time: a.restart_err_time,
            auto_stop_on_ping: a.auto_stop_on_ping,
            ping_level: a.ping_level,
            sell_all_on_ping: a.sell_all_on_ping,
            restart_after_ping: a.restart_after_ping,
            restart_ping_time: a.restart_ping_time,
        },
        btc_blink: BtcBlinkSettings {
            blink_btc: b.blink_btc,
            blink_btc_delta: b.blink_btc_delta,
            blink_btc_delta_up: b.blink_btc_delta_up,
            alarm_btc: b.alarm_btc,
            alarm_type: b.alarm_type,
        },
        general: GeneralSettings {
            take_profit_on: t.use_g_take_profit,
            take_profit_pct: t.g_take_profit,
            trailing_on: t.trailing_stop,
            trailing_pct: t.trailing_drop,
            vstop_on: t.panic_if_vol_drop,
            vol_drop_level: t.vol_drop_level,
            buy_iceberg: t.buy_iceberg,
            sell_iceberg: t.sell_iceberg,
            blacklist_on: t.use_coins_black_list,
            blacklist_text: t.coins_black_list_text.clone(),
            exclude_blacklisted_from_deltas: t.exclude_black_list_delta,
        },
        order_rules: OrderRulesSettings {
            trailing_float: t.trailing_float,
            auto_sell_partial: t.auto_sell_partial,
            auto_cancel_buy_order: t.auto_cancel_buy_order,
            cancel_buy_on_sell_fill: t.cancel_buy_on_sell_fill,
            dont_buy_new_coins: t.dont_buy_new_coins,
            deltas_by_trades: t.deltas_by_trades,
            analyze_on_start: sig.load_deep_history,
        },
        gestures: GestureSettings {
            buy_set_click: mo.buy_set_click,
            short_set_click: mo.short_set_click,
            pending_order_set_click: t.pending_order_set_click,
            pending_short_set_click: mo.pending_short_set_click,
            same_hotkeys_for_move: mo.same_hotkeys_for_move,
            buy_move_click: mo.buy_move_click,
            short_buy_move_click: mo.short_buy_move_click,
            replace_buy_kind: mo.replace_buy_kind,
            sell_move_click: mo.sell_move_click,
            short_sell_move_click: mo.short_sell_move_click,
            replace_sell_kind: mo.replace_sell_kind,
            buy_move_click_2: mo.buy_move_click_2,
            short_buy_move_click_2: mo.short_buy_move_click_2,
            replace_buy_kind_2: mo.replace_buy_kind_2,
            sell_move_click_2: mo.sell_move_click_2,
            short_sell_move_click_2: mo.short_sell_move_click_2,
            replace_sell_kind_2: mo.replace_sell_kind_2,
        },
        leverage: LeverageSettings {
            auto_max_order: m.auto_max_order,
            auto_lev_up: m.auto_lev_up,
            auto_isolated: m.auto_isolated,
            auto_cross: m.auto_cross,
            tlg_report: m.tlg_report,
            auto_fix_lev: m.auto_fix_lev,
            fix_lev: m.fix_lev,
            lev_control: t.auto_lev_control.clone(),
        },
        manual: ManualSettings {
            order_sizes: hotkeys.o_size,
            // `saturating_sub`, not `-`: `b_num` is untrusted wire `i32`, and this repo runs with
            // `debug-assertions = false` even in dev (root `Cargo.toml`), so a corrupt `i32::MIN`
            // would otherwise wrap silently to `i32::MAX` and clamp to slot 5 — the LARGEST
            // preset — instead of being caught, on a money-adjacent field.
            order_size_sel: hotkeys.b_num.saturating_sub(1).clamp(0, 5) as usize,
            strat_names: t.manual_strats_names.clone(),
            strat_buttons: CoreStratButtons {
                use_buttons: strat_buttons.use_buttons,
                show_button: strat_buttons.show_button,
                hot_keys: strat_buttons.hot_keys,
            },
            core_hotkeys: CoreHotkeyLayout {
                order_size: hotkeys.o_keys,
                sell_preset: hotkeys.s_keys,
                named: [
                    (CoreHotkeyAction::CancelBuy, hotkeys.cancel_buy),
                    (CoreHotkeyAction::PanicSell, hotkeys.panic_sell),
                    (CoreHotkeyAction::JoinSells, hotkeys.join_sells),
                    (CoreHotkeyAction::SwitchCharts, hotkeys.switch_charts),
                    (CoreHotkeyAction::ReloadBook, hotkeys.reload_book),
                    (CoreHotkeyAction::NewLong, hotkeys.new_long),
                    (CoreHotkeyAction::NewShort, hotkeys.new_short),
                    (CoreHotkeyAction::SplitOrder, hotkeys.split_order),
                    (CoreHotkeyAction::ShiftBuyUp, hotkeys.shift_buy_up),
                    (CoreHotkeyAction::ShiftBuyDown, hotkeys.shift_buy_down),
                    (CoreHotkeyAction::ShiftSellUp, hotkeys.shift_sell_up),
                    (CoreHotkeyAction::ShiftSellDown, hotkeys.shift_sell_down),
                    (CoreHotkeyAction::MakeShot, hotkeys.make_shot),
                    (CoreHotkeyAction::MakeShotBot, hotkeys.make_shot_bot),
                    (CoreHotkeyAction::ReloadChart, hotkeys.reload_chart),
                    (CoreHotkeyAction::ScalePlus, hotkeys.scale_plus),
                    (CoreHotkeyAction::ScaleMinus, hotkeys.scale_minus),
                    (CoreHotkeyAction::SellPlus, hotkeys.sell_plus),
                    (CoreHotkeyAction::SellMinus, hotkeys.sell_minus),
                    (CoreHotkeyAction::SpyMode, hotkeys.spy_mode),
                    (CoreHotkeyAction::ShowCharts, hotkeys.show_charts),
                    (CoreHotkeyAction::SplitOrderX, hotkeys.split_order_x),
                    (CoreHotkeyAction::SwitchFigure, hotkeys.switch_figure),
                    (CoreHotkeyAction::FitSells, hotkeys.fit_sells),
                    (CoreHotkeyAction::PanicSellOne, hotkeys.panic_sell_one),
                    (CoreHotkeyAction::CancelAllBuys, hotkeys.cancel_all_buys),
                    (CoreHotkeyAction::Broadcast, hotkeys.broadcast),
                ],
            },
            ignore_strat_sell_price: t.ignore_strat_sell_price,
            use_lev_for_take: t.use_lev_for_take,
        },
    }
}

/// Write only the areas `touched` names into a full safe-share snapshot.
///
/// Every other section — including each section's `unknown_tail`, which carries settings written
/// by a newer core than this build knows — travels back untouched, and so does every area this
/// call was not told to touch: two edits queued before either's echo arrives can no longer restore
/// each other's fields, and a mask that never names the manual block cannot reach it at all. A
/// field added to [`CoreConfig`] is still picked up by both the read and the write, so a tab can
/// never send a value the projection cannot show, nor show one it cannot send — the mask narrows
/// WHEN a named field is written, never WHETHER an unnamed one could be.
pub(super) fn apply_core_config(cfg: &mut SharedConfig, wanted: &CoreConfig, touched: FieldMask) {
    // Destructured for the reason `rejection_within_mask` is, and it matters MORE here: a bit with
    // a comparison arm but no applier arm would send every edit naming it, never apply it, and burn
    // the retry budget into a `GaveUp` — the two functions have to grow together.
    let FieldMask {
        auto_buy,
        auto_start,
        btc_blink,
        general,
        gestures,
        interface,
        order_rules,
        leverage,
        signals,
        special,
        telegram,
        ignore_strat_sell_price,
    } = touched;
    if auto_buy {
        apply_auto_buy(cfg, &wanted.auto_buy);
    }
    if auto_start {
        apply_auto_start(cfg, &wanted.auto_start);
    }
    if btc_blink {
        apply_btc_blink(cfg, &wanted.btc_blink);
    }
    if general {
        apply_general(cfg, &wanted.general);
    }
    if gestures {
        apply_gestures(cfg, &wanted.gestures);
    }
    if interface {
        apply_interface(cfg, &wanted.interface);
    }
    if order_rules {
        apply_order_rules(cfg, &wanted.order_rules);
    }
    if leverage {
        apply_leverage(cfg, &wanted.leverage);
    }
    if signals {
        apply_signals(cfg, &wanted.signals);
    }
    if special {
        apply_special(cfg, &wanted.special);
    }
    if telegram {
        apply_telegram(cfg, &wanted.telegram);
    }
    if ignore_strat_sell_price {
        cfg.trading.ignore_strat_sell_price = wanted.manual.ignore_strat_sell_price;
    }
}

/// Apply the two price-approach alerts to the `signals` section.
///
/// Six fields of a section with about a hundred: everything else in it — including its
/// `unknown_tail` — travels back untouched, exactly as `apply_general` leaves the rest of
/// `trading` alone. The connectivity alert that neighbours them on the wire is NOT here: it belongs
/// to [`apply_interface`], the page that draws it.
fn apply_signals(cfg: &mut SharedConfig, s: &SignalsSettings) {
    let sig = &mut cfg.signals;
    sig.play_sell_alert = s.play_sell_alert;
    sig.sell_alert_level = s.sell_alert_level;
    sig.signal_sound_2 = s.signal_sound_2;
    sig.play_buy_alert = s.play_buy_alert;
    sig.buy_alert_level = s.buy_alert_level;
    sig.buy_signal_sound = s.buy_signal_sound;
}

/// Apply the Hotkeys page's mouse-gesture block to `trading.multi_orders`.
///
/// Sixteen of the record's twenty-four fields, plus `trading.pending_order_set_click` beside it.
/// The eight left out are: `join_sell_kind`, which the wire marks a mirror of
/// `ClientSettingsCommand::join_sell_kind`, so it travels on the compact channel too and writing it
/// from here would set two routes fighting over one field; `use_multi_orders`, `split_sells`,
/// `show_orders_num`, `kir_style`, `fix_pos` and `done_opacity`, which are in the record but not on
/// this PAGE — Moonbot draws them with its chart, not with its gestures; and `ver`, which is the
/// wire's own version byte, not a setting.
fn apply_gestures(cfg: &mut SharedConfig, g: &GestureSettings) {
    cfg.trading.pending_order_set_click = g.pending_order_set_click;
    let mo = &mut cfg.trading.multi_orders;
    mo.buy_set_click = g.buy_set_click;
    mo.short_set_click = g.short_set_click;
    mo.pending_short_set_click = g.pending_short_set_click;
    mo.same_hotkeys_for_move = g.same_hotkeys_for_move;
    mo.buy_move_click = g.buy_move_click;
    mo.short_buy_move_click = g.short_buy_move_click;
    mo.replace_buy_kind = g.replace_buy_kind;
    mo.sell_move_click = g.sell_move_click;
    mo.short_sell_move_click = g.short_sell_move_click;
    mo.replace_sell_kind = g.replace_sell_kind;
    mo.buy_move_click_2 = g.buy_move_click_2;
    mo.short_buy_move_click_2 = g.short_buy_move_click_2;
    mo.replace_buy_kind_2 = g.replace_buy_kind_2;
    mo.sell_move_click_2 = g.sell_move_click_2;
    mo.short_sell_move_click_2 = g.short_sell_move_click_2;
    mo.replace_sell_kind_2 = g.replace_sell_kind_2;
}

/// Apply the rest of Moonbot's General page — the rows the compact popup does not draw.
///
/// Six fields of `trading` plus `signals.load_deep_history`, which is the one field of this area
/// outside `trading` and cannot collide with [`apply_signals`]: that applier owns six other fields
/// of the same section and none of them is this one.
fn apply_order_rules(cfg: &mut SharedConfig, r: &OrderRulesSettings) {
    cfg.signals.load_deep_history = r.analyze_on_start;
    let t = &mut cfg.trading;
    t.trailing_float = r.trailing_float;
    t.auto_sell_partial = r.auto_sell_partial;
    t.auto_cancel_buy_order = r.auto_cancel_buy_order;
    t.cancel_buy_on_sell_fill = r.cancel_buy_on_sell_fill;
    t.dont_buy_new_coins = r.dont_buy_new_coins;
    t.deltas_by_trades = r.deltas_by_trades;
}

/// Apply the General tab to the exit rules, iceberg flags and blacklist in `trading`.
fn apply_general(cfg: &mut SharedConfig, g: &GeneralSettings) {
    let t = &mut cfg.trading;
    t.use_g_take_profit = g.take_profit_on;
    t.g_take_profit = g.take_profit_pct;
    t.trailing_stop = g.trailing_on;
    t.trailing_drop = g.trailing_pct;
    t.panic_if_vol_drop = g.vstop_on;
    t.vol_drop_level = g.vol_drop_level;
    t.buy_iceberg = g.buy_iceberg;
    t.sell_iceberg = g.sell_iceberg;
    t.use_coins_black_list = g.blacklist_on;
    t.coins_black_list_text = g.blacklist_text.clone();
    t.exclude_black_list_delta = g.exclude_blacklisted_from_deltas;
}

/// Apply Moonbot's "Специальные" page to `trading` and its `send_shots_config` and
/// `orders_control` sub-records.
///
/// Thirty-five fields across `trading`, its `send_shots_config` and its `orders_control`:
/// everything else in that section — including its `unknown_tail`, the exits [`apply_general`] owns
/// and the leverage block [`apply_leverage`] owns — travels back untouched.
///
/// `orders_control.sign_orders` is NOT among them on purpose: the wire's own doc marks it a mirror
/// of `ClientSettingsCommand::sign_orders`, so it travels on the compact channel too, and writing it
/// from here would set two routes fighting over one field. Its neighbours `min_price`, `max_time`
/// and `h_pos_control` are absent for the plainer reason that no row of this page draws them — see
/// `core_expert::pages::special`, which states what that costs.
fn apply_special(cfg: &mut SharedConfig, s: &SpecialSettings) {
    let t = &mut cfg.trading;
    t.log_level = s.log_level;
    t.auto_delete_logs = s.auto_delete_logs;
    t.chart_clean_up_time = s.chart_clean_up_time;
    t.max_orders = s.max_orders;
    t.unlimited_orders = s.unlimited_orders;
    t.random_price = s.random_price;
    t.correct_order_price = s.correct_order_price;
    t.use_book_ticker = s.use_book_ticker;
    t.m_avg_use_vol_weight = s.m_avg_use_vol_weight;
    t.auto_buy_bnb = s.auto_buy_bnb;
    t.auto_buy_bnb_level = s.auto_buy_bnb_level;
    t.auto_buy_bnb_volume = s.auto_buy_bnb_volume;
    t.auto_reduce_order = s.auto_reduce_order;
    t.auto_close_zero_pos = s.auto_close_zero_pos;
    t.auto_lower_lev = s.auto_lower_lev;
    t.use_websocket_api = s.use_websocket_api;
    t.iceberg_step = s.iceberg_step;
    t.sell_x2_level = s.sell_x2_level;
    t.no_trades_markets_text = s.no_trades_markets_text.clone();
    t.multi_commands = s.multi_commands;
    t.h_pos_black_list_text = s.h_pos_black_list_text.clone();
    let oc = &mut t.orders_control;
    oc.liq_control = s.liq_control;
    oc.ignore_replacing_bug = s.ignore_replacing_bug;
    oc.ignore_protection = s.ignore_protection;
    oc.active = s.orders_control_active;
    oc.h_pos_report = s.h_pos_report;
    oc.h_pos_auto_sell = s.h_pos_auto_sell;
    let shots = &mut t.send_shots_config;
    shots.may_send = s.send_shots;
    shots.profit_abs = s.profit_abs;
    shots.profit_pers = s.profit_pers;
    shots.profit_session = s.profit_session;
    shots.send_negative = s.send_negative;
    shots.send_public = s.send_public;
    shots.time_scale = s.time_scale;
    shots.price_scale = s.price_scale;
}

/// Apply Moonbot's Telegram page to `signals` and the one `trading` flag beside it.
///
/// Six fields: everything else in both sections — including their `unknown_tail`s, the alert sounds
/// [`apply_signals`] owns and the message filter [`apply_auto_buy`] owns — travels back untouched.
fn apply_telegram(cfg: &mut SharedConfig, t: &TelegramSettings) {
    let sig = &mut cfg.signals;
    sig.pump_channel = t.pump_channel.clone();
    sig.pump_channels = t.pump_channels.clone();
    sig.multi_channels = t.multi_channels;
    sig.more_then_1_channel = t.more_then_1_channel;
    sig.listen_moon_channel = t.listen_moon_channel;
    cfg.trading.use_moon_bl = t.use_moon_bl;
}

/// Apply Moonbot's autobuy page to `signals`, its `signal_config` sub-record and one `trading`
/// field.
///
/// Thirty-two fields: everything else in each section — including their `unknown_tail`s and the two
/// price-approach alerts [`apply_signals`] owns — travels back untouched.
fn apply_auto_buy(cfg: &mut SharedConfig, b: &AutoBuySettings) {
    let sig = &mut cfg.signals;
    sig.monitor_clipboard = b.monitor_clipboard;
    sig.clipboard_auto_buy = b.clipboard_auto_buy;
    sig.lower_case_token_cbd = b.lower_case_token_cbd;
    sig.look_full_link_cbd = b.look_full_link_cbd;
    sig.advanced_filter_clipboard = b.advanced_filter_clipboard;
    sig.telegram_auto_buy = b.telegram_auto_buy;
    sig.lower_case_token_tlg = b.lower_case_token_tlg;
    sig.look_full_link_tlg = b.look_full_link_tlg;
    sig.advanced_filter = b.advanced_filter;
    sig.dont_buy_reply = b.dont_buy_reply;
    sig.msg_keywords_long = b.msg_keywords_long.clone();
    sig.msg_keywords_short = b.msg_keywords_short.clone();
    sig.msg_black_words = b.msg_black_words.clone();
    sig.msg_token_tags = b.msg_token_tags.clone();
    sig.lower_price_words = b.lower_price_words.clone();
    let c = &mut sig.signal_config;
    c.use_keywords = b.use_keywords;
    c.buy_key_dist = b.buy_key_dist;
    c.use_black_words = b.use_black_words;
    c.use_words_count = b.use_words_count;
    c.words_count = b.words_count;
    c.use_lower_price_words = b.use_lower_price_words;
    c.x_lower_price = b.x_lower_price;
    c.x_found_price = b.x_found_price;
    c.buy_if_price_found = b.buy_if_price_found;
    c.use_price = b.use_price;
    c.use_stops = b.use_stops;
    c.only_1_token = b.only_1_token;
    c.use_token_tags = b.use_token_tags;
    c.tokens_no_tags = b.tokens_no_tags;
    c.token_links = b.token_links;
    c.special_formats = b.special_formats;
    cfg.trading.auto_cancel_lower_buy = b.auto_cancel_lower_buy;
}

/// Apply Moonbot's interface page across the four sections it lives in.
///
/// Twenty-eight fields of the several hundred those sections hold: everything else in each of them
/// — including all four `unknown_tail`s — travels back untouched, exactly as `apply_general` leaves
/// the rest of `trading` alone.
///
/// Three of Moonbot's rows on that page are deliberately NOT here, and the page draws them
/// disabled. `trading.pending_buy_price` is not the drawing flag its caption suggests: the wire
/// documents it as using the pending-buy price instead of the current ask for SELL calculations,
/// which is trading maths, not appearance. `trading.use_lev_for_take` already belongs to
/// [`crate::feed::ManualSettings`], and projecting one wire field into two areas would leave the
/// second stale after a write and make `edit_satisfied` false for any mask naming that area.
/// `visual`'s
/// `manual_charts_full_screen` sits behind that section's tail gate, so a core older than the field
/// reads it back as `false` however it was written — an edit that could never echo, and would burn
/// all three attempts.
fn apply_interface(cfg: &mut SharedConfig, i: &InterfaceSettings) {
    let t = &mut cfg.trading;
    t.buy_on_enter = i.buy_on_enter;
    t.dbl_click_panic_sell = i.dbl_click_panic_sell;
    t.chart_split_zones = i.chart_split_zones;
    t.draw_stop = i.draw_stop;
    t.pending_orders_spread = i.pending_orders_spread;
    t.pending_orders_spread_h_delta = i.pending_orders_spread_h_delta;
    let v = &mut cfg.visual;
    v.hide_forum_label = i.hide_forum_label;
    v.scrolling_charts = i.scrolling_charts;
    v.startup_load_charts = i.startup_load_charts;
    v.hide_right_chart_panel = i.hide_right_chart_panel;
    v.left_chart_info = i.left_chart_info;
    v.show_iceberg = i.show_iceberg;
    v.show_orders_captions = i.show_orders_captions;
    v.orders_captions_lower = i.orders_captions_lower;
    v.hide_pnl = i.hide_pnl;
    v.hide_buy_button = i.hide_buy_button;
    v.hide_cashback_button = i.hide_cashback_button;
    v.remember_chart_buttons = i.remember_chart_buttons;
    v.show_filters.scale_tool = i.scale_tool;
    v.icon_selection = i.icon_selection;
    v.colors.price_line_width = i.price_line_width;
    v.panic_sell_opacity = i.panic_sell_opacity;
    v.book_cumulative_opacity = i.book_cumulative_opacity;
    v.book_orders_opacity = i.book_orders_opacity;
    v.book_orders_width = i.book_orders_width;
    cfg.signals.play_signal_sound = i.play_signal_sound;
    let u = &mut cfg.ui;
    u.confirm_close = i.confirm_close;
    u.hide_demo_button = i.hide_demo_button;
}

/// Apply the leverage-management block.
fn apply_leverage(cfg: &mut SharedConfig, l: &LeverageSettings) {
    let m = &mut cfg.trading.auto_manage_lev;
    m.auto_max_order = l.auto_max_order;
    m.auto_lev_up = l.auto_lev_up;
    // Isolated and cross are mutually exclusive in Moonbot; the caller owns which one is set, and
    // both are written so turning one on turns the other off in the same packet.
    m.auto_isolated = l.auto_isolated;
    m.auto_cross = l.auto_cross;
    m.tlg_report = l.tlg_report;
    m.auto_fix_lev = l.auto_fix_lev;
    m.fix_lev = l.fix_lev;
    cfg.trading.auto_lev_control = l.lev_control.clone();
}

/// Apply the AutoStart tab to `trading.auto_start` and `trading.auto_start_2`.
fn apply_auto_start(cfg: &mut SharedConfig, s: &AutoStartSettings) {
    let a = &mut cfg.trading.auto_start;
    a.auto_start = s.auto_start;
    a.auto_detect_on = s.auto_detect_on;
    a.strategies_on = s.strategies_on;
    a.remember_state = s.remember_state;
    a.auto_update = s.auto_update;
    a.dont_wait_sells = s.dont_wait_sells;
    a.work_time = s.work_time;
    // The wire fraction holds more precision than one minute, so rewriting it from an unchanged
    // minute value would drift the core's own boundary (0.9999 -> 0.99930...) on every OK press.
    if day_fraction_to_minutes(a.work_time_from) != s.work_time_from_min {
        a.work_time_from = minutes_to_day_fraction(s.work_time_from_min);
    }
    if day_fraction_to_minutes(a.work_time_to) != s.work_time_to_min {
        a.work_time_to = minutes_to_day_fraction(s.work_time_to_min);
    }
    a.auto_stop_if_loss = s.auto_stop_if_loss;
    a.auto_stop_loss = s.auto_stop_loss;
    a.stop_trades = s.stop_trades;
    a.sell_if_loss = s.sell_if_loss;
    a.auto_stop_if_loss_hours = s.auto_stop_if_loss_hours;
    a.auto_stop_hours_val = s.auto_stop_hours_val;
    a.stop_hours = s.stop_hours;
    a.stop_hours_trades = s.stop_hours_trades;
    a.ignore_emulator = s.ignore_emulator;
    a.panic_btc = s.panic_btc;
    a.panic_btc_delta = s.panic_btc_delta;
    a.panic_btc_delta_up = s.panic_btc_delta_up;
    a.panic_market = s.panic_market;
    a.panic_market_delta = s.panic_market_delta;
    a.auto_stop_on_errors = s.auto_stop_on_errors;
    a.errors_level = s.errors_level;
    a.sell_all_on_errors = s.sell_all_on_errors;
    a.restart_after_err = s.restart_after_err;
    a.restart_err_time = s.restart_err_time;
    a.auto_stop_on_ping = s.auto_stop_on_ping;
    a.ping_level = s.ping_level;
    a.sell_all_on_ping = s.sell_all_on_ping;
    a.restart_after_ping = s.restart_after_ping;
    a.restart_ping_time = s.restart_ping_time;

    let a2 = &mut cfg.trading.auto_start_2;
    a2.reset_session = s.reset_session;
    a2.rs_hours = s.rs_hours;
    a2.max_session_cap = s.max_session_cap;
    a2.restart_on_market = s.restart_on_market;
    a2.btc_higher_then = s.btc_higher_then;
    a2.btc_lower_then = s.btc_lower_then;
    a2.market_higher_then = s.market_higher_then;
}

/// Apply the BTC blink and alarm controls drawn at the bottom of the AutoStart tab to
/// `visual.blink_config`. A separate function from [`apply_auto_start`] because [`FieldMask`]
/// tracks the two as separate [`CoreConfigArea`] areas, matching [`CoreConfig::btc_blink`] being
/// its own projected section.
fn apply_btc_blink(cfg: &mut SharedConfig, blink: &BtcBlinkSettings) {
    let b = &mut cfg.visual.blink_config;
    b.blink_btc = blink.blink_btc;
    b.blink_btc_delta = blink.blink_btc_delta;
    b.blink_btc_delta_up = blink.blink_btc_delta_up;
    b.alarm_btc = blink.alarm_btc;
    b.alarm_type = blink.alarm_type;
}

#[cfg(test)]
mod tests;
