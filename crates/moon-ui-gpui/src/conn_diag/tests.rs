//! Unit tests for the connection-verdict wording. Owned by the breakage gate, which authors
//! every deterministic test in this repository.

use super::*;

/// Physical inbound datagrams must not reuse either the silence sentence or its firewall advice.
///
/// Breakage: keying the verdict only off accepted Sliced bytes tells an operator to open a port
/// after the socket has already proved that packets reached the terminal.
#[test]
fn unparsed_datagrams_do_not_use_the_silent_wording() {
    let silent = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 0,
        bytes: 0,
        elapsed_ms: 12_000,
    };
    let unparsed = FailureClass::NoResponse {
        packets_sent: 9,
        packets_received: 73,
        bytes: 0,
        elapsed_ms: 12_000,
    };

    assert_ne!(reason(&silent), reason(&unparsed));
    assert_ne!(next_step(&silent), next_step(&unparsed));
    assert_ne!(fault_short(&silent), fault_short(&unparsed));
    assert!(reason(&unparsed).contains("73"));
}
