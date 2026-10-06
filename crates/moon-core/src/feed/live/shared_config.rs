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

mod field_mask;
mod from_proto;
mod satisfaction;
mod wire_writers;

pub use field_mask::FieldMask;
pub(crate) use from_proto::core_config_from_proto;
use satisfaction::{edit_satisfied, rejection_within_mask};
pub(super) use wire_writers::apply_core_config;

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

/// What one packet in flight asked for.
struct PendingConfirmation {
    /// The projection expected back from the core.
    expected: CoreConfig,
    /// What the packet carried; see [`SentAsk`].
    ask: SentAsk,
}

/// The half of a sent packet that both the plan and the confirmation read.
#[derive(Clone)]
struct SentAsk {
    /// Union of the confirmed entries' masks, which scopes the projection comparison.
    touched: FieldMask,
    /// Markets whose membership the packet decided, checked against the packet's own list.
    fav: Vec<String>,
    /// How many queue entries this packet confirms.
    edit_count: usize,
}

impl Default for SentAsk {
    fn default() -> Self {
        Self {
            // `FieldMask` has no `Default` on purpose — an all-false mask is a decision, not an
            // absence — so the empty one is named here rather than derived.
            touched: FieldMask::EMPTY,
            fav: Vec::new(),
            edit_count: 0,
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
    /// What the packet in flight asked for; see [`PendingConfirmation`].
    pending_confirmation: Option<PendingConfirmation>,
    /// Whether the "queued edits, no base snapshot" stall has been reported. Latched to one line
    /// per stall, not one per feed-loop iteration; cleared once a base is available again.
    missing_snapshot_logged: bool,
    /// Whether the "waiting behind the compact settings channel" stall has been reported, latched
    /// per queued edit for the same reason. Cleared by the next `enqueue`.
    gated_logged: bool,
    /// The marked-markets list as `channels.settings` last reported it, so a line is written when
    /// the core's own list MOVES rather than once per settings event.
    ///
    /// `None` until the first observation, which is itself worth a line: it answers "does this core
    /// carry anything in that field at all", which is the first question asked of it.
    fav_logged: Option<String>,
    /// Every non-empty text field as `channels.settings` last reported it; see
    /// [`Self::note_settings_text`].
    text_logged: Option<Vec<String>>,
}

/// What ONE queued entry asks the core for.
///
/// An enum rather than a struct with optional halves, and the same shape the compact channel's
/// `SettingsMutation` uses next door: a projection and a list delta are two different asks, and a
/// value that can hold both at once is a state no caller can produce and every reader must branch
/// on.
enum QueuedOp {
    /// A complete projection, written under the mask naming what it may touch.
    ///
    /// Boxed because it is two orders of magnitude larger than the delta beside it, and every
    /// queued entry would otherwise carry that size whichever kind it is.
    Config {
        config: Box<CoreConfig>,
        touched: FieldMask,
    },
    /// One market marked or unmarked, applied to the snapshot at SEND time.
    ///
    /// A DELTA rather than a whole list, for the reason the temporary blacklist is one: the core
    /// writes this field too — MoonBot's own star, a second terminal, and this queue's own earlier
    /// entry — and a string frozen when the trader pressed would delete whatever arrived in
    /// between. Nothing else about the queue changes: the send already rebuilds from the freshly
    /// retained snapshot, so a delta applied there is simply the honest form of this one field.
    Fav { market: String, on: bool },
}

impl QueuedOp {
    /// What this entry names, for the two log lines that report an entry leaving the queue.
    fn describe(&self) -> String {
        match self {
            QueuedOp::Config { touched, .. } => format!("{touched:?}"),
            QueuedOp::Fav { market, on } => format!("favorite {market}={on}"),
        }
    }
}

/// One queued write and how many times it has been sent without a matching echo.
struct QueuedEdit {
    op: QueuedOp,
    attempts: u8,
}

/// Whether the core's list already says what these marks asked for.
///
/// Membership, never string equality: the core may reorder the list or re-space it, and an exact
/// comparison would leave a write that LANDED unconfirmed until the attempt budget dropped it as
/// refused. Case is folded, being the rule every symbol comparison here uses; a core that respells
/// a name any further has changed it, and that is a rejection rather than a match. The state each
/// market must end in is read from the packet that was SENT, so nothing is remembered twice.
fn fav_met(actual: &str, expected: &str, markets: &[String]) -> bool {
    markets.iter().all(|market| {
        crate::feed::fav_markets_has(actual, market)
            == crate::feed::fav_markets_has(expected, market)
    })
}

/// Pure next action selected from a retained snapshot.
enum SequenceAction {
    /// Nothing to send right now.
    Idle,
    /// Send this complete config and confirm the listed prefix on echo.
    Send {
        config: Box<SharedConfig>,
        /// What the packet asks of the core, carried through to the echo comparison.
        ask: SentAsk,
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
            fav_logged: None,
            text_logged: None,
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
            op: QueuedOp::Config {
                config: Box::new(config),
                touched,
            },
            attempts: 0,
        });
    }

