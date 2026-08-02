//! Drives the real logging chain — `log::` macro -> `TeeLogger` -> ring buffer — and asserts that
//! no network address survives it.
//!
//! The unit tests beside `applog::redact` check the scanner in isolation; this one checks that the
//! scanner is actually WIRED IN. Removing the redaction from `push`, from `TeeLogger::log`, or
//! installing the inner `env_logger` directly would leave those tests green and this one failing.

use moon_core::applog;

/// The lines are the shapes that reach us from outside: the moonproto resolve error (which formats
/// the endpoint itself) and a core log line carrying the machine it runs on.
#[test]
fn addresses_logged_by_dependencies_never_reach_the_buffer() {
    // Files belong to the running application, not to a test run; the ring is what we inspect.
    applog::set_file_logging(false, 0);
    // Same installer the application uses, so the chain under test is the chain that ships.
    let env = env_logger::Builder::new()
        .parse_filters("moon_core=info,log_redaction=info")
        .build();
    applog::install(env).expect("no other logger is installed in this test binary");

    log::error!("server address resolve failed for 10.0.0.7:3011: timed out");
    log::warn!("[UDP_Cmd 172.110.223.179:10042]: BAD HMAC");
    log::info!("bound [::1]:3000 ok");
    log::info!("live connect core=BB1 market=BTCUSDT transport=V0");

    let lines: Vec<String> = applog::snapshot(16).into_iter().map(|l| l.msg).collect();
    assert!(
        lines.len() >= 4,
        "the wrapper must buffer accepted records: {lines:?}"
    );
    for line in &lines {
        assert!(
            !line.contains("10.0.0.7")
                && !line.contains("172.110.223.179")
                && !line.contains("::1]:3000")
                && !line.contains(":3011"),
            "address survived the logging chain: {line}"
        );
    }
    // The masked lines must still be readable diagnostics, and an address-free line untouched.
    assert!(lines.iter().any(|l| l.contains("BAD HMAC")));
    assert!(lines
        .iter()
        .any(|l| l == "live connect core=BB1 market=BTCUSDT transport=V0"));
}
