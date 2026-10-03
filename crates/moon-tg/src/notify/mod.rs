//! Notification decisions and the owner-thread tick that applies them.
//!
//! `decide`, `due`, and `DownTracker::step` stay pure. [`tick`] reads the report
//! replica and writes the durable outbox.

pub(crate) mod daily;
pub(crate) mod down;
pub(crate) mod render;
pub(crate) mod tick;
pub(crate) mod trades;
