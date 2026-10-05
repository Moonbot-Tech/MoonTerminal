//! The station's mode, pinned in source (`docs-internal/STATION.md` §3.1).
//!
//! What the station may ask a core is decided by which calls exist in its code, so the guard reads
//! the code: the station crate itself, the bot and Mini App it runs (`moon-tg`, shared with the
//! terminal), the tape recorder, and the narrow client that recorder owns. A red test here is a
//! station that can now do something it was never meant to — subscribe to every market, trade
//! outside the Mini App's commands, or hold a raw `MoonClient` past the narrow link.
//!
//! Comments are stripped before anything is searched, so a doc line that NAMES a forbidden call
//! (explaining why it is forbidden) is not a violation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Moving capability dispatch after normal startup would create station state or reject a first push.
#[test]
fn capabilities_are_available_without_config_credentials_or_a_running_service() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_moon-station"))
        .arg("capabilities")
        .output()
        .expect("run the stateless capability query");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "core_endpoint_override=yes"
    );
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the station crate sits at crates/moon-station")
        .to_path_buf()
}

/// Every `.rs` file under `dir`, recursively, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// A file's code with every `//` comment removed — line, doc and trailing ones. A `//` inside a
/// string literal would be cut too; none of the files read here has one that matters.
fn code(path: &Path) -> String {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    text.lines()
        .map(|line| match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `code` names `ident` as a whole word — `unsubscribe_all_trades` does not name
/// `subscribe_all_trades`.
fn names(code: &str, ident: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(ident).any(|(at, _)| {
        let before = code[..at].chars().next_back();
        let after = code[at + ident.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// `code` with every line break and indent before a `.` removed, so a method chain rustfmt
/// spreads over several lines reads as one `self.client.streams()`.
fn join_chains(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut pending = String::new();
    for c in code.chars() {
        if c.is_whitespace() {
            pending.push(c);
            continue;
        }
        if c != '.' {
            out.push_str(&pending);
        }
        pending.clear();
        out.push(c);
    }
    out
}

/// Every identifier that follows `receiver.` in `code` — `session.drain()` gives `drain`.
///
/// With `standalone`, `receiver` must be a whole name: `self.session.` or `my_session.` is another
/// receiver and is skipped. Without it, `receiver` may end a longer chain (`streams()`).
fn methods_called_on(code: &str, receiver: &str, standalone: bool) -> BTreeSet<String> {
    let code = join_chains(code);
    let needle = format!("{receiver}.");
    let mut out = BTreeSet::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find(&needle) {
        let before = rest[..at].chars().next_back();
        rest = &rest[at + needle.len()..];
        if standalone && before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.insert(name);
        }
    }
    out
}

fn sources(dir: &str) -> Vec<(PathBuf, String)> {
    rust_files(&workspace().join(dir))
        .into_iter()
        .map(|path| {
            let code = code(&path);
            (path, code)
        })
        .collect()
}

fn station_sources() -> Vec<(PathBuf, String)> {
    sources("crates/moon-station/src")
}

/// The bot and the Mini App the station runs: its code is the station's as much as the terminal's.
fn tg_sources() -> Vec<(PathBuf, String)> {
    sources("crates/moon-tg/src")
}

/// Breakage guarded: a station component that talks to a core through its own `MoonClient` —
/// and with it every call moonproto has, trading and `subscribe_all_trades` included — instead of
/// the terminal's feed in station mode or the narrow `StationLink`.
#[test]
fn the_station_crate_never_touches_moonproto_directly() {
    for (path, code) in station_sources().into_iter().chain(tg_sources()) {
        for forbidden in ["moonproto", "MoonClient", "subscribe_all_trades", "FeedCmd"] {
            assert!(
                !names(&code, forbidden),
                "{} names `{forbidden}`: the station reaches a core only through moon-core's \
                 station mode and `StationLink` (STATION.md §3.1)",
                path.display()
            );
        }
    }
}

/// Breakage guarded: trading reaching the station anywhere but the one module that is to own it
/// (phase 10, `tg::trade`, and only with a full key and trading switched on).
#[test]
fn trading_stays_inside_tg_trade() {
    let allowed = workspace().join("crates/moon-station/src/tg/trade");
    for (path, code) in station_sources() {
        if path.starts_with(&allowed) || path == allowed.with_extension("rs") {
            continue;
        }
        assert!(
            !names(&code, "TradeLink"),
            "{} names `TradeLink`: trading belongs to `tg::trade` alone (STATION.md §1 item 17)",
            path.display()
        );
    }
}

/// The `SessionManager` calls the station makes: start the feeds, drain them, map each core to
/// itself as its market source, sum the connection status, reconcile on a reload, and rebuild a
/// core whose exchange identity went stale — or whose reconnect the Mini App asked for — on a
/// fresh client (the terminal's Reconnect; it sends the core no command). Anything else it would
/// call on the session — a command to a core, a trading call — lands here first.
const SESSION_CALLS: [&str; 6] = [
    "conn_summary_group",
    "drain",
    "map_cores_to_themselves",
    "reconcile",
    "reconnect",
    "take_identity_respawn_requests",
];

/// What the station's host of the bot adds, in `tg.rs` alone: the order snapshot Panic Sell's
/// state is read from, and Panic Sell itself — the one Mini App command `moon-tg` routes through
/// its host (the override the host keeps must see the toggle).
const TG_HOST_SESSION_CALLS: [&str; 2] = ["panic_sell_market", "store"];

/// Breakage guarded: the station growing a call into the terminal's session beyond the ones
/// its mode was measured with (STATION.md §7.4, §7.7) — each new one is a decision, made here. Any
/// receiver ending in `session` counts: `self.session.` as much as `session.`.
#[test]
fn the_station_calls_only_its_share_of_the_session() {
    let tg_host = workspace().join("crates/moon-station/src/tg.rs");
    for (path, code) in station_sources() {
        let mut allowed: BTreeSet<String> = SESSION_CALLS.iter().map(|s| s.to_string()).collect();
        if path == tg_host {
            allowed.extend(TG_HOST_SESSION_CALLS.iter().map(|s| s.to_string()));
        }
        let called = methods_called_on(&code, "session", false);
        let extra: Vec<_> = called.difference(&allowed).collect();
        assert!(
            extra.is_empty(),
            "{} calls {extra:?} on the session; the station's share is {SESSION_CALLS:?} (and \
             {TG_HOST_SESSION_CALLS:?} in tg.rs) — add a call here only as a deliberate change \
             of the station's mode",
            path.display()
        );
    }
}

/// What the bot and the Mini App read from the sessions, anywhere in `moon-tg`.
/// `market_source` converts an open order's quote-currency PnL into dollars for the Mini App.
const TG_READ_CALLS: [&str; 6] = [
    "core_run_state",
    "core_venues",
    "market_source",
    "sessions",
    "store",
    "strategy_has_blacklist",
];

/// The owner's commands (STATION.md §1 item 9, §4.2: what the key allows), from the Mini App and
/// the chat's Control section alike: the terminal's own session calls, from the shared command
/// module `control.rs` alone. Trading and AutoDetect go through `dispatch_run`, which keeps them
/// away from a core that is not connected; the raw senders are crate-private.
const TG_TRADE_CALLS: [&str; 8] = [
    "apply_strategies",
    "cancel_market_buys",
    "cancel_order",
    "dispatch_run",
    "set_temp_ban",
    "turn_order_panic_sell",
    "write_core_blacklist",
    "write_strategy_blacklist",
];

/// Breakage guarded: a command to a core reaching the station from anywhere in the bot but the
/// shared owner commands — a chat report that trades, a read that switches a core off — or a
/// call the station's mode was never measured with.
#[test]
fn the_bot_trades_only_from_the_owner_commands() {
    let commands = workspace().join("crates/moon-tg/src/control.rs");
    let reads: BTreeSet<String> = TG_READ_CALLS.iter().map(|s| s.to_string()).collect();
    let trades: BTreeSet<String> = TG_TRADE_CALLS.iter().map(|s| s.to_string()).collect();
    let mut traded = BTreeSet::new();
    for (path, code) in tg_sources() {
        let called: BTreeSet<String> = methods_called_on(&code, "session()", false)
            .into_iter()
            .chain(methods_called_on(&code, "session_mut()", false))
            .collect();
        for call in &called {
            if reads.contains(call) {
                continue;
            }
            assert!(
                trades.contains(call),
                "{} calls `{call}` on the session: not a read the bot makes nor an owner \
                 command — a deliberate change of the station's mode",
                path.display()
            );
            assert!(
                path == commands,
                "{} calls `{call}`: owner commands live in control.rs alone",
                path.display()
            );
            traded.insert(call.clone());
        }
    }
    assert_eq!(
        traded, trades,
        "the scanner lost sight of the owner commands"
    );
}

/// Breakage guarded: the tape recorder — the one station component with a client of its own —
/// building a raw `MoonClient` again, or subscribing its donors to every market (the core then
/// streams the exchange AND the client retains every market's rings: +586 MB in a minute,
/// STATION.md §4.3, §7.1).
#[test]
fn the_tape_recorder_holds_only_a_station_link() {
    let dir = workspace().join("crates/moon-core/src/market/tape_recorder");
    let mut links = 0;
    for path in rust_files(&dir) {
        let code = code(&path);
        for forbidden in ["MoonClient", "subscribe_all_trades", "subscribe_trades_for"] {
            assert!(
                !names(&code, forbidden),
                "{} names `{forbidden}`: the recorder selects pairs through `StationLink` alone",
                path.display()
            );
        }
        links += code.matches("StationLink").count();
    }
    assert!(
        links > 0,
        "the tape recorder must build its donors on `StationLink`"
    );
}

/// Breakage guarded: a station opening `trades.sqlite` — the terminal's close-time capture, and
/// the tape recorder comparing against it — which creates the file and its writer thread for
/// nothing: the station's tape is the recorder's own file (STATION.md §4.3).
#[test]
fn the_tape_recorder_compares_only_in_the_terminal() {
    let code_of = |path: &str| code(&workspace().join(path));
    let code = code_of("crates/moon-core/src/market/tape_recorder/mod.rs");
    let gate = code
        .find("feed::station::enabled()")
        .expect("the recorder must ask whether it runs on a station");
    let queued = code
        .find("self.compares.push")
        .expect("the recorder queues its comparisons in one place");
    assert!(
        gate < queued,
        "the station gate must come before a comparison is queued"
    );
    assert_eq!(
        code.matches("self.compares.push").count(),
        1,
        "a second place queues comparisons past the station gate"
    );
    let lifecycle = code_of("crates/moon-core/src/session/lifecycle.rs");
    let from = lifecycle
        .find("fn capture_closed_trade(")
        .expect("the close-time capture has its own function");
    let lifecycle = &lifecycle[from..];
    let gate = lifecycle
        .find("feed::station::enabled()")
        .expect("the close-time capture must ask whether it runs on a station");
    let capture = lifecycle
        .find("trade_replay::worker::capture(")
        .expect("the close-time capture is queued from the session lifecycle");
    assert!(
        gate < capture,
        "a station must not queue the terminal's close-time capture into trades.sqlite"
    );
}

/// What `StationLink` may call on the client it wraps, and what it may call on each handle.
const LINK_CLIENT_CALLS: [&str; 5] = [
    "drain_events_into",
    "drain_lifecycle_events_into",
    "history",
    "snapshot_versioned",
    "streams",
];
const LINK_STREAM_CALLS: [&str; 2] = ["subscribe_trades_for", "unsubscribe_all_trades"];
const LINK_HISTORY_CALLS: [&str; 1] = ["request_chart"];

/// Breakage guarded: `StationLink` widening past the station's mode — a trading call, a
/// settings or strategies call, or `subscribe_all_trades` — which every station component would
/// then reach without anyone deciding it.
#[test]
fn the_station_link_exposes_only_the_station_mode() {
    let path = workspace().join("crates/moon-core/src/feed/station/link.rs");
    let code = code(&path);
    assert!(
        !names(&code, "subscribe_all_trades"),
        "StationLink must never subscribe to every market"
    );
    let client = methods_called_on(&code, "self.client", true);
    let allowed: BTreeSet<String> = LINK_CLIENT_CALLS.iter().map(|s| s.to_string()).collect();
    assert!(
        client.is_subset(&allowed),
        "StationLink calls {:?} on its client; allowed: {LINK_CLIENT_CALLS:?}",
        client.difference(&allowed).collect::<Vec<_>>()
    );
    let streams = methods_called_on(&code, "streams()", false);
    assert!(
        streams
            .iter()
            .all(|m| LINK_STREAM_CALLS.contains(&m.as_str())),
        "StationLink calls {streams:?} on the stream handle; allowed: {LINK_STREAM_CALLS:?}"
    );
    let history = methods_called_on(&code, "history()", false);
    assert!(
        history
            .iter()
            .all(|m| LINK_HISTORY_CALLS.contains(&m.as_str())),
        "StationLink calls {history:?} on the history handle; allowed: {LINK_HISTORY_CALLS:?}"
    );
    // The client is held privately: a `pub client` would hand every caller the whole API.
    assert!(
        !code.contains("pub client") && !code.contains("pub(crate) client"),
        "StationLink must keep its MoonClient private"
    );
}

/// The event classes one of `feed/station.rs`'s filters names, and its body.
fn kept_by(code: &str, filter: &str) -> (BTreeSet<String>, String) {
    let body = code
        .split_once(&format!("pub fn {filter}("))
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\n}"))
        .map(|(body, _)| body.to_string())
        .unwrap_or_else(|| panic!("feed/station.rs must define `pub fn {filter}`"));
    let kept = body
        .split("Event::")
        .skip(1)
        .filter_map(|tail| tail.split('(').next())
        .map(str::to_string)
        .collect();
    (kept, body)
}

/// Breakage guarded: the station mode's event filters letting another class of core event in —
/// the light station anything past reports, the Mini App's station anything past the account its
/// tabs show — each of which starts a store or a queue the station was measured without
/// (STATION.md §3.1, §7.4, §7.7).
#[test]
fn each_station_profile_keeps_its_own_events_alone() {
    let code = code(&workspace().join("crates/moon-core/src/feed/station.rs"));
    let (light, _) = kept_by(&code, "keeps_reports");
    assert_eq!(
        light,
        BTreeSet::from(["Detect", "MarketHistory", "Report", "ServerLog"].map(String::from)),
        "the light station keeps reports, archive answers, the log (for the clock offset) and \
         detects (judged for the bot's Telegram events, never stored) — nothing else"
    );
    let (account, body) = kept_by(&code, "keeps_account");
    assert!(
        body.contains("keeps_reports(event)"),
        "the Mini App's station keeps what the light one keeps"
    );
    assert_eq!(
        account,
        BTreeSet::from(
            [
                "Account",
                "Balance",
                "KernelHealth",
                "Order",
                "Settings",
                "Strat"
            ]
            .map(String::from)
        ),
        "the Mini App's station adds the account its tabs show — nothing else"
    );
}
