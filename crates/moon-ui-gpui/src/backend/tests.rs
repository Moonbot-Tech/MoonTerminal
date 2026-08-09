//! Backend transition regressions for atomic Main requests and Auto workspace routing.

use moon_core::config::{
    DEFAULT_ORDER_SIZES_USD, GroupExitSettings, GroupTradeSettings, TakeProfitMode, WorkspaceMode,
};
use moon_core::feed::ClientSettingsEdit;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use super::{
    Backend, OpenCompareRequest, OpenMainRequest, apply_group_exit_edit,
    finalize_recent_warning_episodes, update_group_trade_pair, usd_to_base_amount,
};
use crate::backend::core_warn::{WarnAxis, WarnEnabled, WarnEpisode, WarnSnapshot};

/// Build one warning episode for scope/ordering regressions.
///
/// Args:
///     core_id: Optional owner for a core-specific warning.
///     ip: Server owner used by server-wide warnings.
///     start_ms: Ordering timestamp.
///
/// Returns:
///     Enabled closed memory warning with the requested identity.
fn warning_episode(core_id: Option<u64>, ip: IpAddr, start_ms: i64) -> WarnEpisode {
    WarnEpisode {
        id: start_ms as u64,
        axis: if core_id.is_some() {
            WarnAxis::MemGrowth
        } else {
            WarnAxis::SysCpu
        },
        server_ip: Some(ip),
        core_id,
        start_ms,
        end_ms: Some(start_ms + 1),
        peak: 1,
        snap: WarnSnapshot::default(),
    }
}

/// `backend::finalize_recent_warning_episodes` must filter effective membership before LIMIT.
///
/// Mutation: truncate `all` before its scope retain. The two newer foreign rows then consume the
/// limit and the selected core/server warnings disappear from Core Status.
#[test]
fn foreign_warning_rows_cannot_crowd_out_effective_scope() {
    let selected_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
    let foreign_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
    let rows = vec![
        warning_episode(Some(99), foreign_ip, 4_000),
        warning_episode(None, foreign_ip, 3_000),
        warning_episode(Some(7), selected_ip, 2_000),
        warning_episode(None, selected_ip, 1_000),
    ];
    let enabled = WarnEnabled {
        cpu: true,
        mem: true,
        conn: true,
        ping: true,
        exch: true,
        api: true,
    };

    let scoped = finalize_recent_warning_episodes(
        rows,
        enabled,
        &HashSet::from([7]),
        &HashSet::from([selected_ip]),
        2,
    );

    assert_eq!(scoped.len(), 2);
    assert_eq!(scoped[0].core_id, Some(7));
    assert_eq!(scoped[1].core_id, None);
    assert_eq!(scoped[1].server_ip, Some(selected_ip));
}

/// Regression target: removing the Auto guard from
/// `Backend::classic_trade_core_for_main_transition` lets a chart-open request overwrite the
/// user's remembered Classic manual-trading core.
#[test]
fn auto_chart_open_preserves_classic_trade_core() {
    let mut request = OpenMainRequest::default();
    request.request(
        (22, "BTCUSDT".to_string()),
        "desk".to_string(),
        Some("desk".to_string()),
        true,
    );
    let pending = request
        .pending_for_group("desk")
        .cloned()
        .expect("the owning group must see its atomic chart-open request");
    let (opened_core, opened_market, activate) = request
        .take_if_matches("desk", &pending)
        .expect("the unchanged request must drain exactly once");

    let mut remembered = std::collections::HashMap::from([("desk".to_string(), 11)]);
    if let Some(core) = Backend::classic_trade_core_for_main_transition(
        WorkspaceMode::AutoTrading,
        None,
        opened_core,
    ) {
        remembered.insert("desk".to_string(), core);
    }

    assert_eq!(opened_market, "BTCUSDT");
    assert!(activate);
    assert_eq!(request.addressed_group(), Some("desk"));
    assert_eq!(request.revision(), 1);
    assert_eq!(remembered.get("desk"), Some(&11));
    assert_eq!(
        Backend::classic_trade_core_for_main_transition(WorkspaceMode::Classic, None, opened_core),
        Some(22)
    );
}

/// Protects a pending chart-open request when settings move its target core between groups.
///
/// Plausible breakage: omitting `OpenMainRequest::reconcile_group` leaves the request addressed to
/// alpha, so beta never wakes while alpha can consume a core it no longer owns.
#[test]
fn pending_main_request_retargets_when_its_core_moves_groups() {
    let mut request = OpenMainRequest::default();
    request.request((22, "BTCUSDT".to_string()), "alpha".to_string(), None, true);

    assert!(request.reconcile_group(Some("beta".to_string())));
    assert_eq!(request.addressed_group(), Some("beta"));
    assert_eq!(request.revision(), 2);
    assert!(request.pending_for_group("alpha").is_none());
    let pending = request
        .pending_for_group("beta")
        .cloned()
        .expect("the moved core's current group must own the pending request");
    assert!(request.take_if_matches("alpha", &pending).is_none());
    assert_eq!(
        request.take_if_matches("beta", &pending),
        Some((22, "BTCUSDT".to_string(), true))
    );
}

