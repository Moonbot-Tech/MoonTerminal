//! Generation and parity tests for asynchronous chart-warning reads.

use std::net::{IpAddr, Ipv4Addr};

use super::{WarnChartReadCache, WarnChartReadKey, WarnSeriesState, cluster_chart_episodes};
use crate::backend::core_warn::store::PersistedWarnSeries;
use crate::backend::core_warn::{WarnAxis, WarnEnabled, WarnEpisode, WarnSnapshot};
use crate::load_state::LoadState;

/// Build one chart-read key with independent live and persisted revisions.
fn read_key(episode_rev: u64, store_rev: u64) -> WarnChartReadKey {
    WarnChartReadKey {
        ip: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
        episode_rev,
        store_rev: Some(store_rev),
    }
}

/// Build one synthetic episode for chart filtering and clustering.
fn episode(id: u64, axis: WarnAxis, start_ms: i64, end_ms: Option<i64>) -> WarnEpisode {
    WarnEpisode {
        id,
        axis,
        server_ip: Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))),
        core_id: None,
        start_ms,
        end_ms,
        peak: 90,
        snap: WarnSnapshot::default(),
    }
}

/// Build one non-empty persisted-series result.
fn series() -> PersistedWarnSeries {
    PersistedWarnSeries {
        server: Some(vec![(10, 20), (30, 40)]),
        ping: Some(vec![15, 25]),
        exch: Some(vec![35, 45]),
    }
}

/// An older chart completion or failure must not replace or reset the newest request.
#[test]
fn chart_reads_accept_only_the_latest_generation() {
    let mut cache = WarnChartReadCache::default();
    let older = cache.begin(read_key(1, 1)).unwrap();
    let newer = cache.begin(read_key(2, 2)).unwrap();
    assert!(!cache.apply(older));
    cache.fail(older);
    assert_eq!(cache.begin(read_key(2, 2)), None);
    assert!(cache.apply(newer));
    cache.fail(newer);
    assert!(cache.begin(read_key(2, 2)).is_some());
}

/// A result for a previous hover must stay out of the current episode's card.
#[test]
fn hover_series_publish_only_the_current_episode() {
    let mut state = WarnSeriesState::default();
    let older = state.begin(11).unwrap();
    let newer = state.begin(22).unwrap();
    assert!(!state.apply(older, 11, Ok(series())));
    assert!(matches!(state.state, LoadState::Loading { .. }));
    assert!(state.apply(newer, 22, Ok(series())));
    assert_eq!(
        state.state.data().and_then(|value| value.server.as_deref()),
        Some(&[(10, 20), (30, 40)][..])
    );
    state.clear();
    assert!(!state.apply(newer, 22, Ok(series())));
}

/// One transient failure permits a retry, while a second failure and stale failures cannot loop.
#[test]
fn hover_series_retries_once_and_invalidates_with_hover() {
    let mut state = WarnSeriesState::default();
    let first = state.begin(11).unwrap();
    assert!(state.fail(first, 11));
    assert_eq!(state.key, None);
    let retry = state.begin(11).unwrap();
    assert!(!state.fail(retry, 11));
    assert!(matches!(state.state, LoadState::NotReady));
    assert_eq!(state.begin(11), None);
    state.clear();
    assert!(state.begin(11).is_some());
    let current = state.begin(22).unwrap();
    assert!(!state.fail(current, 11));
}

/// Background merging must retain the old axis filters and open-cluster semantics.
#[test]
fn chart_cluster_merge_preserves_filters_and_open_state() {
    let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let persisted = vec![
        episode(1, WarnAxis::SysCpu, 1_000, Some(2_000)),
        episode(2, WarnAxis::MemGrowth, 1_500, Some(2_500)),
        episode(3, WarnAxis::ApiExpiry, 1_700, Some(2_600)),
    ];
    let open = vec![episode(4, WarnAxis::Unreachable, 1_800, None)];
    let enabled = WarnEnabled {
        mem: false,
        ..WarnEnabled::default()
    };
    let clusters = cluster_chart_episodes(
        persisted,
        open,
        ip,
        0,
        10_000,
        enabled,
        moon_core::config::layout::WarnParams::default(),
    );
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].id, 1);
    assert!(clusters[0].open);
    assert_eq!(clusters[0].end_ms, None);
    assert!(clusters[0].conn);
    assert!(clusters[0].cpu_peak.is_some());
    assert!(clusters[0].mem_peak.is_none());
}
