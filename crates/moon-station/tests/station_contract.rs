//! The station's mode, pinned in source (`docs-internal/STATION.md` §3.1, level 3).
//!
//! What the station may ask a core is decided by which calls exist in its code, so the guard reads
//! the code: the station crate itself, the tape recorder it runs, and the narrow client that
//! recorder owns. A red test here is a station that can now do something it was never meant to —
//! subscribe to every market, trade, or hold a raw `MoonClient` past the narrow link.
//!
//! Comments are stripped before anything is searched, so a doc line that NAMES a forbidden call
//! (explaining why it is forbidden) is not a violation.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

fn station_sources() -> Vec<(PathBuf, String)> {
    rust_files(&workspace().join("crates/moon-station/src"))
        .into_iter()
        .map(|path| {
            let code = code(&path);
            (path, code)
        })
        .collect()
}

/// Breakage guarded: a station component that talks to a core through its own `MoonClient` —
/// and with it every call moonproto has, trading and `subscribe_all_trades` included — instead of
/// the terminal's feed in station mode or the narrow `StationLink`.
#[test]
fn the_station_crate_never_touches_moonproto_directly() {
    for (path, code) in station_sources() {
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
            "{} names `TradeLink`: trading belongs to `tg::trade` alone (STATION.md §3.1)",
            path.display()
        );
    }
}

/// The `SessionManager` calls the station makes: start the feeds, drain them, map each core to
/// itself as its market source, sum the connection status, and reconcile on a reload. Anything
/// else it would call on the session — a command to a core, a trading call — lands here first.
const SESSION_CALLS: [&str; 4] = [
    "conn_summary_group",
    "drain",
    "map_cores_to_themselves",
    "reconcile",
];

/// Breakage guarded: the station growing a call into the terminal's session beyond the ones
/// its mode was measured with (STATION.md §7.14) — each new one is a decision, made here.
#[test]
fn the_station_calls_only_its_share_of_the_session() {
    let allowed: BTreeSet<String> = SESSION_CALLS.iter().map(|s| s.to_string()).collect();
    for (path, code) in station_sources() {
        let called = methods_called_on(&code, "session", true);
        let extra: Vec<_> = called.difference(&allowed).collect();
        assert!(
            extra.is_empty(),
            "{} calls {extra:?} on the session; the station's share is {SESSION_CALLS:?} — add \
             a call here only as a deliberate change of the station's mode",
            path.display()
        );
    }
}

/// Breakage guarded: the tape recorder — the one station component with a client of its own —
/// building a raw `MoonClient` again, or subscribing its donors to every market (the core then
/// streams the exchange AND the client retains every market's rings: +586 MB in a minute,
/// STATION.md §4.3).
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

/// Breakage guarded: the station mode's event filter letting another class of core event in —
/// balances, orders, strategies — each of which starts a store or a queue the station was
/// measured without (STATION.md §3.2, §7.14).
#[test]
fn the_station_keeps_reports_archives_and_the_log_alone() {
    let code = code(&workspace().join("crates/moon-core/src/feed/station.rs"));
    let keeps = code
        .split_once("pub fn keeps(")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\n}"))
        .map(|(body, _)| body)
        .expect("feed/station.rs must define `pub fn keeps`");
    let kept: BTreeSet<&str> = keeps
        .split("Event::")
        .skip(1)
        .filter_map(|tail| tail.split('(').next())
        .collect();
    assert_eq!(
        kept,
        BTreeSet::from(["MarketHistory", "Report", "ServerLog"]),
        "the station keeps reports, archive answers and the log (for the clock offset) — nothing \
         else"
    );
}
