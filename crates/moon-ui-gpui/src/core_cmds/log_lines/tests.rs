//! Parsing regressions must not corrupt copied logs or confuse a login stop with an exception.

use super::{Kind, classify};

/// The timestamp is kept apart from severity, including the server's ISO journal format.
#[test]
fn timestamps_and_severities_preserve_messages() {
    assert_eq!(
        classify("06:34:10 AuthCheck failed: 429"),
        (Some("06:34:10"), Kind::Error, "AuthCheck failed: 429")
    );
    assert_eq!(
        classify("2026-01-02T06:34:10+00:00 WARNING rate limit"),
        (
            Some("2026-01-02T06:34:10+00:00"),
            Kind::Warn,
            "WARNING rate limit"
        )
    );
    assert_eq!(
        classify("2026-01-02 06:34:10 MoonBot started"),
        (Some("2026-01-02 06:34:10"), Kind::Plain, "MoonBot started")
    );
    assert_eq!(
        classify("Login stopped: state login_error"),
        (None, Kind::Warn, "Login stopped: state login_error")
    );
    assert_eq!(
        classify("Out of memory: process killed"),
        (None, Kind::Error, "Out of memory: process killed")
    );
    assert_eq!(
        classify("plain log line"),
        (None, Kind::Plain, "plain log line")
    );
    assert_eq!(classify(""), (None, Kind::Plain, ""));
    assert_eq!(classify("яя:яя:яя"), (None, Kind::Plain, "яя:яя:яя"));
}
