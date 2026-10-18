//! Planner assertions shared by the catch-up contract fixtures.

use super::*;

/// Whether missing tape includes an archive portion at the fixture's clock.
pub(crate) fn needs_archive(job: &CatchupJob, now_ms: i64) -> bool {
    let (_, _, archive, old) = portions(job, now_ms);
    archive.is_some() && !old.is_empty()
}
