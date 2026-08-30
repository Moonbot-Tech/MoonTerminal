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

use moonproto::shared_config::SharedConfig;
use moonproto::MoonClient;

use crate::feed::{
    day_fraction_to_minutes, minutes_to_day_fraction, AutoStartSettings, BtcBlinkSettings,
    CoreConfig, CoreConfigArea, CoreConfigEditEvent, CoreConfigEditPhase, CoreConfigEditResult,
    CoreConfigEditRow, CoreConfigMismatch, CoreConfigRejection, CoreHotkeyAction, CoreHotkeyLayout,
    CoreStratButtons, GeneralSettings, LeverageSettings, ManualSettings,
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
/// arrives cannot restore each other's fields, and the gear popup's mask — naming only the four
/// rendered sections — cannot reach the manual block at all, checkbox on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldMask {
    auto_start: bool,
    btc_blink: bool,
    general: bool,
    leverage: bool,
    /// Bit `i` set means slot `i` (0-based) of `ManualSettings::order_sizes` is touched.
    order_size_slots: u8,
    order_size_sel: bool,
}

impl FieldMask {
    /// No fields touched. Base value for building a narrow mask with the `with_*` methods below.
    pub const EMPTY: Self = Self {
        auto_start: false,
        btc_blink: false,
        general: false,
        leverage: false,
        order_size_slots: 0,
        order_size_sel: false,
    };

    /// The four gear-popup sections and nothing else. The manual block is deliberately absent: a
    /// popup OK may never change a manual-trading field, checkbox on or off — see
    /// `commit_core_draft` in `moon-ui-gpui`.
    pub const RENDERED_SECTIONS: Self = Self {
        auto_start: true,
        btc_blink: true,
        general: true,
        leverage: true,
        order_size_slots: 0,
        order_size_sel: false,
    };

    /// Name one order-size preset slot (0-based, clamped into `0..6` like
    /// [`ManualSettings::order_size_sel`]).
    pub fn with_order_size_slot(mut self, slot: usize) -> Self {
        self.order_size_slots |= 1 << slot.min(5);
        self
    }

    /// Name the selected order-size slot.
    pub fn with_order_size_sel(mut self) -> Self {
        self.order_size_sel = true;
        self
    }

    fn touches_order_size_slot(self, slot: usize) -> bool {
        self.order_size_slots & (1 << slot) != 0
    }

