//! Pure moonproto-to-terminal snapshot projections (license/client-settings/lev/runtime),
//! targeted edits to retained settings snapshots, and order-row (`OrderRow`) construction.

use std::sync::Arc;

use moonproto::state::{Order, OrderTraceChartPoint, OrderTraceLine};
use moonproto::{Event, MoonClient};

use crate::config::TakeProfitMode;
use crate::feed::strategies::strat_kind_name;
use crate::feed::{
    ApiKeyExpiry, ClientSettings, ClientSettingsEdit, ConnFault, ConnFaultKind, CoreIdentityFacts,
    CoreInitStep, CoreStartupState, CoreStartupStatus, CoreSysStatus, EngineActionKind,
    EngineActionResult, LicenseState, NewsSnapshot, OrderRow, OrderTrace, OrderTracePoint,
    ProfitState, RuntimeState, WalletKind,
};

/// Project moonproto's retained `NewsState` into a moonproto-free [`NewsSnapshot`]: reduce its flat
/// frame ring into logical items. Called only when an `Event::News` arrives, mirroring the
/// license/settings snapshot idiom.
pub(super) fn news_snapshot_from_proto(news: &moonproto::state::NewsState) -> NewsSnapshot {
    NewsSnapshot {
        items: crate::feed::news::reduce(news.items()),
        catalog: crate::feed::news::parse_catalog(news.tags_json()),
    }
}

fn trace_point(p: OrderTraceChartPoint) -> OrderTracePoint {
    OrderTracePoint {
        time_ms: p.unix_millis() as f64,
        price: p.price,
    }
}

fn valid_trace_point(p: &OrderTracePoint) -> bool {
    p.time_ms > 1.0 && p.price.is_finite() && p.price > 0.0
}

fn valid_trace_tmp_point(p: OrderTraceChartPoint) -> Option<OrderTracePoint> {
    let time_ms = p.unix_millis() as f64;
    (time_ms > 1.0 && p.price.is_finite() && p.price > 0.0).then_some(OrderTracePoint {
        time_ms,
        price: p.price,
    })
}

fn moon_time_to_unix_millis_f64(time: moonproto::MoonTime) -> f64 {
    let millis = time.unix_millis();
    if millis > 0 { millis as f64 } else { 0.0 }
}

pub(super) fn license_state_from_proto(
    license: moonproto::KernelLicenseStateCommand,
) -> LicenseState {
    LicenseState {
        paid_version: license.paid_version,
        reg_id: license.reg_id,
        moon_credits: license.moon_credits,
        moon_credits_hold: license.moon_credits_hold,
        moon_credits_auction: license.moon_credits_auction,
        can_use_watcher: license.can_use_watcher,
        // News-module subscription/trial, previously dropped; surfaced in the News panel footer.
        // Guard non-positive (pre-1970 Delphi) times to None, matching the sibling time converters,
        // so a malformed stamp reads as "no subscription" rather than "expired".
        news_valid_until: license
            .news_valid_until
            .map(|t| t.unix_millis())
            .filter(|&ms| ms > 0),
        news_trial_used: license.news_trial_used,
    }
}

/// Computes the toolbar's main TP from moonproto `ClientSettings`. Raw fields
/// (`s_price`/`sb_num`/...) are `pub(crate)` in production, so read them ONLY through helpers.
/// This is the button's own TP (from `x_sell`/scalp), IGNORING `fixed_sell_mode`: the non-fixed-sell
/// branch of `effective_take_profit_percent`, ensuring an S-slot selection does not replace the
/// displayed main TP.
fn main_take_profit_percent(c: &moonproto::ClientSettingsCommand) -> f64 {
    if c.x_sell > 0 {
        let mut value = f64::from(c.x_sell);
        if c.x_tmode {
            value *= 10.0;
        }
        value.min(900.0)
    } else {
        f64::from(c.x_sell_scalp) / 50.0
    }
}

/// Convert the complete MoonProto settings snapshot into the terminal's retained projection.
pub(super) fn client_settings_from_proto(c: &moonproto::ClientSettingsCommand) -> ClientSettings {
    let fixed_sell_pcts =
        std::array::from_fn(|i| c.fixed_sell_preset_percent(i + 1).unwrap_or(0.0));
    ClientSettings {
        take_profit_pct: c.effective_take_profit_percent(),
        take_profit_main_pct: main_take_profit_percent(c),
        take_profit_extended: c.x_tmode,
        take_profit_mode: if c.x_sell == 0 {
            TakeProfitMode::Scalp
        } else if c.x_tmode {
            TakeProfitMode::Extended
        } else {
            TakeProfitMode::Normal
        },
        fixed_sell_mode: c.fixed_sell_mode,
        stop_loss_pct: c.price_drop_level,
        trailing_drop_pct: c.trailing_drop,
        use_global_take_profit: c.use_g_take_profit,
        global_take_profit_pct: c.g_take_profit,
        panic_if_price_drop: c.panic_if_price_drop,
        emu_mode: c.emu_mode,
        buy_iceberg: c.buy_iceberg,
        sell_iceberg: c.sell_iceberg,
        sign_orders: c.sign_orders,
        use_stop_market: c.use_stop_market,
        vol_drop_level: c.vol_drop_level,
        use_blacklist: c.use_coins_black_list,
        blacklist_text: c.coins_black_list_text.clone(),
        fixed_sell_pcts,
        fixed_sell_slot: c.selected_fixed_sell_slot(),
        use_manual_strategy: c.use_manual_strategy,
        manual_strategy_id: c.manual_strategy_id,
    }
}

pub(super) fn runtime_state_from_proto(s: &moonproto::RuntimeStateCommand) -> RuntimeState {
    RuntimeState {
        is_started: s.is_started,
        auto_detect_active: s.auto_detect_active,
    }
}

/// Project the core's report profit counters shown beside the AutoStart loss caps.
///
/// The two pairs are the core's own trade-window and hourly counters; they come from the report
/// database rather than from balances, so they can legitimately disagree with the header P&L.
pub(super) fn profit_state_from_proto(s: &moonproto::ProfitStateCommand) -> ProfitState {
    ProfitState {
        total_profit: s.rep_total_profit,
        total_trades: s.rep_total_trades,
        hourly_profit: s.rep_trades_total,
        hourly_trades: s.rep_count_trades,
    }
}

/// Reads a retained core snapshot only when the batch contains any event matched by the caller's
/// predicate, including non-settings events such as `KernelHealth`; otherwise returns `None`.
/// Snapshot access is cheap, but unnecessary without a matching event.
pub(super) fn settings_event_snapshot<T>(
    events: &[Event],
    client: &MoonClient,
    matched: impl Fn(&Event) -> bool,
    extract: impl FnOnce(Arc<moonproto::MoonStateSnapshot>) -> Option<T>,
) -> Option<T> {
    events
        .iter()
        .any(matched)
        .then(|| client.snapshot())
        .flatten()
        .and_then(extract)
}

/// Convert protocol-v4 `KernelHealth` into terminal telemetry and stamp its
/// receipt time. The caller reads `KernelHealth` from `snapshot().kernel_health()`
/// (its retained value keeps the last memory sample between CPU-only Pings), not
/// from the raw event. Process vs system CPU is a SCOPE distinction — each keeps
/// its own field so machine-wide CPU never renders as a process average.
pub(super) fn sys_status_from_proto(
    h: moonproto::state::KernelHealth,
    updated_ms: i64,
) -> CoreSysStatus {
    CoreSysStatus {
        process_cpu_percent: Some(h.process_cpu_percent),
        system_cpu_percent: Some(h.system_cpu_percent),
        used_memory_mb: h.used_memory_mb,
        free_physical_memory_mb: h.free_physical_memory_mb,
        logical_cpu_count: h.logical_cpu_count,
        round_trip_ms: h.core_round_trip_ms,
        order_api_latency_ms: h.order_api_latency_ms,
        updated_ms,
    }
}

