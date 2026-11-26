//! Regression tests through SessionManager's local command queue and retained snapshots.

use super::*;
use crate::config::{ServerConfig, TakeProfitMode};
use crate::feed::ClientSettings;
use crate::feed::tests::Attempt;
use crate::market::{MarketDataSource, MarketStore};
use crate::session::{CoreSession, CoreStore, conn_sig};
use std::ops::{Deref, DerefMut};
use std::sync::mpsc::Receiver;

const CORE: CoreId = 9_111;

/// Keep the inert feed's receivers alive until the session handle has stopped it.
struct Fixture {
    session: SessionManager,
    _attempts: Receiver<Attempt>,
    done: Receiver<()>,
}

impl Deref for Fixture {
    type Target = SessionManager;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl DerefMut for Fixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.session.sessions.clear();
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

/// A session with an inert transport whose command receiver remains alive during the test.
fn fixture(on: bool, text: &str) -> Fixture {
    let config: ServerConfig = serde_json::from_str(r#"{"id":9111,"name":"CORE_A"}"#).unwrap();
    let (handle, attempts, done) = crate::feed::tests::backoff_feed(config.clone());
    attempts.recv_timeout(Duration::from_secs(2)).unwrap();
    let market = MarketStore::shared(0.0);
    let mut store = CoreStore::default();
    store.ensure(CORE);
    store.core_mut(CORE).unwrap().client_settings = Some(ClientSettings {
        take_profit_pct: 0.0,
        take_profit_main_pct: 0.0,
        take_profit_extended: false,
        take_profit_mode: TakeProfitMode::Normal,
        fixed_sell_mode: false,
        stop_loss_pct: 0.0,
        trailing_drop_pct: 0.0,
        use_global_take_profit: false,
        global_take_profit_pct: 0.0,
        panic_if_price_drop: false,
        emu_mode: false,
        buy_iceberg: false,
        sell_iceberg: false,
        sign_orders: false,
        use_stop_market: false,
        vol_drop_level: 0,
        use_blacklist: on,
        blacklist_text: text.to_string(),
        fixed_sell_pcts: [0.0; 6],
        fixed_sell_slot: 1,
        use_manual_strategy: false,
        manual_strategy_id: 0,
    });
    let session = SessionManager {
        blacklists_sent: Default::default(),
        trade_sounds: Vec::new(),
        trade_sound_epochs: Default::default(),
        catchup_last: Default::default(),
        catchup_unknown: Default::default(),
        catchup_running: Default::default(),
        sessions: vec![CoreSession {
            id: CORE,
            name: config.name.clone(),
            group: config.group.clone(),
            conn_sig: conn_sig(&config),
            handle,
        }],
        config_order: Vec::new(),
        feed_wake: None,
        order_retirement: Default::default(),
        store,
        market_source: MarketDataSource::new(market.clone()),
        market,
        mode: Default::default(),
        core_venue: Default::default(),
        core_base: Default::default(),
        core_provider: Default::default(),
        providers: Default::default(),
        wanted: Default::default(),
        pending_drop: Default::default(),
        wanted_orderbook: Default::default(),
        pending_ob_drop: Default::default(),
        last_cmd: Default::default(),
        core_updates: Default::default(),
        identity_respawns: Default::default(),
    };
    Fixture {
        session,
        _attempts: attempts,
        done,
    }
}

/// Publish one full list snapshot and reconcile accepted intents like the session drain.
fn echo(session: &mut SessionManager, on: bool, text: &str) {
    let cs = session
        .store
        .core_mut(CORE)
        .unwrap()
        .client_settings
        .as_mut()
        .unwrap();
    cs.use_blacklist = on;
    cs.blacklist_text = text.to_string();
    session.observe_blacklist_echo(CORE, false);
}

/// Assert the most recently recorded list write independently of the published echo.
fn assert_sent(session: &SessionManager, on: bool, text: &str) {
    let writes = session.blacklists_sent.lock().unwrap();
    assert_eq!(writes[&(CORE, None)].sent, (on, text.to_string()));
}

/// A single uncontested insertion restores the original formatting and switch.
#[test]
fn add_then_revert_restores_text_byte_for_byte() {
    for text in ["", "A,B", "A, B", "A,B,"] {
        let session = fixture(false, text);
        let undo = session
            .add_to_blacklist(CORE, None, "XALPHA")
            .unwrap()
            .unwrap();
        assert!(undo(&session).unwrap());
        assert_sent(&session, false, text);
    }
}

/// The next command may be queued before any echo, and owns its added token.
#[test]
fn add_a_then_b_before_echo_revert_a_keeps_b_and_flag() {
    let session = fixture(false, "");
    let undo = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    session.add_to_blacklist(CORE, None, "XBETA").unwrap();
    assert!(undo(&session).unwrap());
    assert_sent(&session, true, "XBETA");
}

/// A snapshot of the first write cannot displace the newer pending list.
#[test]
fn undo_with_in_flight_intermediate_echo_removes_only_token() {
    let mut session = fixture(false, "");
    let undo = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    session.add_to_blacklist(CORE, None, "XBETA").unwrap();
    echo(&mut session, true, "XALPHA");
    assert!(undo(&session).unwrap());
    assert_sent(&session, true, "XBETA");
}

/// A replacement with identical text can have a different owner for the original token.
#[test]
fn undo_after_lift_and_readd_does_nothing() {
    let mut session = fixture(false, "");
    let older = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    echo(&mut session, true, "XALPHA");
    session.write_core_blacklist(CORE, "xalpha", true).unwrap();
    let newer = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    assert!(!older(&session).unwrap());
    assert_sent(&session, true, "XALPHA");
    assert!(newer(&session).unwrap());
    assert_sent(&session, true, "");
}

/// Core Settings and the coin menu must share the same write tracker.
#[test]
fn undo_does_not_erase_queued_settings_edit() {
    let mut session = fixture(false, "");
    let undo = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    echo(&mut session, true, "XALPHA");
    session
        .set_blacklist(CORE, true, "XALPHA, XBETA,".to_string())
        .unwrap();
    assert!(undo(&session).unwrap());
    assert_sent(&session, true, " XBETA,");
}

/// A competing edit owns its text and switch except for the inserted token.
#[test]
fn revert_after_intervening_edit_removes_only_token() {
    let mut session = fixture(false, "A,B,");
    let undo = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    echo(&mut session, true, "XALPHA, B, XCETUS,");
    assert!(undo(&session).unwrap());
    assert_sent(&session, true, " B, XCETUS,");
}

/// The preexisting enabled switch survives exact-text undo.
#[test]
fn revert_does_not_disable_flag_it_did_not_enable() {
    let session = fixture(true, "A, B");
    let undo = session
        .add_to_blacklist(CORE, None, "XALPHA")
        .unwrap()
        .unwrap();
    assert!(undo(&session).unwrap());
    assert_sent(&session, true, "A, B");
}

/// A single existing token can enable a list without being counted as a new entry.
#[test]
fn enabling_existing_token_does_not_return_inverse() {
    let session = fixture(false, "A, XALPHA");
    assert!(
        session
            .add_to_blacklist(CORE, None, "xalpha")
            .unwrap()
            .is_none()
    );
    assert_sent(&session, true, "A, XALPHA");
}

/// A bulk click owns its switch change even if its first coin was already present.
#[test]
fn multi_undo_disables_flag_it_enabled_even_if_first_coin_was_listed() {
    let session = fixture(false, "XALPHA");
    let (coins, undo) = session
        .add_many_to_blacklist(CORE, &["XALPHA", "XBETA"])
        .unwrap()
        .unwrap();
    assert_eq!(coins, ["XBETA"]);
    assert_sent(&session, true, "XALPHA,XBETA");
    assert!(undo(&session).unwrap());
    assert_sent(&session, false, "XALPHA");
}

/// Undo of a second pending temp ban restores the first, which can still be undone.
#[test]
fn temp_undo_second_click_before_echo_restores_first_ban() {
    let session = fixture(false, "");
    let first = session
        .add_temp_ban_with_undo(CORE, "XALPHAUSDT".to_string(), Duration::from_secs(3600))
        .unwrap();
    let second = session
        .add_temp_ban_with_undo(CORE, "XALPHAUSDT".to_string(), Duration::from_secs(14_400))
        .unwrap();
    assert!(second(&session).unwrap());
    assert!(first(&session).unwrap());
}

/// A later refresh on the market supersedes the earlier inverse before the feed echoes.
#[test]
fn temp_undo_skips_when_ban_refreshed_later() {
    let session = fixture(false, "");
    let older = session
        .add_temp_ban_with_undo(CORE, "XALPHAUSDT".to_string(), Duration::from_secs(3600))
        .unwrap();
    let newer = session
        .add_temp_ban_with_undo(CORE, "XALPHAUSDT".to_string(), Duration::from_secs(3600))
        .unwrap();
    assert!(!older(&session).unwrap());
    assert!(newer(&session).unwrap());
}

/// The menu's forced cancellation reaches the core queue as its own command, never as a cancel.
///
/// Mutation: make `force_cancel_order` send `CoreCmd::CancelOrder`. The confirmation the user
/// accepted would then promise the background cancellation and get the ordinary one, which a
/// stuck order already ignores. This stops at the session queue: the feed's mapping onto
/// `orders().force_cancel` needs a live `MoonClient` and is not reached here.
#[test]
fn force_cancel_queues_its_own_command() {
    let session = fixture(false, "");
    session.force_cancel_order(CORE, 4242).unwrap();
    let (_, queued, _) = session
        ._attempts
        .recv_timeout(Duration::from_secs(10))
        .expect("the retry loop's second attempt drains the queue");
    let kinds: Vec<_> = queued.iter().map(|q| &q.cmd).collect();
    assert!(
        kinds
            .iter()
            .any(|cmd| matches!(cmd, crate::feed::CoreCmd::ForceCancelOrder { uid: 4242 })),
        "force cancel was not queued: {kinds:?}"
    );
    assert!(
        !kinds
            .iter()
            .any(|cmd| matches!(cmd, crate::feed::CoreCmd::CancelOrder { .. })),
        "force cancel must not fall back to the ordinary cancel: {kinds:?}"
    );
}
