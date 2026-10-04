//! Notification decisions and the owner-thread tick that applies them.
//!
//! `decide` and `DownTracker::step` stay pure. [`tick`] reads the report
//! replica and writes the durable outbox; [`reports`] does the same for automatic reports.

pub(crate) mod charts;
pub(crate) mod down;
pub(crate) mod events;
pub(crate) mod render;
pub(crate) mod reports;
#[cfg(test)]
pub(crate) mod test_host;
pub(crate) mod tick;
pub(crate) mod trades;