/// Project one moonproto `StartupState` into the terminal's own phase.
///
/// The wildcard is load-bearing, not defensive padding: moonproto's enum is `#[non_exhaustive]`, so
/// a newer library can report a phase this build has never seen. Mapping it onto a known phase
/// would render a guess as fact, so it lands on `Unknown` and the UI shows no progress for it.
fn startup_state_from_proto(state: moonproto::StartupState) -> CoreStartupState {
    match state {
        moonproto::StartupState::Connecting => CoreStartupState::Connecting,
        moonproto::StartupState::Initializing => CoreStartupState::Initializing,
        moonproto::StartupState::Ready => CoreStartupState::Ready,
        moonproto::StartupState::Reconnecting => CoreStartupState::Reconnecting,
        moonproto::StartupState::Failed => CoreStartupState::Failed,
        moonproto::StartupState::Disconnected => CoreStartupState::Disconnected,
        _ => CoreStartupState::Unknown,
    }
}

/// Project one moonproto `InitStep` into the terminal's own step.
///
/// Same `#[non_exhaustive]` reasoning as [`startup_state_from_proto`]: an unrecognised step becomes
/// `None` — "the core is between steps we can name" — rather than being folded onto a real one,
/// which would report the wrong step as current.
fn init_step_from_proto(step: moonproto::InitStep) -> Option<CoreInitStep> {
    Some(match step {
        moonproto::InitStep::BaseCheck => CoreInitStep::BaseCheck,
        moonproto::InitStep::AuthCheck => CoreInitStep::AuthCheck,
        moonproto::InitStep::GetMarketsList => CoreInitStep::GetMarketsList,
        moonproto::InitStep::UpdateMarketsList => CoreInitStep::UpdateMarketsList,
        moonproto::InitStep::StrategySchema => CoreInitStep::StrategySchema,
        moonproto::InitStep::PostInitFlush => CoreInitStep::PostInitFlush,
        moonproto::InitStep::StartupSnapshot => CoreInitStep::StartupSnapshot,
        moonproto::InitStep::StartupEvents => CoreInitStep::StartupEvents,
        _ => return None,
    })
}

/// Convert moonproto's passive startup snapshot into moonproto-free terminal state.
///
/// The completed-step SET is re-encoded as our own bitmask rather than carried across, because
/// moonproto's `InitStepSet` can only be read, never built, outside its crate — so a mirror the
/// terminal can construct in a test needs its own representation. Steps this build does not
/// recognise are dropped from the mask instead of shifting the ones it does.
pub(super) fn startup_status_from_proto(s: moonproto::StartupStatus) -> CoreStartupStatus {
    let mut completed_mask = 0u16;
    for step in s.completed_steps.iter() {
        if let Some(step) = init_step_from_proto(step) {
            completed_mask |= 1 << step as u8;
        }
    }
    CoreStartupStatus {
        state: startup_state_from_proto(s.state),
        current_step: s.current_step.and_then(init_step_from_proto),
        completed_mask,
        elapsed_ms: s.elapsed_ms,
        received_sliced_bytes: s.received_sliced_bytes,
        receive_rate_bytes_per_sec: s.receive_rate_bytes_per_sec,
        active_sliced_transfers: s.active_sliced_transfers,
        received_sliced_blocks: s.received_sliced_blocks,
        duplicate_sliced_blocks: s.duplicate_sliced_blocks,
        active_received_blocks: s.active_received_blocks,
        active_expected_blocks: s.active_expected_blocks,
        idle_for_ms: s.idle_for_ms,
        current_step_retries: s.current_step_retries,
        total_init_retries: s.total_init_retries,
        reconnect_count: s.reconnect_count,
        current_local_udp_port: s.current_local_udp_port,
        current_port_sent_packets: s.current_port_sent_packets,
        current_port_received_packets: s.current_port_received_packets,
        previous_local_udp_port: s.previous_local_udp_port,
        sent_packets_before_last_port_change: s.sent_packets_before_last_port_change,
        received_packets_before_last_port_change: s.received_packets_before_last_port_change,
        local_port_change_count: s.local_port_change_count,
        round_trip_ms: s.round_trip_ms,
        path_mtu_bytes: s.path_mtu_bytes,
        downlink_delivery_percent: s.downlink_delivery_percent,
    }
}

/// Resolve MoonProto's own init-step NAME back to a step this build recognises.
///
/// `InitError` names the step as a `&'static str` rather than as the `InitStep` enum
/// [`init_step_from_proto`] converts, so this is the only bridge between the two. The match is
/// EXACT and total over the eight steps this build recognises; the strategy-schema step accepts its
/// two wire spellings. Anything else returns `None` and the caller keeps the raw name. A step
/// renamed upstream therefore degrades to "we cannot name this stage" rather than to a confidently
/// wrong one — the same `#[non_exhaustive]` discipline [`init_step_from_proto`] applies, expressed
/// over strings because that is what the error carries.
///
/// Args:
///     raw: MoonProto's wire name for the init step.
///
/// Returns:
///     The matching terminal step, or `None` for an upstream name this build does not know.
fn init_step_by_name(raw: &str) -> Option<CoreInitStep> {
    Some(match raw {
        "BaseCheck" => CoreInitStep::BaseCheck,
        "AuthCheck" => CoreInitStep::AuthCheck,
        "GetMarketsList" => CoreInitStep::GetMarketsList,
        "UpdateMarketsList" => CoreInitStep::UpdateMarketsList,
        // The strategy-schema step reports itself by its WIRE command name, not by the enum
        // variant's name — verified live in MoonProto `init/steps.rs`, outside any `cfg(test)`.
        // Both spellings map here so a rename on either side degrades one of them, never both.
        "StrategySchema" | "TStratSchemaRequest" => CoreInitStep::StrategySchema,
        "PostInitFlush" => CoreInitStep::PostInitFlush,
        "StartupSnapshot" => CoreInitStep::StartupSnapshot,
        "StartupEvents" => CoreInitStep::StartupEvents,
        _ => return None,
    })
}

/// Project what `BaseCheck` reported into the moonproto-free identity facts.
///
/// A pure copy of what the payload held, with NO inference added. In particular nothing here
/// derives "the core answered" from the startup step mask: the mask and the payload come from two
/// different publication paths, and MoonProto refreshes the payload's snapshot only at Ready
/// (`publish_snapshot_profiled` is gated on `startup.is_none()`, and a failed attempt publishes
/// nothing at all), so on a failing attempt the mask can say `BaseCheck` completed while the payload
/// is still the empty default. Combining the two manufactures "answered, and reported no version"
/// out of a core that has merely not published yet, which is a confident lie about the user's core.
/// [`CoreIdentityFacts`] carries the full argument.
///
/// Args:
///     info: What `MoonClient::server_info` held at the failure site, if anything.
///
/// Returns:
///     The identity facts a verdict may rest on.
fn identity_facts(info: Option<moonproto::ServerInfo>) -> CoreIdentityFacts {
    let info = info.unwrap_or_default();
    CoreIdentityFacts {
        has_identity: info.has_identity(),
        server_version: info.server_version,
        moonproto_version: info.moonproto_version,
    }
}

/// Project MoonProto's typed startup failure into the moonproto-free [`ConnFault`].
///
/// This is the ONLY place the typed `ConnectError` is read. It is deliberately a projection rather
/// than a classification: it renames the wire shape and nothing more, so the decision about WHICH
/// user-facing cause a shape means — and the wording for it — stays in the layer that can localize.
///
/// Args:
///     error: MoonProto's typed failure for this connection attempt.
///     info: `BaseCheck` result, when the core got that far.
///     startup: Startup snapshot polled AT the failure site.
///
/// Returns:
///     The fault record retained for this core until it next reaches `Ready`.
pub(super) fn conn_fault_from_proto(
    error: moonproto::ConnectError,
    info: Option<moonproto::ServerInfo>,
    startup: moonproto::StartupStatus,
) -> ConnFault {
    let kind = match error {
        // Both mean "the terminal stopped this attempt", never "the core is bad", so they carry no
        // stage: there is nothing about the core to report.
        moonproto::ConnectError::Canceled => ConnFaultKind::Aborted,
        moonproto::ConnectError::Init(moonproto::InitError::SendChannelClosed) => {
            ConnFaultKind::Aborted
        }
        moonproto::ConnectError::ConnectTimedOut { timeout } => ConnFaultKind::ConnectTimedOut {
            timeout_ms: timeout.as_millis() as u64,
        },
        moonproto::ConnectError::Init(moonproto::InitError::NotAuthenticated) => {
            ConnFaultKind::NotAuthenticated
        }
        moonproto::ConnectError::Init(moonproto::InitError::CriticalStepTimedOut(step)) => {
            ConnFaultKind::InitStepTimedOut {
                step: init_step_by_name(step),
                raw_step: step.to_string(),
            }
        }
        moonproto::ConnectError::Init(moonproto::InitError::CriticalStepFailed {
            step,
            message,
        }) => ConnFaultKind::InitStepFailed {
            step: init_step_by_name(step),
            raw_step: step.to_string(),
            message,
        },
    };
    fault(kind, info, startup_status_from_proto(startup))
}

