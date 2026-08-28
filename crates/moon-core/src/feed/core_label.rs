//! Core identity for log lines: the id AND the name the core carried at that moment.
//!
//! Log text used to identify a core by `CoreId` alone, which reads fine while writing the code and
//! badly while reading a log — an id is only resolvable against the configuration as it was then,
//! and the configuration changes. The registry here maps id to the CURRENT name, so a line formatted
//! through [`core_label`] carries both, and a name edited later does not rewrite what was logged.
//!
//! The registry is written when a session starts or its configuration is replaced, and read from
//! every feed thread while formatting, so it is a plain `RwLock` map: writes are rare, reads are on
//! the log path only.

use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

use crate::session::CoreId;

/// Live id → name map. Missing means the session has not started (or has stopped).
static NAMES: OnceLock<RwLock<HashMap<CoreId, String>>> = OnceLock::new();

fn names() -> &'static RwLock<HashMap<CoreId, String>> {
    NAMES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Records the name a core is running under, replacing any previous one.
pub fn set_core_name(id: CoreId, name: &str) {
    if let Ok(mut map) = names().write() {
        map.insert(id, name.to_string());
    }
}

/// Drops a core's name once its session is gone, so a later id reuse cannot inherit it.
pub fn clear_core_name(id: CoreId) {
    if let Ok(mut map) = names().write() {
        map.remove(&id);
    }
}

/// Formats `id` together with the core's current name, for use in a log message.
///
/// Renders as `19 «BB1»`, or as the bare id when no name is registered — during early startup, or
/// after the session stopped.
pub fn core_label(id: CoreId) -> CoreLabel {
    CoreLabel { id }
}

/// Display wrapper produced by [`core_label`]; resolves the name when the message is formatted, so
/// the line records the name in force at that instant.
pub struct CoreLabel {
    id: CoreId,
}

impl fmt::Display for CoreLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Formatted UNDER the read guard rather than through a cloned name: this runs on the
        // connection-churn path — once per lifecycle event, per core — and a whole config of cores
        // flapping at once would otherwise be one heap allocation each. Nothing reachable from a
        // `Formatter` reads this map again, so holding it here cannot re-enter.
        let Ok(map) = names().read() else {
            return write!(f, "{}", self.id);
        };
        match map.get(&self.id) {
            Some(name) => write!(f, "{} «{name}»", self.id),
            None => write!(f, "{}", self.id),
        }
    }
}

#[cfg(test)]
mod tests;
