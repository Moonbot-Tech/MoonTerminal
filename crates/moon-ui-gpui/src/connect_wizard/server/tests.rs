//! Provider login normalization regressions for the shared server step.

use super::effective_login;

/// Rejecting empty input again would make the root placeholder unusable for newcomers.
#[test]
fn empty_login_uses_root() {
    assert_eq!(effective_login(""), Some("root"));
    assert_eq!(effective_login(" \n\t"), Some("root"));
}

/// Keeping pasted padding would reject valid provider credentials or send the wrong login.
#[test]
fn provider_login_trims_surrounding_whitespace() {
    assert_eq!(effective_login("root\n"), Some("root"));
    assert_eq!(effective_login(" admin "), Some("admin"));
}

/// Removing all whitespace would silently turn an invalid login into a different account.
#[test]
fn provider_login_rejects_inner_whitespace() {
    for login in ["ro ot", "ro\not", "ad\tmin"] {
        assert_eq!(effective_login(login), None);
    }
}