    /// Queue one market to be marked or unmarked in the core's favourites.
    ///
    /// Absolute, not a toggle: the state was decided where the trader pressed, and this queue may
    /// send it several round trips later — see [`crate::feed::fav_markets_set`].
    ///
    /// Args:
    ///     market: Market as the core spells it.
    ///     on: Whether it must be listed afterwards.
    pub(super) fn enqueue_fav_market(&mut self, market: String, on: bool) {
        self.gated_logged = false;
        self.queue.push_back(QueuedEdit {
            op: QueuedOp::Fav { market, on },
            attempts: 0,
        });
    }

    /// Allow the next plan after a `SharedConfigUpdated` echo.
    pub(super) fn observe_update(&mut self) {
        self.waiting_for_echo = false;
    }

    /// Report every non-empty TEXT field of the core's safe-share snapshot to `channels.settings`,
    /// when any of them changes.
    ///
    /// The question it answers is "where does MoonBot keep the list I just edited": its own star
    /// writes SOMEWHERE, the field named for it (`trading.fav_markets`) did not move when a market
    /// was added on the core, and no amount of reading either source settles which field did. The
    /// snapshot carries exactly twenty text fields, so listing the non-empty ones is a short line
    /// that names the answer outright.
    ///
    /// Written on change only, first observation included, for the reason the blacklist line beside
    /// it is: the projection is republished on every settings event.
    ///
    /// Args:
    ///     server_id: Core the line is stamped with.
    ///     cfg: The core's safe-share snapshot, as retained.
    pub(super) fn note_settings_text(&mut self, server_id: u64, cfg: &SharedConfig) {
        if !crate::settings_diag::enabled() {
            self.text_logged = None;
            return;
        }
        let fields: [(&str, &str); 20] = [
            ("trading.fav_markets", &cfg.trading.fav_markets),
            (
                "trading.coins_black_list_text",
                &cfg.trading.coins_black_list_text,
            ),
            (
                "trading.h_pos_black_list_text",
                &cfg.trading.h_pos_black_list_text,
            ),
            (
                "trading.h_pos_black_list_add",
                &cfg.trading.h_pos_black_list_add,
            ),
            ("trading.manual_strategy", &cfg.trading.manual_strategy),
            ("trading.auto_lev_control", &cfg.trading.auto_lev_control),
            (
                "trading.clear_triggers_string",
                &cfg.trading.clear_triggers_string,
            ),
            (
                "trading.no_trades_markets_text",
                &cfg.trading.no_trades_markets_text,
            ),
            ("signals.pump_channel", &cfg.signals.pump_channel),
            ("signals.msg_keywords_long", &cfg.signals.msg_keywords_long),
            (
                "signals.msg_keywords_short",
                &cfg.signals.msg_keywords_short,
            ),
            ("signals.msg_token_tags", &cfg.signals.msg_token_tags),
            ("signals.msg_black_words", &cfg.signals.msg_black_words),
            ("signals.lower_price_words", &cfg.signals.lower_price_words),
            ("signals.strats_filter", &cfg.signals.strats_filter),
            ("signals.news_tags_filter", &cfg.signals.news_tags_filter),
            (
                "signals.news_tokens_filter",
                &cfg.signals.news_tokens_filter,
            ),
            ("visual.ai_card_model", &cfg.visual.ai_card_model),
            ("visual.ai_card_prompt", &cfg.visual.ai_card_prompt),
            ("ui.strat_editor_chapters", &cfg.ui.strat_editor_chapters),
        ];
        // The LENGTH beside the text, and the text cut: one of these fields is an AI prompt that
        // would flood the file, while what is being looked for is a short list — and a length that
        // grew between two lines names the field even when its value was cut.
        let shot: Vec<String> = fields
            .iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .map(|(name, value)| {
                let head: String = value.chars().take(300).collect();
                let cut = match head.chars().count() < value.chars().count() {
                    true => "…",
                    false => "",
                };
                format!("{name} len={} \"{head}{cut}\"", value.len())
            })
            .collect();
        if self.text_logged.as_deref() == Some(shot.as_slice()) {
            return;
        }
        crate::settings_diag::line(&format!(
            "core={} settings text: {}",
            crate::feed::core_label(server_id),
            match shot.is_empty() {
                true => "(every text field is empty)".to_string(),
                false => shot.join(" | "),
            }
        ));
        self.text_logged = Some(shot);
    }