/// Assemble one fault record.
///
/// The three producers differ only in the kind they name; identity and the frozen snapshot are the
/// same record whatever ended the attempt, and are built here so a new field on [`ConnFault`], or
/// any change to how identity is derived, is one edit rather than three.
///
/// Args:
///     kind: What ended the attempt.
///     info: `BaseCheck` result, when the core got that far.
///     startup: Startup snapshot captured at the failure site.
///
/// Returns:
///     The fault record.
fn fault(
    kind: ConnFaultKind,
    info: Option<moonproto::ServerInfo>,
    startup: CoreStartupStatus,
) -> ConnFault {
    ConnFault {
        kind,
        identity: identity_facts(info),
        startup,
    }
}

/// Build the fault for `LifecycleEvent::BindFailed` — the terminal's OWN socket, not the core's.
///
/// It takes the same identity and startup arguments as [`conn_fault_from_proto`] so both faults are
/// the same record whatever produced them; both are normally empty here, because a client that
/// cannot bind never reached the core to learn anything about it.
///
/// Args:
///     consecutive_failures: Complete 200-port bind sweeps that failed in a row.
///     info: `BaseCheck` result, when an earlier connection on this client got that far.
///     startup: Startup snapshot polled at the moment the bind failure was reported.
///
/// Returns:
///     The fault record for a local bind failure.
pub(super) fn bind_fault(
    consecutive_failures: u32,
    info: Option<moonproto::ServerInfo>,
    startup: moonproto::StartupStatus,
) -> ConnFault {
    fault(
        ConnFaultKind::LocalBindFailed {
            consecutive_failures,
        },
        info,
        startup_status_from_proto(startup),
    )
}

/// Build the fault for a first startup this terminal gave up on — see
/// [`super::startup_watchdog`].
///
/// Carries the reason across the crate boundary as facts, so the panel words it through the
/// existing localized "stalled at this step" verdict rather than through the untranslatable
/// raw-text fallback every other `live::run` error lands on. It gets its OWN kind rather than
/// borrowing [`ConnFaultKind::InitStepTimedOut`]: see that variant's sibling for why a step nobody
/// answered must not be read as evidence about the core.
///
/// Takes the ALREADY-PROJECTED snapshot, unlike its two siblings: the caller is the startup poll,
/// which converted it one line earlier, and re-reading `startup_status()` here would describe a
/// slightly later moment than the one that decided to give up.
///
/// Args:
///     info: `BaseCheck` result, when the core got that far before stalling.
///     startup: Startup snapshot that the stall was detected on.
///
/// Returns:
///     The fault record for a stalled startup.
pub(super) fn stall_fault(
    info: Option<moonproto::ServerInfo>,
    startup: CoreStartupStatus,
) -> ConnFault {
    fault(ConnFaultKind::StartupStalled, info, startup)
}

/// Convert one successful `CheckAPIExpirationTime` answer into terminal state.
///
/// The day count is the core's own (`reported_days_left`), because the terminal's clock plays no
/// part in it. A legacy core that predates that field falls back to `days_until`, which compares
/// moonproto's UNNORMALIZED server-local timestamp against the local clock and can therefore be a
/// day out; that answer also carries no absolute date, so its reader ages the count instead.
///
/// An empty date field alone does NOT mean "no expiration": the parser zeroes the date whenever the
/// core's timestamp is unusable, and still returns the count beside it. Only an empty date with no
/// counting-down number is an unlimited key, and that is recorded as its own flag rather than
/// re-derived downstream — see [`api_days_and_unlimited`].
///
/// A day count outside [`SANE_DAYS`] is dropped rather than displayed — see that constant for why
/// the negative side is the narrow one.
///
/// Args:
///     expiration: The parsed expiration from `AccountEvent::ApiExpirationUpdated`.
///     now: Terminal clock used for the legacy fallback and the receipt stamp.
///
/// Returns:
///     Terminal-side expiration state for the store.
pub(super) fn api_key_expiry_from_proto(
    expiration: moonproto::ApiExpirationTime,
    now: std::time::SystemTime,
) -> ApiKeyExpiry {
    let known = expiration.is_known();
    let reported = expiration.reported_days_left();
    // A CURRENT answer is the one that carries `reported_days_left`; only for those is the absolute
    // date normalized to this terminal's clock. A legacy answer's date is the core's own local
    // timestamp, which nothing corrects, so it is deliberately NOT kept: taking it would make the
    // un-normalized value the preferred source and put a core in another time zone a day out.
    let current = reported.is_some();
    let dated_days = known.then(|| expiration.days_until(now)).flatten();
    let (days_left, unlimited) = api_days_and_unlimited(known, reported, dated_days);
    let checked_ms = crate::util::unix_ms_i64_of(now);
    // The date is the third wire field and the day count is the second, so a core could answer with
    // one sane and the other not. Keep the date only while BOTH agree on being plausible — the
    // reader prefers the date, and an unchecked one would slip past the guard above.
    let at_unix = (known && current)
        .then(|| expiration.unix_seconds())
        .flatten()
        .filter(|_| days_left.is_some())
        .filter(|at_unix| {
            let days = (at_unix - crate::util::unix_ms_i64_of(now).div_euclid(1_000)) / 86_400;
            SANE_DAYS.contains(&days)
        });
    ApiKeyExpiry {
        unlimited,
        known,
        days_left,
        at_unix,
        checked_ms,
    }
}

/// Decide the day count and the unlimited flag from the three facts one answer carries.
///
/// Split out as a pure function because the shape that matters most cannot be built through
/// moonproto's public API: an answer with NO usable date but a real count. The parser produces it
/// (it zeroes the date whenever the core's timestamp is unusable, yet still returns the count
/// beside it), and gating the count on the date would render such a key as unlimited.
///
/// Unlimited is the absence of BOTH: no usable date and no count to run down. A zero count with no
/// date is exactly how the wire spells "this key does not expire", so it is not kept as a number.
///
/// Args:
///     known: Whether the answer carries a usable expiration date.
///     reported: The core's own day count, when the answer carries that field.
///     dated_days: Days derived from the date, for a legacy answer with no count field.
///
/// Returns:
///     The day count to retain, and whether the key has no expiration at all.
fn api_days_and_unlimited(
    known: bool,
    reported: Option<i32>,
    dated_days: Option<i64>,
) -> (Option<i32>, bool) {
    // Decided BEFORE the plausibility filter: a core that sent a count said something about this
    // key's lifetime, even if the number is unusable. Deciding after would turn a rejected count
    // into "no expiry" — an infinity glyph built out of a value we threw away.
    let unlimited = !known && reported.is_none_or(|days| days == 0);
    let days_left = reported
        .map(i64::from)
        .or(dated_days)
        .filter(|days| SANE_DAYS.contains(days))
        // A zero on an answer with no date is "no expiry", not "expires today".
        .filter(|days| *days != 0 || known)
        .map(|days| days as i32);
    (days_left, unlimited)
}

