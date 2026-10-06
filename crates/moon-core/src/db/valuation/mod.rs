//! Persistent historical quote-to-USDT valuation for report rows.
//!
//! `valuation.sqlite` is a derived cache, separate from the recoverable report replica. It stores
//! immutable closed-minute spot rates and prepared trade values. Readers still compare every
//! stored input with the current report row, so an upsert under the same report identity cannot
//! publish a stale conversion while the worker is catching up.

mod current;
mod health;
mod provider;
mod resolver;
mod worker;

#[cfg(test)]
mod tests;

// `FaultCause` stays crate-private: it is the worker's builder, and a public constructor would let
// anything mint a fault the worker never reported — the opposite of "health is published, not
// derived". `FailureKind` is public only because `ValuationFault.kind` is a public field, so a
// consumer could otherwise read the class without being able to name its type.
pub(crate) use current::current_rate_sql;
use current::publish_current_rates;
pub(crate) use current::{CurrentRate, CurrentRates, FRESHNESS_MS};
pub use current::{RatePin, ValuationMode, pin_current_rates};
pub(crate) use health::FaultCause;
pub use health::{FailureKind, StageHealth, ValuationFault, ValuationStage, ValuationStatus};
pub(crate) use provider::{HttpSpotRateSource, SpotRateSource};
pub use worker::{ValuationHandle, spawn_worker};

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::read_fail::{self, ReadResult};

/// Schema and routing version included in every cached rate and prepared valuation.
///
/// Incrementing this value invalidates old rows without destructive migration when provider
/// routing or conversion semantics change.
pub const ALGORITHM_VERSION: i64 = 2;

/// Attached SQLite schema name used by report and Analytics queries.
pub(crate) const SCHEMA: &str = "valuation";

/// Durable report-side change queue consumed by the valuation worker.
const OUTBOX_TABLE: &str = "valuation_outbox";

/// The canonical derived cache is available to new report readers.
const HEALTHY: u8 = 1;

/// The canonical derived cache must not be attached until recovery succeeds.
const UNHEALTHY: u8 = 2;

/// Process-wide health of the one canonical valuation cache.
static CACHE_HEALTH: AtomicU8 = AtomicU8::new(HEALTHY);

/// Advanced after every write of [`CACHE_HEALTH`], so a reader can tell that attachability may
/// have changed since it last looked even when both looks saw the same health.
static CACHE_HEALTH_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Serializes file-family replacement against attachment proof and validation.
static CACHE_LIFECYCLE: RwLock<()> = RwLock::new(());

/// Process-wide serialization for tests that change canonical cache health.
#[cfg(test)]
static TEST_HEALTH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test guard that restores cache health after a fixture changes global state.
#[cfg(test)]
pub(super) struct TestHealthGuard {
    /// Held solely to serialize global cache-health fixtures.
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestHealthGuard {
    /// Restore the healthy default before another parallel test can attach its fixture.
    fn drop(&mut self) {
        CACHE_HEALTH.store(HEALTHY, Ordering::Release);
    }
}

/// Serialize one fixture that attaches valuation storage and reset its health baseline.
///
/// Returns:
///     Guard holding the test-only global health lock until fixture teardown.
#[cfg(test)]
pub(super) fn test_health_guard() -> TestHealthGuard {
    let guard = TEST_HEALTH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    CACHE_HEALTH.store(HEALTHY, Ordering::Release);
    TestHealthGuard { _lock: guard }
}

mod coverage;
pub(crate) use coverage::*;

mod outbox;
pub(crate) use outbox::*;

mod rate_types;
pub(crate) use rate_types::*;

mod store;
pub use store::attach_epoch;
pub(crate) use store::*;

mod attach;
pub(crate) use attach::*;

mod rate_cache;
pub(crate) use rate_cache::*;