    /// Report the core's marked-markets list to `channels.settings` when it changes.
    ///
    /// The field this terminal's star reads and writes is `trading.fav_markets`, and what MoonBot
    /// itself does with it is not settled by reading either source — the repo's own settings map
    /// marks it "под вопросом". So the question is answered the way this channel answers the two
    /// blacklist ones: by writing down what the CORE actually holds, before and after a change is
    /// made on either side.
    ///
    /// Written on change only, first observation included: the projection is republished on every
    /// compact settings event, and a line per event would bury the one that matters.
    ///
    /// Args:
    ///     server_id: Core the line is stamped with.
    ///     config: The projection just built from that core's snapshot.
    pub(super) fn note_fav_markets(&mut self, server_id: u64, config: &CoreConfig) {
        if !crate::settings_diag::enabled() {
            // Switched off: the next switch-on reports what the core holds then, not what it held
            // when the channel was last on.
            self.fav_logged = None;
            return;
        }
        if self.fav_logged.as_deref() == Some(config.fav_markets.as_str()) {
            return;
        }
        let markets = crate::feed::fav_markets_list(&config.fav_markets);
        crate::settings_diag::line(&format!(
            "core={} fav_markets len={} count={} raw=\"{}\"",
            crate::feed::core_label(server_id),
            config.fav_markets.len(),
            markets.len(),
            config.fav_markets,
        ));
        self.fav_logged = Some(config.fav_markets.clone());
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
    fn observe_send_success(&mut self, config: &SharedConfig, ask: SentAsk) {
        self.waiting_for_echo = true;
        self.sent_at = Some(Instant::now());
        let edit_count = ask.edit_count;
        self.pending_confirmation = Some(PendingConfirmation {
            expected: core_config_from_proto(config),
            ask,
        });
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
            SequenceAction::Send { config, ask } => {
                let (edit_count, touched) = (ask.edit_count, ask.touched);
                match client.settings().send_shared_config(&config) {
                    Ok(()) => {
                        let wanted = core_config_from_proto(&config);
                        self.observe_send_success(&config, ask);
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
    ///
    /// Pushes [`CoreConfigEditEvent::Drained`] when the queue runs empty in this call — after the
    /// verdicts and drops that emptied it, so a reader sees why before it sees that.
    fn next_action(
        &mut self,
        config: &SharedConfig,
        server_id: u64,
        events: &mut Vec<CoreConfigEditEvent>,
    ) -> SequenceAction {
        if self.waiting_for_echo {
            return SequenceAction::Idle;
        }
        let had_work = !self.queue.is_empty();
        if let Some(PendingConfirmation { expected, ask }) = self.pending_confirmation.take() {
            let actual = core_config_from_proto(config);
            // Scoped to the mask, not the whole projection. Anything this write did not name is
            // free to have moved between the send and the echo — a trader in Moonbot's own dialogs
            // — and comparing it would leave a landed edit unconfirmed, re-sending the whole
            // snapshot until the budget ran out and the edit was dropped as `GaveUp`.
            let mut areas = match rejection_within_mask(&expected, &actual, ask.touched) {
                Some(CoreConfigRejection::Areas(areas)) => areas,
                None => Vec::new(),
            };
            // The list is checked by MEMBERSHIP beside the mask's own comparison; see `fav_met`.
            if !fav_met(&actual.fav_markets, &expected.fav_markets, &ask.fav) {
                areas.push(CoreConfigArea::FavMarkets);
            }
            let edit_count = ask.edit_count;
            match (!areas.is_empty()).then_some(CoreConfigRejection::Areas(areas)) {
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
                if had_work {
                    events.push(CoreConfigEditEvent::Drained);
                }
                return SequenceAction::Idle;
            };
            if edit_satisfied(config, &head.op) {
                // The quietest of the three ways an edit leaves the queue: no send line precedes
                // it, so "the core already holds this" and "it was never sent" read alike.
                log::info!(
                    "core {} shared config edit satisfied by the core's own snapshot ({})",
                    crate::feed::core_label(server_id),
                    head.op.describe()
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
                     configuration still differs on {}, so it is NOT applied",
                    crate::feed::core_label(server_id),
                    head.op.describe()
                );
                events.push(CoreConfigEditEvent::Resolved(CoreConfigEditResult::GaveUp));
                self.queue.pop_front();
                continue;
            }
            let mut next = config.clone();
            let mut ask = SentAsk {
                edit_count: self.queue.len(),
                ..SentAsk::default()
            };
            for queued in &self.queue {
                match &queued.op {
                    QueuedOp::Config { config, touched } => {
                        apply_core_config(&mut next, config.as_ref(), *touched);
                        ask.touched = ask.touched.union(*touched);
                    }
                    // Applied to the packet BEING BUILT, so a second mark queued behind the first
                    // builds on it rather than on the snapshot both were queued against. The state
                    // it ends in is then read back off that packet, so it is never carried twice.
                    QueuedOp::Fav { market, on } => {
                        next.trading.fav_markets =
                            crate::feed::fav_markets_set(&next.trading.fav_markets, market, *on);
                        if !ask.fav.iter().any(|held| held.eq_ignore_ascii_case(market)) {
                            ask.fav.push(market.clone());
                        }
                    }
                }
            }
            return SequenceAction::Send {
                config: Box::new(next),
                ask,
            };
        }
    }
}

#[cfg(test)]
mod tests;