    fn union(self, other: Self) -> Self {
        Self {
            auto_start: self.auto_start || other.auto_start,
            btc_blink: self.btc_blink || other.btc_blink,
            general: self.general || other.general,
            leverage: self.leverage || other.leverage,
            order_size_slots: self.order_size_slots | other.order_size_slots,
            order_size_sel: self.order_size_sel || other.order_size_sel,
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
            self.waiting_for_echo = false;
        }
        let Ok(config) = client.settings().build_shared_config() else {
            return;
        };
        match self.next_action(&config, events) {
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
                                mismatches: None,
                            },
                        )));
                        log::info!(
                            "core {} shared config sent ({edit_count} edits)",
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
        events: &mut Vec<CoreConfigEditEvent>,
    ) -> SequenceAction {
        if self.waiting_for_echo {
            return SequenceAction::Idle;
        }
        if let Some((expected, touched, edit_count)) = self.pending_confirmation.take() {
            let actual = core_config_from_proto(config);
            if actual == expected {
                for _ in 0..edit_count {
                    self.queue.pop_front();
                }
                events.push(CoreConfigEditEvent::Resolved(
                    CoreConfigEditResult::Confirmed,
                ));
            } else if let Some(rejection) = rejection_within_mask(&expected, &actual, touched) {
                // Not dequeued: the entries stay queued and MAX_ATTEMPTS below still ends it.
                events.push(CoreConfigEditEvent::Resolved(
                    CoreConfigEditResult::NotApplied(rejection),
                ));
            }
            // Else: the whole projection differs only in fields this edit never touched — a
            // concurrent core-side change, not a rejection (goal A's B6 property). Emit nothing
            // and replan below on the fresh base.
        }
        loop {
            let Some(head) = self.queue.front() else {
                return SequenceAction::Idle;
            };
            if edit_satisfied(config, &head.config) {
                self.queue.pop_front();
                continue;
            }
            if head.attempts >= MAX_ATTEMPTS {
                // Before the mask this could only dump the whole packet, because nothing knew
                // which of ~530 fields the core still disagreed on. Now the mask names exactly
                // what THIS edit touched, so the log names that instead.
                log::error!(
                    "shared config write gave up after {MAX_ATTEMPTS} sends: the core's \
                     configuration still differs on {:?}, so it is NOT applied",
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

/// Whether the core's snapshot already carries everything this write would set.
fn edit_satisfied(config: &SharedConfig, wanted: &CoreConfig) -> bool {
    &core_config_from_proto(config) == wanted
}

/// What the echo disagreed with the terminal about, restricted to the fields `touched` actually
/// names — never the whole projection, so a concurrent core-side change to an untouched field
/// cannot read as this edit's rejection. `None` means every touched field matches: the mismatch
/// lies entirely outside what this edit asked to change.
fn rejection_within_mask(
    expected: &CoreConfig,
    actual: &CoreConfig,
    touched: FieldMask,
) -> Option<CoreConfigRejection> {
    let mut areas = Vec::new();
    if touched.auto_start && expected.auto_start != actual.auto_start {
        areas.push(CoreConfigArea::AutoStart);
    }
    if touched.btc_blink && expected.btc_blink != actual.btc_blink {
        areas.push(CoreConfigArea::BtcBlink);
    }
    if touched.general && expected.general != actual.general {
        areas.push(CoreConfigArea::General);
    }
    if touched.leverage && expected.leverage != actual.leverage {
        areas.push(CoreConfigArea::Leverage);
    }
    if !areas.is_empty() {
        return Some(CoreConfigRejection::Areas(areas));
    }
    let mut fields = Vec::new();
    for slot in 0..6 {
        if !touched.touches_order_size_slot(slot) {
            continue;
        }
        let (requested, actual_v) = (
            expected.manual.order_sizes[slot],
            actual.manual.order_sizes[slot],
        );
        // Same reasoning as `ManualSettings`'s hand-written `PartialEq`: this is an
        // equality-of-snapshots test, not an IEEE numeric comparison, so a core holding a
        // non-finite preset cannot spuriously mismatch itself.
        if requested.total_cmp(&actual_v).is_ne() {
            fields.push(CoreConfigMismatch::OrderSizeSlot {
                slot,
                requested,
                actual: actual_v,
            });
        }
    }
    if touched.order_size_sel && expected.manual.order_size_sel != actual.manual.order_size_sel {
        fields.push(CoreConfigMismatch::OrderSizeSel {
            requested: expected.manual.order_size_sel,
            actual: actual.manual.order_size_sel,
        });
    }
    (!fields.is_empty()).then_some(CoreConfigRejection::Fields(fields))
}

/// Project the settings the terminal renders out of a full safe-share snapshot.
pub(super) fn core_config_from_proto(cfg: &SharedConfig) -> CoreConfig {
    let a = &cfg.trading.auto_start;
    let a2 = &cfg.trading.auto_start_2;
    let b = &cfg.visual.blink_config;
    let t = &cfg.trading;
    let m = &cfg.trading.auto_manage_lev;
    let hotkeys = &cfg.ui.hotkeys_config;
    let strat_buttons = &cfg.trading.manual_strats_config;
    CoreConfig {
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
    if touched.auto_start {
        apply_auto_start(cfg, &wanted.auto_start);
    }
    if touched.btc_blink {
        apply_btc_blink(cfg, &wanted.btc_blink);
    }
    if touched.general {
        apply_general(cfg, &wanted.general);
    }
    if touched.leverage {
        apply_leverage(cfg, &wanted.leverage);
    }
    apply_manual(cfg, &wanted.manual, touched);
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

/// Write the manual block's write half, gated by `touched`: nothing here is written unless the
/// caller named it at enqueue time (see [`FieldMask`] and the module doc). The popup's mask never
/// names any of this — see `commit_core_draft` in `moon-ui-gpui` — so today the only producer that
/// can reach it is a narrow toolbar write.
fn apply_manual(cfg: &mut SharedConfig, m: &ManualSettings, touched: FieldMask) {
    let hotkeys = &mut cfg.ui.hotkeys_config;
    for slot in 0..6 {
        if touched.touches_order_size_slot(slot) {
            hotkeys.o_size[slot] = m.order_sizes[slot];
        }
    }
    if touched.order_size_sel {
        // GUARD A — conditional `b_num` write, identical shape and reasoning to
        // `apply_auto_start`'s `work_time_from` guard above ("rewriting it from an unchanged
        // minute value would drift the core's own boundary"): `order_size_sel` is a lossy
        // projection (`b_num.saturating_sub(1).clamp(0,5)`), so writing `b_num` back from an
        // unchanged selection would silently move a core with `b_num = 0` to slot 1.
        if hotkeys.b_num.saturating_sub(1).clamp(0, 5) as usize != m.order_size_sel {
            hotkeys.b_num = (m.order_size_sel.min(5) as i32) + 1;
        }
    }
}

#[cfg(test)]
mod tests;
