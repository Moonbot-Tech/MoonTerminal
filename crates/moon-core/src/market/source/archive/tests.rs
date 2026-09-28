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

#[test]
fn an_answered_request_is_never_repeated_either() {
    let answered = ArchiveRequest {
        epoch: 4,
        outcome: Outcome::Answered,
    };
    assert_eq!(
        decide(Some(&answered), 4, Instant::now()),
        Decision::SkipAccepted
    );
    assert_eq!(
        decide(Some(&answered), 5, Instant::now()),
        Decision::SendNewClient,
        "an answer belongs to the client that received it"
    );
}

fn claim(gate: &ArchiveGate, provider: CoreId, market: &str, request: ArchiveRequest) {
    gate.state
        .lock()
        .unwrap()
        .markets
        .entry(provider)
        .or_default()
        .insert(market.to_string(), request);
}

#[test]
fn an_answer_moves_only_an_accepted_request() {
    let gate = ArchiveGate::default();
    claim(&gate, 1, "BTCUSDT", accepted(1));
    claim(&gate, 1, "ETHUSDT", refused(1, 1, Instant::now()));

    gate.answered(1, "BTCUSDT", 1);
    gate.answered(1, "ETHUSDT", 1);
    gate.answered(2, "BTCUSDT", 1);

    let state = gate.state.lock().unwrap();
    assert_eq!(state.markets[&1]["BTCUSDT"].outcome, Outcome::Answered);
    assert!(
        matches!(
            state.markets[&1]["ETHUSDT"].outcome,
            Outcome::Refused { .. }
        ),
        "a refusal was never sent, so no answer can belong to it"
    );
    assert!(
        !state.markets.contains_key(&2),
        "an answer for a market this gate never asked records nothing"
    );
}

#[test]
fn a_waiting_reader_wakes_on_the_answer_not_at_the_deadline() {
    let gate = std::sync::Arc::new(ArchiveGate::default());
    claim(&gate, 1, "BTCUSDT", accepted(1));
    let answering = gate.clone();
    let answer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        answering.answered(1, "BTCUSDT", 1);
    });

    let started = Instant::now();
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        started + Duration::from_secs(10),
        || true,
        || {},
    );
    answer.join().unwrap();

    assert_eq!(waited, ArchiveWait::Answered);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_market_answered_earlier_returns_without_waiting() {
    let gate = ArchiveGate::default();
    claim(
        &gate,
        1,
        "BTCUSDT",
        ArchiveRequest {
            epoch: 1,
            outcome: Outcome::Answered,
        },
    );
    let started = Instant::now();
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        started + Duration::from_secs(10),
        || true,
        || {},
    );
    assert_eq!(waited, ArchiveWait::Answered);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn a_refused_send_and_a_silent_core_both_end_the_wait() {
    let gate = ArchiveGate::default();
    claim(&gate, 1, "ETHUSDT", refused(1, 1, Instant::now()));
    let now = Instant::now();
    assert_eq!(
        gate.wait_for_answer(
            1,
            "ETHUSDT",
            1,
            now + Duration::from_secs(10),
            || true,
            || {}
        ),
        ArchiveWait::Refused
    );

    claim(&gate, 1, "BTCUSDT", accepted(1));
    let started = Instant::now();
    assert_eq!(
        gate.wait_for_answer(
            1,
            "BTCUSDT",
            1,
            started + Duration::from_millis(50),
            || true,
            || {}
        ),
        ArchiveWait::TimedOut
    );
}

#[test]
fn an_unsent_request_is_retried_until_it_goes_out() {
    let gate = ArchiveGate::default();
    let mut sends = 0;
    let started = Instant::now();
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        started + Duration::from_secs(10),
        || true,
        || {
            sends += 1;
            // The third try gets past the spacing; the core has already answered by then.
            if sends == 3 {
                claim(
                    &gate,
                    1,
                    "BTCUSDT",
                    ArchiveRequest {
                        epoch: 1,
                        outcome: Outcome::Answered,
                    },
                );
            }
        },
    );
    assert_eq!(waited, ArchiveWait::Answered);
    assert_eq!(sends, 3);
}

#[test]
fn a_reconnect_releases_its_waiting_reader() {
    let gate = std::sync::Arc::new(ArchiveGate::default());
    claim(&gate, 1, "BTCUSDT", accepted(1));
    let current = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let replacing = current.clone();
    let reconnect = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        replacing.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    let started = Instant::now();
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        started + Duration::from_secs(10),
        || current.load(std::sync::atomic::Ordering::SeqCst),
        || {},
    );
    reconnect.join().unwrap();

    assert_eq!(waited, ArchiveWait::Superseded);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a replaced client answers nothing; its reader must not sit out the deadline"
    );
}

#[test]
fn an_answer_from_the_previous_connection_settles_nothing() {
    let gate = ArchiveGate::default();
    // The new connection (epoch 2) has already asked; the old one's answer is still in flight.
    claim(&gate, 1, "BTCUSDT", accepted(2));

    gate.answered(1, "BTCUSDT", 1);
    assert_eq!(
        gate.state.lock().unwrap().markets[&1]["BTCUSDT"].outcome,
        Outcome::Accepted,
        "the new core has not answered; the old connection's merge is not in its rings"
    );

    gate.answered(1, "BTCUSDT", 2);
    assert_eq!(
        gate.state.lock().unwrap().markets[&1]["BTCUSDT"].outcome,
        Outcome::Answered
    );
}

#[test]
fn a_stale_reader_never_sends_through_its_dead_client() {
    let gate = ArchiveGate::default();
    let mut sends = 0;
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        Instant::now() + Duration::from_secs(10),
        || false,
        || sends += 1,
    );
    assert_eq!(waited, ArchiveWait::Superseded);
    assert_eq!(
        sends, 0,
        "a send through a replaced client would overwrite the new client's request"
    );
}

#[test]
fn forgetting_the_provider_releases_its_waiting_reader_without_a_resend() {
    let gate = std::sync::Arc::new(ArchiveGate::default());
    claim(&gate, 1, "BTCUSDT", accepted(1));
    let forgetting = gate.clone();
    let forget = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        forgetting.forget_provider(1);
    });

    let sends = std::sync::atomic::AtomicUsize::new(0);
    let started = Instant::now();
    let waited = gate.wait_for_answer(
        1,
        "BTCUSDT",
        1,
        started + Duration::from_secs(10),
        // A respawn forgets the provider before the old slot goes dark: the old client still
        // looks current here.
        || true,
        || {
            sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
    );
    forget.join().unwrap();

    assert_eq!(waited, ArchiveWait::Superseded);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        sends.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a resend through the dying client would record a claim the new slot's first epoch          collides with, and the new client's request would never go out"
    );
    assert!(!gate.state.lock().unwrap().markets.contains_key(&1));
}
