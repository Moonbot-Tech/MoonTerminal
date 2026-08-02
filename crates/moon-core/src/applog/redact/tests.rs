use super::*;

/// The line the terminal itself used to emit, and the moonproto error that still does: an address
/// with a port must leave nothing but the mask, while the core name around it survives.
#[test]
fn an_address_with_a_port_is_replaced_whole() {
    assert_eq!(
        addresses("live connect 192.168.1.10:3000 core=BB1"),
        "live connect <addr> core=BB1"
    );
    assert_eq!(
        addresses("server address resolve failed for 10.0.0.7:3011: timed out"),
        "server address resolve failed for <addr>: timed out"
    );
}

/// Redacting must not depend on a port following, and must handle several addresses per line.
#[test]
fn bare_and_repeated_addresses_are_replaced() {
    assert_eq!(
        addresses("peer 10.1.2.3 -> peer 10.1.2.4:65535 done"),
        "peer <addr> -> peer <addr> done"
    );
}

/// Regression target: the core writes `key:value` and ranges without spaces, and a boundary rule
/// that treats `:` or `-` as part of the previous token silently skips the address after it.
#[test]
fn an_address_after_a_colon_or_hyphen_is_still_replaced() {
    assert_eq!(
        addresses("host:10.0.0.7 unreachable"),
        "host:<addr> unreachable"
    );
    assert_eq!(
        addresses("range 10.0.0.1-10.0.0.9 scanned"),
        "range <addr>-<addr> scanned"
    );
    assert_eq!(addresses("[10.0.0.1]:80 refused"), "<addr> refused");
}

/// Regression target: a colon rule loose enough for `db::add` or `feed::live` redacts Rust paths,
/// which open a large share of the log lines this project writes.
#[test]
fn rust_paths_are_left_alone() {
    for line in [
        "db::add called",
        "feed::live wants a snapshot",
        "dead::beef is a word, not a host",
        "moon_core::feed::live connected",
    ] {
        assert!(
            matches!(addresses(line), Cow::Borrowed(_)),
            "must not redact: {line}"
        );
    }
}

/// Regression target: masking `0..=255` blindly would eat build/version strings and any dotted
/// identifier that merely looks like a quad.
#[test]
fn versions_and_over_range_quads_are_left_alone() {
    for line in [
        "build: moonterminal=v8.1.2.3 moonui=1.2.3",
        "MoonBot 8.1.2.3.4 started",
        "ratio 300.1.2.3 out of range",
        "rev a1.2.3.4",
    ] {
        assert!(
            matches!(addresses(line), Cow::Borrowed(_)),
            "must not redact: {line}"
        );
    }
}

/// Regression target: an IPv6 rule loose enough to accept `HH:MM:SS` — or the four-field
/// `D:HH:MM:SS` uptime and colon-separated ids the core emits — would redact them as addresses.
#[test]
fn clocks_and_durations_are_left_alone() {
    for line in [
        "12:34:56.789 INFO ready",
        "elapsed 1:05 remaining 0:42",
        "sync took 00:00:03",
        "uptime 1:02:03:04",
        "elapsed 0:00:00:05 ok",
        "id abc:def:012:345:678 seen",
    ] {
        assert!(
            matches!(addresses(line), Cow::Borrowed(_)),
            "must not redact: {line}"
        );
    }
}

/// IPv6 in the forms a dual-stack socket actually reports: compressed, bracketed with a port, and
/// the IPv4-mapped tail, which a colon scan alone would leave half-redacted.
#[test]
fn ipv6_forms_are_replaced_whole() {
    assert_eq!(addresses("bound fe80::1 ok"), "bound <addr> ok");
    assert_eq!(addresses("bound [::1]:3000 ok"), "bound <addr> ok");
    assert_eq!(addresses("peer ::ffff:192.168.1.1 up"), "peer <addr> up");
    assert_eq!(
        addresses("peer 2001:0db8:85a3:0000:0000:8a2e:0370:7334 up"),
        "peer <addr> up"
    );
    // A zone id names the local interface; `[::]` is the unspecified bind address.
    assert_eq!(addresses("iface fe80::1%eth0 up"), "iface <addr> up");
    assert_eq!(addresses("bound [::]:1024"), "bound <addr>");
    // A MAC identifies the machine just as an address does.
    assert_eq!(addresses("mac aa:bb:cc:dd:ee:ff"), "mac <addr>");
    // An all-letter address is still an address; only the compressed form needs the digit guard.
    assert_eq!(
        addresses("peer cafe:babe:dead:beef:cafe:babe:dead:beef"),
        "peer <addr>"
    );
}

/// Regression target: a dot after an address opens the `::ffff:1.2.3.4` mapped tail, and treating a
/// failed tail as "not an address" left the whole literal in clear at the end of a sentence.
#[test]
fn an_address_ending_a_sentence_is_replaced() {
    assert_eq!(
        addresses("connected to fe80::1. next"),
        "connected to <addr>. next"
    );
    assert_eq!(addresses("peer 2001:db8::1."), "peer <addr>.");
    assert_eq!(
        addresses("host 10.0.0.7. retrying"),
        "host <addr>. retrying"
    );
}

/// The real shapes this project's own logs carry, taken verbatim from `logs/*_<core>.log` written
/// before this change. Each must come back masked, with the surrounding diagnostics intact.
#[test]
fn live_core_log_shapes_are_replaced() {
    assert_eq!(
        addresses("[UDP_Cmd 172.110.223.179:10042]: BAD HMAC"),
        "[UDP_Cmd <addr>]: BAD HMAC"
    );
    assert_eq!(
        addresses("DNS 1.1.1.1 Resolving dapi.binance.com error"),
        "DNS <addr> Resolving dapi.binance.com error"
    );
}

/// A line without an address must come back borrowed: this path runs on every core log line, and an
/// allocation per line is the cost the fast path exists to avoid.
#[test]
fn a_clean_line_is_borrowed() {
    let line = "core BB1 place order BTCUSDT short=false price=64000.5 size=0.01";
    assert!(matches!(addresses(line), Cow::Borrowed(_)));
}

/// Non-ASCII text surrounds addresses in this project's logs; slicing by byte index must not split a
/// multi-byte character.
#[test]
fn redaction_survives_non_ascii_neighbours() {
    assert_eq!(
        addresses("ядро «БиБи1» → 172.16.0.9:3000 — соединение потеряно"),
        "ядро «БиБи1» → <addr> — соединение потеряно"
    );
}
