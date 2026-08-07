use super::*;

fn accepted(epoch: u64) -> ArchiveRequest {
    ArchiveRequest {
        epoch,
        outcome: Outcome::Accepted,
    }
}

fn refused(epoch: u64, count: u32, at: Instant) -> ArchiveRequest {
    ArchiveRequest {
        epoch,
        outcome: Outcome::Refused { count, at },
    }
}

#[test]
fn an_accepted_request_is_never_repeated_for_the_same_client() {
    assert_eq!(
        decide(Some(&accepted(4)), 4, Instant::now()),
        Decision::SkipAccepted,
        "MoonProto retries an accepted request itself, forever; a second send would only add \
         another endless multi-megabyte transfer"
    );
}

#[test]
fn a_replaced_client_reopens_an_accepted_request() {
    assert_eq!(
        decide(Some(&accepted(4)), 5, Instant::now()),
        Decision::SendNewClient,
        "a reconnect installs a new client with empty retained rings, so the archive is gone and \
         must be asked for again"
    );
}

#[test]
fn a_first_request_is_always_allowed() {
    assert_eq!(decide(None, 1, Instant::now()), Decision::SendFirst);
}

#[test]
fn a_refusal_waits_out_its_window_then_may_retry() {
    let now = Instant::now();

    assert_eq!(
        decide(Some(&refused(1, 1, now)), 1, now),
        Decision::SkipRefusalWindow
    );
    assert_eq!(
        decide(Some(&refused(1, 1, now)), 1, now + ARCHIVE_SEND_RETRY),
        Decision::SendRetryRefusal
    );
}

#[test]
fn refusals_stop_at_the_cap_but_the_cap_dies_with_its_client() {
    let now = Instant::now();
    let later = now + ARCHIVE_SEND_RETRY;
    let capped = refused(1, ARCHIVE_MAX_REFUSALS, now);

    assert_eq!(
        decide(Some(&capped), 1, later),
        Decision::SkipRefusalCap,
        "a client whose runtime is gone refuses forever; the cap stops the ticking"
    );
    assert_eq!(
        decide(Some(&capped), 2, later),
        Decision::SendNewClient,
        "the cap must not outlive the client it was counted against"
    );
}

#[test]
fn spacing_holds_back_a_burst_but_not_the_first_send() {
    let now = Instant::now();

    assert!(spacing_allows(None, now));
    assert!(
        !spacing_allows(Some(now), now),
        "a restored layout opens every chart in one frame; their merges must not land together"
    );
    assert!(spacing_allows(Some(now), now + ARCHIVE_SEND_SPACING));
}

#[test]
fn forgetting_a_provider_leaves_its_neighbours_claimed() {
    let gate = ArchiveGate::default();
    {
        let mut state = gate.state.lock().unwrap();
        state
            .markets
            .entry(42)
            .or_default()
            .insert("BTCUSDT".to_string(), accepted(1));
        state
            .markets
            .entry(7)
            .or_default()
            .insert("ETHUSDT".to_string(), accepted(1));
    }

    gate.forget_provider(42);

    let state = gate.state.lock().unwrap();
    assert!(!state.markets.contains_key(&42));
    assert!(
        state.markets[&7].contains_key("ETHUSDT"),
        "a departed provider must not take another provider's claims with it"
    );
}