/// Plausible remaining lifetime of an exchange API key, in days.
///
/// Asymmetric on purpose. Forward, a century covers any real key. Backward, only a quarter: a core
/// that is CONNECTED AND TRADING cannot be running on a key that expired years ago, so a large
/// negative count is not a fact about the key. Observed live: two connected cores answer `-1000`
/// while every other core on the same terminal reports "no expiry" — a core-side placeholder whose
/// meaning is not documented in the protocol. Rendering it as "expired" would put a red warning on
/// two healthy cores, which is worse than admitting the answer is unusable.
const SANE_DAYS: std::ops::RangeInclusive<i64> = -90..=36_500;

/// Applies a targeted toolbar edit to the retained settings snapshot THROUGH command helpers.
/// Raw fields `s_price`/`sb_num` are `pub(crate)` in production; `price_drop_level` is public.
pub(super) fn apply_client_settings_edit(
    s: &mut moonproto::ClientSettingsCommand,
    edit: ClientSettingsEdit,
) {
    match edit {
        ClientSettingsEdit::TakeProfit { pct, extended } => {
            // x_tmode/"s9": on -> x_sell stores pct/10 (displayed as 100..900%); off -> x_sell
            // stores pct directly (1..100%). The core clamps TP to 100 without this flag, so set
            // both fields.
            let mode = if extended {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_take_profit_pct(pct) else {
                return;
            };
            s.fixed_sell_mode = false;
            if extended {
                s.x_tmode = true;
                s.x_sell = (pct / 10.0) as i32;
            } else {
                s.x_tmode = false;
                s.x_sell = pct as i32;
            }
        }
        ClientSettingsEdit::StopLossPct(pct) => {
            let Some(pct) = crate::config::GroupExitSettings::canonical_stop_loss_pct(pct) else {
                return;
            };
            s.price_drop_level = pct;
        }
        ClientSettingsEdit::ScalpTakeProfit(pct) => {
            let Some(pct) = TakeProfitMode::Scalp.canonical_take_profit_pct(pct) else {
                return;
            };
            // Fixed-sell preset encoding also reads x_tmode, so scalp must clear a previous
            // extended generation before the group serializer writes S1-S6.
            s.x_tmode = false;
            s.set_scalp_take_profit_percent(pct);
        }
        ClientSettingsEdit::SelectFixedSellSlot(slot) => {
            // Enable fixed-sell mode; otherwise the effective TP remains on x_sell and does not
            // change. This makes effective_take_profit_percent() equal the selected preset, while
            // the toolbar deliberately continues to show the main TP and the S-button shows the
            // selected fixed-sell value.
            s.fixed_sell_mode = true;
            s.set_selected_fixed_sell_slot(slot);
        }
        ClientSettingsEdit::EngageMainTakeProfit => {
            // Return to the main TP by disabling fixed-sell without changing the TP value (x_sell/scalp).
            s.fixed_sell_mode = false;
        }
        ClientSettingsEdit::SetFixedSellPct { slot, pct } => {
            // Displayed percentage = s_price * (x_tmode ? 10 : 1); derive s_price inversely.
            let mode = if s.x_tmode {
                TakeProfitMode::Extended
            } else {
                TakeProfitMode::Normal
            };
            let Some(pct) = mode.canonical_fixed_sell_pct(pct) else {
                return;
            };
            let price = if s.x_tmode {
                (pct / 10.0) as f32
            } else {
                pct as f32
            };
            s.set_fixed_sell_preset_price(slot, price);
        }
        // Core behavior defaults still owned by the compact channel. The exit rules, icebergs and
        // blacklist moved to the safe-share channel with the settings popup's General tab, which is
        // the only place that edited them; `emu_mode` stayed because that channel is the ONLY one
        // carrying it (see `shared_config::absorb`).
        ClientSettingsEdit::UseStopMarket(on) => s.use_stop_market = on,
        ClientSettingsEdit::PanicIfPriceDrop(on) => s.panic_if_price_drop = on,
        ClientSettingsEdit::SignOrders(on) => s.sign_orders = on,
        ClientSettingsEdit::EmuMode(on) => s.emu_mode = on,
    }
}

/// Builds the chart's server-provided trace from the core's own polyline.
///
/// `points[0]` opens the trace at the leg's RAW `create_time` (moonproto
/// `state/orders/apply_helpers.rs`), while every later point (`points[1..]`) is corrected there
/// against `ServerTimeDelta`. That asymmetry now matters on our side too:
/// `session::clock_skew::CoreClockSkew::correct` shifts ONLY `points[0]` for exactly this reason —
/// shifting an already-corrected `points[1..]` again would double-correct it.
fn order_trace(line: &OrderTraceLine) -> Option<OrderTrace> {
    let points: Vec<OrderTracePoint> = line.points.iter().copied().map(trace_point).collect();
    if !points.iter().any(valid_trace_point) {
        return None;
    }
    Some(OrderTrace {
        points,
        tmp_point: line.tmp_point.and_then(valid_trace_tmp_point),
        stop_price: line.stop_price.filter(|p| p.is_finite() && *p > 0.0),
        stop_time_ms: line
            .stop_time
            .map(|time| time.unix_millis() as f64)
            .filter(|time_ms| *time_ms > 1.0),
    })
}

/// Resolve the chart price of an order's stop-loss line from the wire field `sl_level`.
///
/// `sl_level` carries two different quantities depending on who wrote it last, and the wire does
/// not say which. When the CORE applies the stop it stores the resolved trigger PRICE — its log
/// reads `StopLoss applied (buyPrice 0.89100 stop -3.00% => 0.91856)`, and that 0.91856 is what
/// arrives. When the stop is set by a COMMAND — the terminal enabling it, which sends
/// `with_stop_loss_percent` — the field keeps the PERCENT, and the core is content to hold it
/// (`OrderCommand op=3 … applied`, no recalculation) until something makes it apply the stop.
///
/// Drawing a percent as a price is what put a stop-loss 10.65 on a 0.00010458 chart: a line at
/// `+10183491%` that also dragged the auto-Y range with it, flattening the whole pane
/// (`10kSATS`, BB1, 2026-08-11 17:18). So the value is read against the entry before it is
/// believed, and a percent is converted the way the core would, `entry * (1 ∓ p/100)`.
///
/// Args:
///     entry: Order entry price; `0` or non-finite when the order has none yet.
///     is_short: Whether the position is short, which flips the stop's side.
///     fixed: The wire's `sl_fixed` flag — an absolute price the trader chose.
///     level: The wire's `sl_level`, either a price or a percent.
///
/// Returns:
///     Chart price for the stop line, or `None` when nothing usable can be derived.
fn stop_loss_line_price(
    entry: f64,
    is_short: bool,
    fixed: bool,
    level: f64,
    entry_filled: bool,
) -> Option<f64> {
    if !level.is_finite() || level <= 0.0 {
        return None;
    }
    // A fixed level is an absolute price the trader chose, and may sit on either side on purpose.
    // With no entry to compare against there is nothing to reason with either, so the reported value
    // is drawn as-is rather than dropping the line of a stop that IS enabled.
    if fixed || !entry.is_finite() || entry <= 0.0 {
        return Some(level);
    }
    // Before the fill the field is ALWAYS a percent, with no guessing involved: the core resolves a
    // stop into a price at the fill and not before. Every unfilled order carrying a stop in the
    // 2026-08-11 diagnostic held its plain percentage there (10.65 against entries from 0.0049 to
    // 0.22), so an unfilled order needs no plausibility test at all.
    if !entry_filled {
        return pct_level_price(entry, is_short, level);
    }
    // The side of the entry is what separates the two meanings, not the distance: a percentage stop
    // protects the position, so its price is BELOW a long's entry and ABOVE a short's. A percent
    // that lands on the protected side cannot be that price. The distance still matters at the far
    // end, where a percent can fall on the correct side by coincidence — 10.65 under a $1000 entry
    // reads as a price 99% away, which no stop is. A twentieth of the entry is the cut: it leaves
    // room for a -90% catastrophe stop while excluding a two-digit percent on any market priced
    // above a few hundred.
    let price_side = if is_short {
        level > entry && level <= entry * 20.0
    } else {
        level < entry && level >= entry / 20.0
    };
    if !price_side {
        // The core quirk from the 2026-07-17 report: a SHORT's percentage stop can arrive already
        // resolved but computed with the long formula, ending up just below the entry, on the
        // profit side. Close to the entry it is that price, mirrored back; far below it is a
        // percent.
        if is_short && level >= entry / 2.0 && level < entry {
            return Some(2.0 * entry - level);
        }
        return pct_level_price(entry, is_short, level);
    }
    Some(level)
}

/// Convert a percentage stop level into the price the core would resolve it to.
///
/// Args:
///     entry: Order entry price, already validated as finite and positive.
///     is_short: Whether the position is short, which puts the stop above the entry.
///     level: Percentage distance, always positive on the wire.
///
/// Returns:
///     Stop price, or `None` when the percentage leaves nothing above zero.
fn pct_level_price(entry: f64, is_short: bool, level: f64) -> Option<f64> {
    let price = if is_short {
        entry * (1.0 + level / 100.0)
    } else {
        entry * (1.0 - level / 100.0)
    };
    (price.is_finite() && price > 0.0).then_some(price)
}

/// Project one retained MoonProto order into a UI row.
///
/// This conversion has no report-database side effects; protocol-v4 reports are
/// replicated independently through `Event::Report`.
fn build_order_row(server_id: u64, snap: &moonproto::MoonStateSnapshot, o: &Order) -> OrderRow {
    // Display name for the market. Hyperliquid spot names pairs by INDEX ("@206"); moonproto
    // provides the human-readable name in `market_name_mb_classic` ("UENAUSDT"). Store the classic
    // name in the order/report so `coin_of_market` yields "UENA", not "@206". Gate this on the
    // "@" prefix so ordinary markets such as BTCUSDT remain unchanged. INTERNAL catalog lookups
    // below (price/snapshot/liquidation) keep the RAW `o.market_name`, which the core uses as its
    // key. Fall back to the raw name when mb_classic is empty or also starts with "@".
    let indexed = o.market_name.starts_with('@');
    let catalog = snap.markets().get(&o.market_name).map(|h| {
        h.with(|m| {
            // `market_currency`, NOT `market_currency_canonic`. The two answer different
            // questions, measured on live cores: for Bybit's `1000BONKPERP` the catalog holds
            // currency=`1kBONKPERP` / canonic=`BONKPERP`, and for COIN-M's `AAVEUSD_PERP`
            // currency=`AAVE_RP` / canonic=`AAVE`. The core matches its coin lists against
            // `market_currency` by exact text, and the report writes that same spelling, so it is
            // the token to show and to write back. `canonic` is the contract-free WALLET identity,
            // which is why `feed::assets` deliberately prefers it when deduplicating holdings.
            let coin = m.market_currency.trim();
            // The classic spelling is only ever needed for an indexed name, so an ordinary market
            // does not pay for a String it would discard.
            let classic = indexed.then(|| m.market_name_mb_classic.clone());
            (
                classic,
                coin.to_string(),
                m.base_currency.trim().to_ascii_uppercase(),
            )
        })
    });
    let market_display = if indexed {
        catalog
            .as_ref()
            .and_then(|(classic, ..)| classic.clone())
            .filter(|s| !s.is_empty() && !s.starts_with('@'))
            .unwrap_or_else(|| o.market_name.clone())
    } else {
        o.market_name.clone()
    };
    // `strat` is the strategy TYPE (kind), `strat_name` its user-assigned `StrategyName`. Both come
    // from one snapshot lookup; a manual order or an unknown snapshot leaves the name empty.
    //
    // These are captured at row-build time. `strat` (kind) is immutable for a strategy's lifetime,
    // but `StrategyName` is user-editable, so a rename does not refresh the Orders panel's Name
    // column until that core's next order event bumps `orders_table_rev`. This is deliberate: the
    // panel signature keys on order revisions, not strategy revisions, to avoid rebuilding the table
    // on every unrelated strategy edit. The stale name self-heals on the next order tick.
    let (strat, strat_name) = match snap.strats().snapshot(o.strat_id) {
        Some(s) => (
            strat_kind_name(s.kind().ordinal()).to_string(),
            s.strategy_name().unwrap_or_default().to_string(),
        ),
        None => (o.strat_id.to_string(), String::new()),
    };
    // The entry leg is ALWAYS `buy_order`, for both longs and shorts. The state machine is phased:
    // entry is `Buy*`, exit is `Sell*`; a short's entry also lives in buy_order/buy_price, while
    // sell_order is the empty exit leg. The old short path used sell_order and produced fill_pct=0.
    let leg = &o.buy_order;
    let fill_pct = if leg.quantity > 0.0 {
        ((leg.quantity - leg.quantity_remaining) / leg.quantity * 100.0) as f32
    } else {
        0.0
    };
    let pick = |qb: f64, q: f64, qr: f64| {
        if qb != 0.0 {
            qb
        } else if q != 0.0 {
            q
        } else {
            qr
        }
    };
    let bs = pick(
        o.buy_order.quantity_base,
        o.buy_order.quantity,
        o.buy_order.quantity_remaining,
    );
    let ss = pick(
        o.sell_order.quantity_base,
        o.sell_order.quantity,
        o.sell_order.quantity_remaining,
    );
    let raw_size = if bs.abs() >= ss.abs() { bs } else { ss };

    let mkt = snap.markets().price(&o.market_name);
    let last = mkt.as_ref().map(|p| p.p_last as f32).unwrap_or(0.0);
    // Entry price for the entry line and stop/take-profit level calculations.
    //
    // IMPORTANT (the "line above the actual buy" bug): after execution the core stores the
    // BREAK-EVEN price — the fill plus round-trip commission, some 0.03–0.1% off — in BOTH
    // `buy_price` and `buy_order.actual_price`. The leg's `mean_price` is the raw average fill, and
    // that is what the core itself measures from: a stop it resolved on a filled short came out at
    // exactly -0.200000% of `mean_price` and at -0.240% of `buy_price` (order diagnostic, BB1
    // 2026-08-11). Measuring from the break-even price is what made every stop and take-profit
    // label read a few hundredths of a percent off what Moonbot shows for the same order.
    //
    // `mean_price` also tracks a WORKING order's cancel-replace, so it stays correct before the
    // fill too — the fallbacks below only cover an order so fresh it has no fill data at all.
    //
    // This deliberately no longer consults the market's average POSITION price. That is a per-COIN
    // number shared by every order on it, and it never arrives anyway: `pos_price` was zero in all
    // 56060 diagnostic samples across 21 cores, since the balance record only carries it when the
    // core sets its flag.
    let mkt_snapshot = snap.markets().get(&o.market_name).map(|h| h.snapshot());
    let contract_size = mkt_snapshot
        .as_ref()
        .map(|m| m.contract_size())
        .unwrap_or(1.0);
    // "In position" means holding a position for which PnL and lines are rendered. The signal is
    // the authoritative moonproto worker PHASE, not an inference from `sell_order.quantity`:
    // - `fill_pct > 0` means the ENTRY leg (`buy_order`) has at least some fill. This covers every
    //   case with a buy leg: partial entry (BuySet while filling), BuyDone, and short entry (which
    //   also uses buy_order). A partially filled entry already holds part of the position, and PnL
    //   for that part is valid because quantity comes from the remaining exit leg.
    // - `status == SellSet` means the exit/take-profit IS PLACED. The only case fill_pct misses is
    //   selling an already-held spot asset (listing-sell/MoonHook): there is no buy leg, so
    //   fill_pct=0, but the order is in the Sell phase. Do NOT trigger on BuySet: the entry is still
    //   waiting and fill_pct=0 means there is no position, so PnL/lines correctly stay absent until
    //   the first fill.
    // The sell line is additionally gated by `sell_price > 0` below, so it cannot be drawn too
    // early: until the core sets a sell price, there is no exit line even when `in_position`.
    let in_position = crate::feed::order_holds_position(o);
    // moonproto updates the top-level `o.buy_price`/`o.sell_price` ONLY when the worker status
    // changes or on a LOCAL move_order. Server-side cancel-replace (the core follows the market
    // with its limit order) changes only `*_order.actual_price`. Therefore, the line price for a
    // WORKING (unfilled) order is the leg's `actual_price`; otherwise the line freezes at its
    // placement price. In the 2026-07-17 report, a short buy stayed where the order book had long
    // since moved and was "fixed" only by dragging it, which locally rewrites buy_price.
    let live = |p: f64| (p.is_finite() && p > 0.0).then_some(p);
    let entry = live(o.buy_order.mean_price)
        .or_else(|| live(o.buy_order.actual_price))
        .unwrap_or(o.buy_price);
    let valid_entry = entry.is_finite() && entry > 0.0;

    // Coin-margined (inverse) futures report quantity in CONTRACTS rather than the base coin
    // (`contract_size != 1`; contract notional is fixed in quote/USD, e.g. BTCUSD = $100 and other
    // *USD contracts = $10). Actual coin size = contracts * contract_size / entry_price. This lets
    // both the size label (coins) and its USD notional (coins * price = contracts * cs) use the
    // shared formula correctly. `contract_size == 1` means linear/spot and size is already in coins.
    //
    // IMPORTANT: `contract_size != 1` alone does NOT mean inverse. For LINEAR quanto futures on
    // Gate (ASTEROID_USDT: cs=10000 coins/contract), the core already reports legs IN COINS;
    // dividing by the tiny price inflated quantity to 7e14 and PnL to -71 million for an actual
    // -$1 result. A true coin-margined contract has an EMPTY QUOTE CURRENCY because the contract is
    // denominated in USD; `build_assets` distinguishes it the same way. Convert only in that case.
    let quote_is_empty = mkt_snapshot
        .as_ref()
        .is_some_and(|m| m.base_currency.trim().is_empty());
    let convert_contract_qty = |qty: f64, price: f64| {
        if quote_is_empty
            && contract_size != 1.0
            && contract_size > 0.0
            && price.is_finite()
            && price > 0.0
        {
            qty * contract_size / price
        } else {
            qty
        }
    };
    let size = if valid_entry {
        convert_contract_qty(raw_size, entry)
    } else {
        raw_size
    };
    // The inbound half of the contracts<->coins rule, printed for the followed markets only: an
    // order whose size is NOT converted reaches the chart label as a contract count multiplied by
    // a coin price, which is the same figure wrong by the contract size — $4.99K where the order
    // is $500. The three inputs that decide it are invisible from outside otherwise.
    if crate::order_diag::follows(
        &crate::feed::core_label(server_id).to_string(),
        &o.market_name,
    ) {
        crate::order_diag::line(&format!(
            "core {} uid={} market={} size {raw_size} -> {size} (quote_is_empty={quote_is_empty}, \
             contract_size={contract_size}, entry={entry}, valid_entry={valid_entry})",
            crate::feed::core_label(server_id),
            o.uid,
            o.market_name,
        ));
    }
    let sell_remaining_raw = if o.sell_order.quantity_remaining != 0.0 {
        o.sell_order.quantity_remaining
    } else if ss != 0.0 {
        ss
    } else {
        raw_size
    };
    let remaining_price = if o.sell_price.is_finite() && o.sell_price > 0.0 {
        o.sell_price
    } else {
        entry
    };
    let remaining_size = if sell_remaining_raw.is_finite() && sell_remaining_raw > 0.0 {
        convert_contract_qty(sell_remaining_raw, remaining_price)
    } else {
        size
    };
    let fin = |v: f64| (v.is_finite() && v > 0.0).then_some(v);
    let pct_stop = |level: f64| {
        if o.is_short {
            entry * (1.0 + level / 100.0)
        } else {
            entry * (1.0 - level / 100.0)
        }
    };
    // Trailing-stop calculation: a fixed value is an absolute level; otherwise, when entry is
    // valid, derive a percentage level from entry. Disabled or missing-entry cases return `None`.
    let stop = |enabled: bool, fixed: bool, level: f64| {
        if !enabled {
            None
        } else if fixed {
            fin(level)
        } else if valid_entry {
            fin(pct_stop(level))
        } else {
            None
        }
    };
    let stop_loss = o
        .stops
        .stop_loss_enabled()
        .then(|| {
            stop_loss_line_price(
                entry,
                o.is_short,
                o.stops.stop_loss_fixed(),
                o.stops.stop_loss_level(),
                crate::feed::order_entry_filled(o),
            )
        })
        .flatten();
    let trailing = stop(
        o.stops.trailing_enabled(),
        o.stops.trailing_fixed(),
        o.stops.trailing_level(),
    );
    let take_profit = o
        .stops
        .take_profit_enabled()
        .then(|| fin(o.stops.take_profit()))
        .flatten();
    let vstop = o.vstop_on.then(|| fin(o.vstop_level)).flatten();
    let pending_cond = o.pending_buy_cond_price.and_then(fin);
    let liq = snap.markets().get(&o.market_name).and_then(|h| {
        let bp = h.balance_position();
        let v = if o.is_short {
            bp.short_liq_price
        } else {
            bp.long_liq_price
        };
        fin(v).or_else(|| fin(bp.liq_price))
    });
    let pending = o.pending_buy_cond_price.is_some();
    // `filled`, which means a position is held and gates the sell line, stops, TP, liquidation,
    // and PnL, equals `in_position` — one reading of `order_holds_position`, shared with the packet
    // assembly so that "the core owns this order's stops" cannot mean one thing on screen and
    // another inside a command. Without it, a sale from an already-held asset (no buy leg at all)
    // showed neither lines nor PnL even though Moonbot displayed both.
    let filled = in_position;
    let create_time_ms = moon_time_to_unix_millis_f64(o.buy_order.create_time());
    // Line starts that only the WIRE knows. The retained line store can otherwise date a line no
    // earlier than the first snapshot this process received, which is the terminal's launch for
    // every position that predates it. Both legs carry their own times in the canonical PLACEMENT
    // sections, so they arrive with the very first snapshot after a restart.
    //
    // These are absolute UTC milliseconds off the wire (`i64` into `MoonTime::from_unix_millis`),
    // but that does NOT mean they need no correction: they are read off the CORE's own clock, and a
    // core running ahead of or behind true UTC ships them raw. moonproto's own `ServerTimeDelta`
    // correction cannot reach this path at all — its accessor is `#[cfg(test)] pub(crate)` — so
    // `session::clock_skew::CoreClockSkew` estimates the same skew from these very fields
    // (`create_time_ms`, `sell_create_time_ms`, `entry_fill_time_ms`) and corrects them once they
    // reach `CoreData::apply`, before the retained line store or the chart ever sees them raw.
    let sell_create_time_ms = moon_time_to_unix_millis_f64(o.sell_order.create_time());
    let entry_fill_time_ms = moon_time_to_unix_millis_f64(o.buy_order.close_time());
    // EFFECTIVE stop flags: the order's own flag from the wire, plus — only while the order has no
    // position — the stop its strategy is going to give it.
    //
    // `stops` carries the core's own state for this order: moonproto applies section OSEC_STOPS
    // straight into it, and that section sits in `ORDER_RECONCILE_MASK`, so the library keeps it
    // repaired against the core's catalog rather than leaving it at whatever last arrived.
    //
    // The position is what divides the two readings. Moonbot materializes a stop INTO the order
    // when the entry fills — the core logs `StopLoss applied (buyPrice … => …)` from
    // `BOrderWorker.CheckBuyOrder`, taking the level from the strategy or, failing that, from the
    // general settings (`StopLoss taken from general settings`). Before that the order holds no stop
    // and the strategy's flag is the honest answer; after it, the order's own flag is, and switching
    // a stop off is a state of the ORDER which the core keeps and reports. Inheriting past the fill
    // is what redrew a hand-disabled SL as ON one frame after the strategy snapshot arrived, and
    // again on every restart. The same rule gates the outgoing StopSettings in `feed::trade`, so a
    // row and a packet cannot disagree about who owns this order's stops.
    //
    // An explicit terminal click still wins over an INHERITED stop: the wire cannot spell "the
    // trader declined the stop his strategy is about to give this order", so without the override
    // switching a working order's stop off would redraw as ON on the very next frame. The override
    // expires as soon as the wire disagrees, so a stop the core arms anyway comes back on its own.
    let stop_eff = |kind: crate::feed::OrderStopKind, wire: bool, strat_on: bool| -> bool {
        crate::feed::trade::stop_override(server_id, o.uid, kind, wire).unwrap_or(
            wire || crate::feed::stop_inherited_from_strategy(
                crate::feed::order_entry_filled(o),
                strat_on,
            ),
        )
    };
    let eff_strat_id = crate::feed::strategies::effective_strat_id(snap, o.strat_id);
    let strat_snapshot = snap.strats().snapshot(eff_strat_id);
    let strat_schema = snap.strats().strategy_schema();
    // The strategy serializer (mirrored in Delphi and moonproto) DOES NOT send fields whose value
    // equals the SCHEMA DEFAULT, because the writer skips defaults. A missing snapshot field means
    // "equals the schema default", NOT "disabled", so fall back to the schema's own default value.
    // Field names are Moonbot Delphi names, confirmed against strings in MoonBot.exe.
    let strat_flag = |name: &str| -> bool {
        let Some(s) = strat_snapshot else {
            return false;
        };
        if let Some(v) = s.fields.get_bool(name) {
            return v;
        }
        strat_schema
            .and_then(|sc| sc.field(name))
            .and_then(|f| f.default_value.as_ref())
            .is_some_and(|v| matches!(v, moonproto::FieldValue::Bool(true)))
    };
    // The effective strategy is the order's own or the core settings' manual strategy, which governs
    // manual orders with strat_id=0. A manual order with no strategy snapshot falls back to the
    // ClientSettings stop defaults, whose "drop" percentages are NEGATIVE (price_drop_level=-1.1
    // means SL 1.1%), so nonzero means enabled — not "> 0".
    let (sl_strat, ts_strat, vstop_strat) = if strat_snapshot.is_some() {
        (
            strat_flag("UseStopLoss"),
            strat_flag("UseTrailing"),
            strat_flag("UseBV_SV_Stop"),
        )
    } else if o.strat_id == 0 {
        snap.settings()
            .client_settings
            .as_ref()
            .map(|c| {
                (
                    c.price_drop_level != 0.0,
                    c.trailing_drop != 0.0,
                    c.vol_drop_level != 0,
                )
            })
            .unwrap_or((false, false, false))
    } else {
        (false, false, false)
    };
    // The coin token, from the CATALOG. It is not a display convenience: it is what the coin
    // context menu writes into the core's and the strategy's blacklists, and the core matches
    // those against this very field. `market_currency_canonic`/`market_currency` also carry the
    // core's own foldings — `1000BONKPERP` is reported as `1kBONKPERP`, `AAVEUSD_PERP` as
    // `AAVE_RP` — which no rule applied to the market name can reconstruct. `feed::assets` reads
    // the same pair of fields for the same reason.
    //
    // The name-based rules answer only when the catalog does not hold this market: a market
    // delisted while an order is still open. There the exchange's rules apply to the EXCHANGE's
    // spelling, so to the raw `market_name` — except for a Hyperliquid spot index, whose raw name
    // (`@206`) carries no coin at all while the classic display spelling (`UENAUSDT`) does. With
    // no catalog entry there is no classic spelling either, so an index falls back to itself:
    // nothing outside the catalog can say which coin `@206` is.
    let (coin, quote) = match catalog {
        Some((_, coin, quote)) if !coin.is_empty() => {
            // A COIN-M contract reports no base currency, so its quote comes from the name
            // (`BTCUSD_PERP` → `USD`); otherwise the order dialog would show a bare coin.
            let quote = if quote.is_empty() {
                crate::symbol::parse::split_market(&o.market_name, exchange_of(snap))
                    .quote
                    .to_ascii_uppercase()
            } else {
                quote
            };
            (coin, quote)
        }
        _ => {
            let parts = crate::symbol::parse::split_market(&o.market_name, exchange_of(snap));
            let coin = if parts.is_index() {
                crate::symbol::coin_of_market(&market_display).to_string()
            } else {
                parts.base.to_string()
            };
            (coin, parts.quote.to_ascii_uppercase())
        }
    };
    OrderRow {
        // The data KEY is the raw name used by the core; display uses the resolved mb_classic name.
        market: o.market_name.clone(),
        market_display,
        coin,
        quote,
        is_short: o.is_short,
        size,
        remaining_size,
        sl_on: stop_eff(
            crate::feed::OrderStopKind::StopLoss,
            o.stops.stop_loss_enabled(),
            sl_strat,
        ),
        ts_on: stop_eff(
            crate::feed::OrderStopKind::Trailing,
            o.stops.trailing_enabled(),
            ts_strat,
        ),
        vstop_on: stop_eff(crate::feed::OrderStopKind::VStop, o.vstop_on, vstop_strat),
        sl_fixed: o.stops.stop_loss_fixed(),
        ts_fixed: o.stops.trailing_fixed(),
        vstop_fixed: o.vstop_fixed,
        vstop_level: o.vstop_level,
        vstop_vol: o.vstop_vol,
        buy_price: entry,
        // As with entry, the live price of a WORKING sell order is `sell_order.actual_price`;
        // server-side take-profit moves do not update `o.sell_price` until status changes.
        sell_price: if o.sell_order.actual_price.is_finite() && o.sell_order.actual_price > 0.0 {
            o.sell_order.actual_price
        } else {
            o.sell_price
        },
        create_time_ms,
        sell_create_time_ms,
        entry_fill_time_ms,
        price: last,
        fill_pct,
        strat,
        strat_name,
        strat_id: o.strat_id,
        status: o.status.name().to_string(),
        uid: o.uid,
        emulator: o.emulator_mode,
        job_is_done: o.job_is_done,
        pending,
        filled,
        stop_loss,
        trailing,
        take_profit,
        vstop,
        pending_cond,
        liq,
        panic_sell: o.panic_sell,
        is_moon_shot: o.is_moon_shot,
        corridor_price_down: o.corridor_price_down,
        corridor_price_up: o.corridor_price_up,
        buy_trace: o.buy_trace_line.as_ref().and_then(order_trace),
        sell_trace: o.sell_trace_line.as_ref().and_then(order_trace),
    }
}

/// The naming family of the core this snapshot belongs to.
///
/// `BaseCheck` reports the exchange ordinal; before it arrives the snapshot has none and the
/// parser recognizes the name's shape instead, which lands on the same coin for every market
/// these cores actually list.
fn exchange_of(snap: &moonproto::MoonStateSnapshot) -> crate::symbol::Exchange {
    snap.server_info()
        .exchange_code
        .map(|code| crate::symbol::Exchange::from_code(code.stable_id()))
        .unwrap_or_default()
}

/// Build the current order rows and overlay captured order events from this drain.
///
/// Event rows preserve terminal states that may disappear from the retained
/// snapshot before the application processes the event queue.
/// Reports one order batch to the order channel, and only when it changed.
///
/// Answers the question static reading cannot: an order the core says it published either reaches
/// `snapshot().orders()` or it does not, and the moonproto client drops exactly one class silently —
/// one whose MARKET it has not seen yet stays parked with no log line. So this prints both sides:
/// which followed markets the catalog holds, and which orders on them actually arrived. A followed
/// market present with zero orders on it is the parked case; orders present here while the panels
/// show none puts the loss in our own UI.
///
/// Gated by a per-core signature so an idle terminal writes nothing at the table's 4 Hz.
fn diag_order_batch(server_id: u64, snap: &moonproto::MoonStateSnapshot, rows: &[OrderRow]) {
    if !crate::order_diag::enabled() {
        return;
    }
    let core = crate::feed::core_label(server_id).to_string();
    let markets: Vec<String> = snap
        .markets()
        .iter()
        .map(|h| h.name().to_string())
        .filter(|n| crate::order_diag::follows(&core, n))
        .collect();
    if markets.is_empty() && !crate::order_diag::follows(&core, "") {
        return;
    }
    let matched: Vec<&OrderRow> = rows
        .iter()
        .filter(|r| crate::order_diag::follows(&core, &r.market))
        .collect();
    let mut sig = std::collections::hash_map::DefaultHasher::new();
    {
        use std::hash::Hash;
        rows.len().hash(&mut sig);
        markets.hash(&mut sig);
        for r in &matched {
            (
                r.uid,
                &r.status,
                r.pending,
                r.pending_cond.map(f64::to_bits),
            )
                .hash(&mut sig);
        }
    }
    let sig = {
        use std::hash::Hasher;
        sig.finish()
    };
    static LAST: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, u64>>> =
        std::sync::OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().unwrap();
    if last.insert(server_id, sig) == Some(sig) {
        return;
    }
    drop(last);
    crate::order_diag::line(&format!(
        "core {} orders={} followed_markets={:?} matched_orders={}",
        crate::feed::core_label(server_id),
        rows.len(),
        markets,
        matched.len()
    ));
    for r in matched {
        // The three leg times decide where the chart STARTS each line. A zero here is the whole
        // reason an exit or a stop would date itself to the terminal's launch instead of to the
        // trade, so print them beside the state that produced them.
        crate::order_diag::line(&format!(
            "core {} uid={} market={} status={} pending={} cond={:?} buy={} filled={} short={} \
             size={} remaining={} t_create={} t_sell_create={} t_entry_fill={}",
            crate::feed::core_label(server_id),
            r.uid,
            r.market,
            r.status,
            r.pending,
            r.pending_cond,
            r.buy_price,
            r.filled,
            r.is_short,
            // The size the order came back with, beside the one that was sent: on a coin-margined
            // market these two are in different units (contracts out, coins back through
            // `convert_contract_qty`), and comparing them is the only way to establish from
            // outside what the core did with the number.
            r.size,
            r.remaining_size,
            r.create_time_ms,
            r.sell_create_time_ms,
            r.entry_fill_time_ms
        ));
        diag_order_prices(server_id, snap, r);
    }
}

/// Reports the candidate entry prices of one order, with the stop distance each of them implies.
///
/// "Why does the chart label say -5.80% where Moonbot says -6.0%?" is a question about the BASE the
/// percentage is measured from, and the wire offers four of them: the market's average position
/// price (shared by every order on that coin), the order's `buy_price` and its leg's `actual_price`
/// (both carrying the core's break-even markup after a fill), and the leg's own `mean_price`. The
/// terminal currently draws from the first; Moonbot's own log measures from the order's `buyPrice`
/// (`StopLoss applied (buyPrice 1684.90 stop -3.50% => 1625.93)`).
///
/// Printing all four beside the resulting percentages settles which one this build should use
/// instead of arguing from the field names. Written for the 2026-08-11 stop-label mismatch.
///
/// Args:
///     server_id: Core the order belongs to, for the log prefix.
///     snap: Live snapshot holding the raw order and its market.
///     row: Projected row whose `stop_loss`/`take_profit` prices are being explained.
fn diag_order_prices(server_id: u64, snap: &moonproto::MoonStateSnapshot, row: &OrderRow) {
    // Only orders that actually carry a protective level have a base to argue about, and skipping
    // the rest keeps a `channels.orders = "1"` run over twenty cores down to the lines worth reading.
    if row.stop_loss.is_none() && row.take_profit.is_none() {
        return;
    }
    let Some(o) = snap.orders().iter().find(|o| o.uid == row.uid) else {
        return;
    };
    let pos_price = snap
        .markets()
        .get(&o.market_name)
        .map(|h| h.snapshot().pos_price)
        .unwrap_or(0.0);
    let leg = &o.buy_order;
    let pct = |level: Option<f64>, base: f64| stop_distance_pct(level, base, row.is_short);
    crate::order_diag::line(&format!(
        "core {} uid={} prices pos={pos_price} buy={} actual={} mean={} sell_mean={} fill={}% \
         stop={:?} tp={:?} | stop%: pos={:?} buy={:?} actual={:?} mean={:?}",
        crate::feed::core_label(server_id),
        row.uid,
        o.buy_price,
        leg.actual_price,
        leg.mean_price,
        o.sell_order.mean_price,
        row.fill_pct,
        row.stop_loss,
        row.take_profit,
        pct(row.stop_loss, pos_price),
        pct(row.stop_loss, o.buy_price),
        pct(row.stop_loss, leg.actual_price),
        pct(row.stop_loss, leg.mean_price),
    ));
}

/// Signed distance from `base` to a stop level, in percent, with the order's side applied.
///
/// Mirrors the chart label's own `signed_pct` so the diagnostic prints the number the chart would
/// print if it measured from that base: a long's stop below entry reads negative, and a short's
/// stop above entry reads negative too.
///
/// Args:
///     level: Stop price, when the order has one.
///     base: Candidate entry price to measure from.
///     is_short: Whether the position is short, which flips the sign.
///
/// Returns:
///     Percentage distance, or `None` without a usable level or base.
fn stop_distance_pct(level: Option<f64>, base: f64, is_short: bool) -> Option<f64> {
    let level = level?;
    if !base.is_finite() || base <= 0.0 {
        return None;
    }
    let raw = (level - base) / base * 100.0;
    Some(if is_short { -raw } else { raw })
}

pub(super) fn build_order_rows(
    server_id: u64,
    snap: &moonproto::MoonStateSnapshot,
    events: &[Event],
) -> Vec<OrderRow> {
    let mut order_rows = Vec::new();
    for o in snap.orders().iter() {
        order_rows.push(build_order_row(server_id, snap, o));
    }

    // Snapshot is the live view. Terminal statuses can be removed from that view
    // before the app drains the event queue, so MoonProto carries an Arc<Order>
    // on Created/Updated/Removed. Overlay those captured rows onto the full live
    // snapshot: OrderLineStore still receives a complete seen-set, while closed
    // rows cannot vanish into BackstopMissing.
    for ev in events {
        let Event::Order(order_event) = ev else {
            continue;
        };
        let Some(order) = order_event.order() else {
            continue;
        };
        let row = build_order_row(server_id, snap, order);
        if let Some(existing) = order_rows.iter_mut().find(|r| r.uid == row.uid) {
            *existing = row;
        } else {
            order_rows.push(row);
        }
    }

    diag_order_batch(server_id, snap, &order_rows);
    order_rows
}

/// Maps moonproto `ExchangeKind` to a domain wallet (the inverse of `assets::to_exchange_kind`).
fn wallet_kind_from_proto(k: moonproto::state::ExchangeKind) -> WalletKind {
    match k {
        moonproto::state::ExchangeKind::Spot => WalletKind::Spot,
        moonproto::state::ExchangeKind::Futures => WalletKind::Futures,
        moonproto::state::ExchangeKind::Quarterly => WalletKind::Quarterly,
    }
}

/// Projects `Event::EngineAction` into a terminal result for UI toasts.
pub(super) fn engine_action_result(e: &moonproto::EngineActionEvent) -> EngineActionResult {
    use moonproto::EngineActionKind as K;
    let kind = match &e.kind {
        K::CancelAllOrders => EngineActionKind::CancelAllOrders,
        K::SetLeverage {
            market,
            new_leverage,
        } => EngineActionKind::SetLeverage {
            market: market.clone(),
            leverage: *new_leverage,
        },
        K::SetHedgeMode { hedge_mode } => EngineActionKind::SetHedgeMode { on: *hedge_mode },
        K::ChangePositionType { market, .. } => EngineActionKind::ChangePositionType {
            market: market.clone(),
        },
        K::ConvertDustBnb => EngineActionKind::ConvertDust,
        K::ConfirmRiskLimit { market } => EngineActionKind::ConfirmRiskLimit {
            market: market.clone(),
        },
        K::SetMaMode { ma_mode } => EngineActionKind::SetMaMode { on: *ma_mode },
        K::TransferAsset {
            asset,
            qty,
            from,
            to,
        } => EngineActionKind::TransferAsset {
            asset: asset.clone(),
            qty: *qty,
            from: wallet_kind_from_proto(*from),
            to: wallet_kind_from_proto(*to),
        },
        K::ReloadOrderBook => EngineActionKind::ReloadOrderBook,
    };
    EngineActionResult {
        kind,
        success: e.success,
        error_code: e.error_code,
        error_msg: e.error_msg.clone(),
    }
}

#[cfg(test)]
mod tests;
