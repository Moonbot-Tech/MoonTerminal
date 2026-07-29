//! Regression tests for Core Status defaults and server-name ordering.
//!
//! The warning detectors (sustained CPU, memory growth) moved to `backend::core_warn`; their tests
//! live beside that engine now.

use super::{CoreStatusMode, natural_cmp};

/// `mod.rs:CoreStatusMode::default` must remain By IP; changing it to Flat makes every newly opened
/// Core Status panel bypass the server overview.
#[test]
fn new_core_status_panels_default_to_by_ip() {
    assert_eq!(CoreStatusMode::default(), CoreStatusMode::ByIp);
}

/// `mod.rs:natural_cmp` sorts server names as a human reads them: numbers by value (so `Server 2`
/// precedes `Server 10`, not the lexical reverse) and custom names alphabetically.
#[test]
fn server_names_sort_naturally() {
    let mut names = [
        "Server 10",
        "Server 2",
        "QQ",
        "F1",
        "Server 1",
        "HLFutures2",
    ];
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(
        names,
        [
            "F1",
            "HLFutures2",
            "QQ",
            "Server 1",
            "Server 2",
            "Server 10"
        ]
    );
}
