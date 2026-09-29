//! Display names of report cores, resolved from the live configuration by `core_uid`.
//!
//! The report replica stores `core_name` as a copy taken when each trade was downloaded, so a core
//! renamed later would show its old name on old trades and its new one on new trades. The uid is
//! permanent and never reused, so the configured name keyed by it is the one name a core has now.
//! The stored copy stays the fallback for a core that is no longer configured.

use std::collections::BTreeMap;

/// Current display name of every configured core, keyed by its runtime uid.
///
/// Empty means "no configuration known": every read then serves the stored names unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreNames(BTreeMap<u64, String>);

impl CoreNames {
    /// Build the map from `(uid, name)` pairs; a blank name keeps the stored one for that core.
    ///
    /// Args:
    ///     pairs: Configured cores as `(runtime uid, current name)`.
    ///
    /// Returns:
    ///     The name map.
    pub fn from_pairs<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (u64, S)>,
        S: Into<String>,
    {
        Self(
            pairs
                .into_iter()
                .map(|(uid, name)| (uid, name.into().trim().to_string()))
                .filter(|(_, name)| !name.is_empty())
                .collect(),
        )
    }

    /// Return the name to display for one row.
    ///
    /// Args:
    ///     uid: The row's `core_uid`.
    ///     stored: The row's stored `core_name`.
    ///
    /// Returns:
    ///     The configured name, or `stored` when the core is not configured.
    pub fn resolve<'a>(&'a self, uid: u64, stored: &'a str) -> &'a str {
        self.0.get(&uid).map_or(stored, String::as_str)
    }

    /// SQL for the displayed core name of a row of the aliased source.
    ///
    /// One expression serves every projection and `ORDER BY` of the name, so a sort can never key on
    /// a different name than the one printed.
    ///
    /// Args:
    ///     alias: Table alias of the report source.
    ///
    /// Returns:
    ///     A `CASE` over `core_uid` falling back to the stored column, or the plain column when the
    ///     map is empty.
    pub(in crate::db) fn sql(&self, alias: &str) -> String {
        let stored = format!("{alias}.\"core_name\"");
        if self.0.is_empty() {
            return stored;
        }
        let mut sql = format!("CASE {alias}.\"core_uid\"");
        for (uid, name) in &self.0 {
            // Names are user text: a quote is doubled, the only escape an SQL string literal has.
            sql.push_str(&format!(" WHEN {uid} THEN '{}'", name.replace('\'', "''")));
        }
        sql.push_str(&format!(" ELSE {stored} END"));
        sql
    }
}

#[cfg(test)]
mod tests;
