//! Server-check error messages without opening a connection.

use super::check_failure;
use moon_remote::setup::FirstAccess;
use moon_remote::ssh::OpenError;
use zeroize::Zeroizing;

/// A generic SSH refusal hides which account was tried and where newcomers can find it.
#[test]
fn password_refusal_names_login_and_provider_email() {
    let _locale = crate::test_locale::force("en");
    let first = FirstAccess::Password {
        user: "admin".into(),
        password: Zeroizing::new("synthetic-secret".into()),
    };
    let error = anyhow::Error::new(OpenError::Refused {
        password_offered: true,
    })
    .context("synthetic check context");
    let shown = check_failure(&error, &first);
    assert!(shown.contains("login admin"), "{shown}");
    assert!(shown.contains("VPS provider's email"), "{shown}");
    assert!(!shown.contains("synthetic-secret"));
}

/// Misclassifying other SSH failures as bad credentials would hide host-key security errors.
#[test]
fn other_check_failures_keep_their_diagnostics() {
    let _locale = crate::test_locale::force("en");
    let first = FirstAccess::Password {
        user: "root".into(),
        password: Zeroizing::new("synthetic-secret".into()),
    };
    let error = anyhow::Error::new(OpenError::HostKeyChanged {
        pinned: "synthetic-old".into(),
        presented: "synthetic-new".into(),
    });
    let shown = check_failure(&error, &first);
    assert!(shown.contains("synthetic-old"), "{shown}");
    assert!(shown.contains("synthetic-new"), "{shown}");
    assert!(!shown.contains("VPS provider's email"));
}
