//! General utilities not tied to a specific module.

pub mod display_time;
pub mod fmt;
pub mod hash;
pub mod time;

pub use hash::fnv1a64;
pub use time::{civil_from_days, now_unix_ms, now_unix_ms_i64, unix_ms_i64_of, utc_stamp_compact};
