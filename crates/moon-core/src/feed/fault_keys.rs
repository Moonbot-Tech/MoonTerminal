//! Which short locale key names a core connection fault — one table for every surface.
//!
//! Two surfaces state a fault in a word or two: the desktop, from the classified verdict
//! ([`FailureClass`], `conn_diag`), and the Telegram Mini App, from the raw fault the core link
//! recorded ([`ConnFaultKind`]). They start from different facts — the verdict also knows packet
//! and byte counts, the raw fault does not — so each keeps its own selection, but both select from
//! the SAME `core_status.fault.short.*` keys, and both selections live here, side by side.
//!
//! This crate does not localize; the caller turns the key into text.

use super::{ConnFaultKind, FailureClass};

/// Locale key of the short label for one classified verdict.
///
/// Args:
///     class: The classified failure.
///
/// Returns:
///     A `core_status.fault.short.*` key.
pub fn failure_short_key(class: &FailureClass) -> &'static str {
    match class {
        FailureClass::KeyUnparsable { empty: true } => "core_status.fault.short.key_empty",
        FailureClass::KeyUnparsable { empty: false } => "core_status.fault.short.key_unparsable",
        FailureClass::Endpoint { unresolved: false } => "core_status.fault.short.endpoint_invalid",
        FailureClass::Endpoint { unresolved: true } => {
            "core_status.fault.short.endpoint_unresolved"
        }
        FailureClass::LocalPort { .. } => "core_status.fault.short.local_port",
        FailureClass::NoResponse {
            packets_received: 0,
            bytes: 0,
            ..
        } => "core_status.fault.short.no_response",
        FailureClass::NoResponse { .. } => "core_status.fault.short.unparsed",
        FailureClass::Access { .. } => "core_status.fault.short.access",
        FailureClass::CoreUnidentified { .. } => "core_status.fault.short.unidentified",
        FailureClass::Syncing { stalled: false, .. } => "core_status.fault.short.syncing",
        FailureClass::Syncing { stalled: true, .. } => "core_status.fault.short.stalled",
        FailureClass::Aborted => "core_status.fault.short.aborted",
        FailureClass::Undetermined { .. } => "core_status.fault.short.unknown",
    }
}

/// Stable snake_case kind for one [`ConnFaultKind`], the value the Mini App page receives.
///
/// `KeyUnparsable` splits on `empty` because the Core Status panel already words those two
/// facts apart. The other variants keep one key each; step and packet-count forks stay in the
/// desktop verdict and are not part of this closed set.
///
/// Args:
///     kind: The raw fault.
///
/// Returns:
///     One kind from [`FAULT_KIND_SHORT_KEYS`].
pub fn fault_kind(kind: &ConnFaultKind) -> &'static str {
    match kind {
        ConnFaultKind::KeyUnparsable { empty: true } => "key_empty",
        ConnFaultKind::KeyUnparsable { empty: false } => "key_unparsable",
        ConnFaultKind::EndpointUnusable { unresolved: false } => "endpoint_invalid",
        ConnFaultKind::EndpointUnusable { unresolved: true } => "endpoint_unresolved",
        ConnFaultKind::LocalBindFailed { .. } => "local_bind_failed",
        ConnFaultKind::Aborted => "aborted",
        ConnFaultKind::ConnectTimedOut { .. } => "connect_timed_out",
        ConnFaultKind::NotAuthenticated => "not_authenticated",
        ConnFaultKind::InitStepTimedOut { .. } => "init_step_timed_out",
        ConnFaultKind::StartupStalled => "startup_stalled",
        ConnFaultKind::InitStepFailed { .. } => "init_step_failed",
    }
}

/// Each [`fault_kind`] paired with the Core Status short label it is shown with.
pub const FAULT_KIND_SHORT_KEYS: &[(&str, &str)] = &[
    ("key_empty", "core_status.fault.short.key_empty"),
    ("key_unparsable", "core_status.fault.short.key_unparsable"),
    (
        "endpoint_invalid",
        "core_status.fault.short.endpoint_invalid",
    ),
    (
        "endpoint_unresolved",
        "core_status.fault.short.endpoint_unresolved",
    ),
    ("local_bind_failed", "core_status.fault.short.local_port"),
    ("aborted", "core_status.fault.short.aborted"),
    ("connect_timed_out", "core_status.fault.short.no_response"),
    ("not_authenticated", "core_status.fault.short.access"),
    ("init_step_timed_out", "core_status.fault.short.stalled"),
    ("startup_stalled", "core_status.fault.short.stalled"),
    ("init_step_failed", "core_status.fault.short.unknown"),
];

#[cfg(test)]
mod tests;
