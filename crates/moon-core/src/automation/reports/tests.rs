//! An empty authorization must never reach the report layer's empty-means-all convention.

use super::*;

/// Empty core selection would otherwise invoke the report layer's all-core default.
#[test]
fn empty_scope_is_rejected_before_opening_the_database() {
    let error = query(Vec::new(), 1, 1000, false, 0).expect_err("empty scope");
    assert!(error.to_string().contains("explicitly authorized"));
}
