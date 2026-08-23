//! Backend transition regressions for atomic Main requests and Auto workspace routing.

use moon_core::config::WorkspaceMode;

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};

use super::{
    Backend, ChartHistoryScope, OpenCompareRequest, OpenMainRequest,
    finalize_recent_warning_episodes,
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
        ChartHistoryScope::Default,
        "desk".to_string(),
        Some("desk".to_string()),
        true,
    );
    let pending = request
        .pending_for_group("desk")
        .cloned()
        .expect("the owning group must see its atomic chart-open request");
    let (opened_core, opened_market, history, activate) = request
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
    assert_eq!(history, ChartHistoryScope::Default);
    assert!(activate);
    assert_eq!(request.addressed_group(), Some("desk"));
    assert_eq!(request.revision(), 1);
    assert_eq!(remembered.get("desk"), Some(&11));
    assert_eq!(
        Backend::classic_trade_core_for_main_transition(WorkspaceMode::Classic, None, opened_core),
        Some(22)
    );
}

/// Dropping history from `OpenMainRequest`, or draining it from a parallel field, must fail the
/// independently constructed Report scope after one atomic request round trip.
#[test]
fn main_request_round_trips_exact_target_and_published_report_scope_atomically() {
    let history = ChartHistoryScope::Report {
        filter: moon_core::db::ReportFilter {
            core_uids: vec![91, 92],
            date_from: Some(1_700_000_000),
            date_to: Some(1_700_086_399),
            side: moon_core::db::SideFilter::Short,
            ..Default::default()
        },
        exact_coin: "BTCUSDT".to_string(),
        focus_record_id: Some(401),
    };
    let mut request = OpenMainRequest::default();
    request.request(
        (91, "BTCUSDT".to_string()),
        history.clone(),
        "alpha".to_string(),
        Some("alpha".to_string()),
        false,
    );
    let pending = request
        .pending_for_group("alpha")
        .cloned()
        .expect("the exact owning group must observe the request");

    assert_eq!(
        request.take_if_matches("alpha", &pending),
        Some((91, "BTCUSDT".to_string(), history, false))
    );
}

/// Protects a pending chart-open request when settings move its target core between groups.
///
/// Plausible breakage: omitting `OpenMainRequest::reconcile_group` leaves the request addressed to
/// alpha, so beta never wakes while alpha can consume a core it no longer owns.
#[test]
fn pending_main_request_retargets_when_its_core_moves_groups() {
    let mut request = OpenMainRequest::default();
    request.request(
        (22, "BTCUSDT".to_string()),
        ChartHistoryScope::Default,
        "alpha".to_string(),
        None,
        true,
    );

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
        Some((22, "BTCUSDT".to_string(), ChartHistoryScope::Default, true,))
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
        ChartHistoryScope::Default,
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

/// A comparison started from a chart carries BOTH sides, and the authority rule applies to the
/// request as a whole.
///
/// Plausible breakage: keeping only the target. The tab would then open holding the destination
/// alone — a comparison of one chart, which is not a comparison, and not what an arbitrage click
/// asked for.
#[test]
fn a_paired_compare_request_keeps_the_chart_it_started_from() {
    let request = OpenCompareRequest::pair(
        (7, "ENAUSDT".to_string()),
        (22, "ENAUSDT".to_string()),
        Some("alpha".to_string()),
    );

    assert_eq!(request.anchor_for_test(), Some(&(7, "ENAUSDT".to_string())));
    assert!(request.allows_group("alpha", Some("alpha"), true));
    assert!(!request.allows_group("beta", Some("beta"), true));

    // The other producers state one coin and let the tab gather the rest.
    let plain = OpenCompareRequest::new((22, "ENAUSDT".to_string()), None);
    assert_eq!(plain.anchor_for_test(), None);
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
        ChartHistoryScope::Default,
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
