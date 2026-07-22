//! General utilities not tied to a specific module.

pub mod fmt;
pub mod time;

pub use time::{civil_from_days, now_unix_ms, now_unix_ms_i64, utc_stamp_compact};
