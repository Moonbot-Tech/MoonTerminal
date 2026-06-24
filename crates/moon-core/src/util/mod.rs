//! Общие утилиты, не привязанные к конкретному модулю.

pub mod fmt;
pub mod time;

pub use time::{now_unix_ms, now_unix_ms_i64};
