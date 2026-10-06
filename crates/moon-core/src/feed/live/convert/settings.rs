//! Compact settings, runtime and startup snapshot projections.

use super::*;

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
pub(in crate::feed::live) fn client_settings_from_proto(
    c: &moonproto::ClientSettingsCommand,
) -> ClientSettings {
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

pub(in crate::feed::live) fn runtime_state_from_proto(
    s: &moonproto::RuntimeStateCommand,
) -> RuntimeState {
    RuntimeState {
        is_started: s.is_started,
        auto_detect_active: s.auto_detect_active,
    }
}

/// Project the core's report profit counters shown beside the AutoStart loss caps.
///
/// The two pairs are the core's own hourly and trade-window counters; they come from the report
/// database rather than from balances, so they can legitimately disagree with the header P&L.
///
/// The FIRST wire pair is the hours window and the SECOND the last-N-trades window (#863): a live
/// core showed 103 trades in the first pair beside a trades stop capped at 20, which only the hours
/// window can hold. The Reset buttons already send the kind of the stop they sit under, so only
/// this projection decides which digits each stop shows.
pub(in crate::feed::live) fn profit_state_from_proto(
    s: &moonproto::ProfitStateCommand,
) -> ProfitState {
    ProfitState {
        total_profit: s.rep_trades_total,
        total_trades: s.rep_count_trades,
        hourly_profit: s.rep_total_profit,
        hourly_trades: s.rep_total_trades,
    }
}

/// Reads a retained core snapshot only when the batch contains any event matched by the caller's
/// predicate, including non-settings events such as `KernelHealth`; otherwise returns `None`.
/// Snapshot access is cheap, but unnecessary without a matching event.
pub(in crate::feed::live) fn settings_event_snapshot<T>(
    events: &[Event],
    client: &MoonClient,
    matched: impl Fn(&Event) -> bool,
    extract: impl FnOnce(Arc<moonproto::MoonStateSnapshot>) -> Option<T>,
) -> Option<T> {
    snapshot_when(events.iter().any(matched), client, extract)
}

/// Read the retained snapshot when `gate` says to, and project it.
///
/// The half of [`settings_event_snapshot`] that is not about events, split out for the one caller
/// whose gate is not an event at all: an operator asking for a republish of something the wire will
/// never announce again. Written as a helper rather than copied inline so the snapshot/flatten/
/// project chain has one spelling — nine sibling publish blocks in the live loop reach it through
/// [`settings_event_snapshot`], and a tenth on a hand-rolled copy is how the two drift.
///
/// Args:
///     gate: Whether there is any reason to read the snapshot at all.
///     client: Connected moonproto client holding the retained state.
///     extract: Projects the retained snapshot into the terminal's own type.
///
/// Returns:
///     The projection, or `None` when the gate is shut or no snapshot exists yet.
pub(in crate::feed::live) fn snapshot_when<T>(
    gate: bool,
    client: &MoonClient,
    extract: impl FnOnce(Arc<moonproto::MoonStateSnapshot>) -> Option<T>,
) -> Option<T> {
    gate.then(|| client.snapshot()).flatten().and_then(extract)
}

/// Convert protocol-v4 `KernelHealth` into terminal telemetry and stamp its
/// receipt time. The caller reads `KernelHealth` from `snapshot().kernel_health()`
/// (its retained value keeps the last memory sample between CPU-only Pings), not
/// from the raw event. Process vs system CPU is a SCOPE distinction — each keeps
/// its own field so machine-wide CPU never renders as a process average.
pub(in crate::feed::live) fn sys_status_from_proto(
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
pub(in crate::feed::live) fn startup_status_from_proto(
    s: moonproto::StartupStatus,
) -> CoreStartupStatus {
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
pub(super) fn init_step_by_name(raw: &str) -> Option<CoreInitStep> {
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
