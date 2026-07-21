use super::{BalanceState, ConnStatus, CoreData};

/// A core with the given freshness inputs; everything else stays at its default.
fn core(assets_rev: u64, rate_known: bool, stale: bool, status: ConnStatus) -> CoreData {
    let mut cd = CoreData::new();
    cd.assets_rev = assets_rev;
    cd.assets.global.usd_rate_known = rate_known;
    cd.assets_stale = stale;
    cd.status = status;
    cd
}

/// No snapshot yet is UNKNOWN, never zero — the distinction the Assets panel exists to make.
#[test]
fn without_a_snapshot_the_balance_is_awaiting() {
    let cd = core(0, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Awaiting);
    assert!(!cd.balance_state().has_value());
}

/// An unvaluable snapshot outranks staleness: there is no number, so its age is moot.
#[test]
fn unpriced_outranks_stale() {
    let cd = core(7, false, true, ConnStatus::Disconnected);
    assert_eq!(cd.balance_state(), BalanceState::Unpriced);
    assert!(!cd.balance_state().has_value());
}

/// A live connection is not enough on its own. `assets_rev` and the snapshot both survive a
/// reconnect, so a core back at `Ready` still carries the retained figure until a new snapshot
/// clears the marker — without this the pre-outage balance would be re-promoted to Live.
#[test]
fn a_reconnected_core_stays_stale_until_the_marker_clears() {
    let reconnected = core(7, true, true, ConnStatus::Ready);
    assert_eq!(reconnected.balance_state(), BalanceState::Stale);
    // A retained figure is still a figure: it is shown, but only with its stale marker.
    assert!(reconnected.balance_state().has_value());
    assert!(!reconnected.balance_state().is_current());
}

/// The other half of staleness: a snapshot that arrived before the link ever reached `Ready`.
#[test]
fn a_snapshot_from_a_not_ready_link_is_stale() {
    let cd = core(7, true, false, ConnStatus::Connecting);
    assert_eq!(cd.balance_state(), BalanceState::Stale);
}

/// Ready, priced, and with no stale marker — the only combination that renders at full
/// strength.
#[test]
fn ready_priced_and_unmarked_is_live() {
    let cd = core(7, true, false, ConnStatus::Ready);
    assert_eq!(cd.balance_state(), BalanceState::Live);
    assert!(cd.balance_state().has_value());
    assert!(cd.balance_state().is_current());
}

/// `code()` must separate every variant: it is hashed into a render signature, so a collision
/// would let one trust state be cached as another.
#[test]
fn every_state_hashes_distinctly() {
    let all = [
        BalanceState::Live,
        BalanceState::Stale,
        BalanceState::Awaiting,
        BalanceState::Unpriced,
    ];
    let mut codes: Vec<u64> = all.iter().map(|s| s.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
    // Only Live is current, and only Live/Stale carry a number.
    assert_eq!(all.iter().filter(|s| s.is_current()).count(), 1);
    assert_eq!(all.iter().filter(|s| s.has_value()).count(), 2);
}