/// Protects a group-owned chart request from following a core moved after the producer click.
///
/// Plausible breakage: dropping `OpenMainRequest::authority_group` makes a stale Auto callback from
/// alpha reveal in beta and potentially replace beta's remembered Classic Main core.
#[test]
fn scoped_main_request_cancels_instead_of_following_a_moved_core() {
    let mut request = OpenMainRequest::default();
    request.request(
        (22, "BTCUSDT".to_string()),
        "alpha".to_string(),
        Some("alpha".to_string()),
        true,
    );

    assert!(request.reconcile_group(Some("beta".to_string())));
    assert!(!request.is_pending());
    assert_eq!(request.addressed_group(), None);
    assert!(request.pending_for_group("alpha").is_none());
    assert!(request.pending_for_group("beta").is_none());
}

/// Protects comparison navigation from being consumed after its scoped producer loses authority.
///
/// Plausible breakage: treating every request as unscoped lets a Detects callback opened in alpha
/// create a beta comparison tab after Settings moves the target core.
#[test]
fn scoped_compare_request_refuses_moved_or_hidden_targets() {
    let request = OpenCompareRequest::new((22, "BTCUSDT".to_string()), Some("alpha".to_string()));

    assert!(request.allows_group("alpha", Some("alpha"), true));
    assert!(!request.allows_group("beta", Some("beta"), true));
    assert!(!request.allows_group("alpha", Some("alpha"), false));
}

/// Protects pending chart-open cancellation when settings remove its target core.
///
/// Plausible breakage: retaining the prior route after session reconciliation lets a stale group
/// consume and reveal a chart for a core that no longer exists.
#[test]
fn pending_main_request_cancels_when_its_core_is_removed() {
    let mut request = OpenMainRequest::default();
    request.request(
        (22, "BTCUSDT".to_string()),
        "alpha".to_string(),
        Some("alpha".to_string()),
        true,
    );

    assert!(request.reconcile_group(None));
    assert!(!request.is_pending());
    assert_eq!(request.addressed_group(), None);
    assert_eq!(request.revision(), 2);
    assert_eq!(request.pending_revision_for_group("alpha"), 0);
    assert!(request.pending_for_group("alpha").is_none());
    assert!(
        request
            .take_if_matches("alpha", &(22, "BTCUSDT".to_string()))
            .is_none()
    );
}

#[test]
/// Regression target: removing the preview branch in `backend::update_group_trade_pair` lets an
/// already-open Settings window save its stale TP and undo the value the user sees in the toolbar.
fn live_group_edits_are_mirrored_without_erasing_preview_imports() {
    let mut live = GroupTradeSettings::default();
    let mut preview = live.clone();
    preview.order_sizes_usd = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

    update_group_trade_pair(&mut live, Some(&mut preview), |trade| {
        trade.order_size_sel = 4;
    });

    assert_eq!(live.order_size_sel, 4);
    assert_eq!(preview.order_size_sel, 4);
    assert_eq!(preview.order_sizes_usd, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    assert_eq!(live.order_sizes_usd, DEFAULT_ORDER_SIZES_USD);
}

#[test]
/// Regression target: changing `backend::usd_to_base_amount` to default a missing/zero FX rate to
/// one places a base-coin order with the visible dollar number and can oversize it catastrophically.
fn usd_conversion_fails_closed_without_a_positive_rate() {
    assert_eq!(usd_to_base_amount(100.0, None), None);
    assert_eq!(usd_to_base_amount(100.0, Some(0.0)), None);
    assert_eq!(usd_to_base_amount(100.0, Some(f64::NAN)), None);
    assert_eq!(usd_to_base_amount(f64::MAX, Some(f64::MIN_POSITIVE)), None);
    assert_eq!(usd_to_base_amount(100.0, Some(50_000.0)), Some(0.002));
}

/// Regression target: removing the finite guard from `backend::apply_group_exit_edit` persists NaN;
/// because NaN never equals its echo, every later manual order remains behind the settings barrier.
#[test]
fn nonfinite_exit_edits_cannot_poison_the_group_generation() {
    let mut exit = GroupExitSettings {
        take_profit_pct: 10.0,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_pcts: [1.0; 6],
        fixed_sell_slot: None,
        stop_loss_pct: -2.0,
        stop_loss_enabled: true,
        use_stop_market: false,
    };
    let original = exit;

    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::TakeProfit {
            pct: f64::NAN,
            extended: false,
        }
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::StopLossPct(f32::NAN)
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::SetFixedSellPct {
            slot: 1,
            pct: f64::NAN,
        }
    ));
    assert!(!apply_group_exit_edit(
        &mut exit,
        ClientSettingsEdit::SetFixedSellPct {
            slot: 1,
            pct: 1.0e300,
        }
    ));
    assert_eq!(exit, original);
}
