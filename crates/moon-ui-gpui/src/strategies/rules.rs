//! Strategy-field dependency rules that determine whether a field is editable or a section is
//! active from the values of OTHER fields. Rules come from `assets/param_deps.toml`
//! (`"Field" = "A=VAL;B<>VAL"`), parsed and evaluated by [`FieldDeps`] in the core crate, where a
//! model without this window reads them too. This module keeps the window's loading: startup
//! first tries the external development path, then uses the bundled fallback. External-file hot
//! reload requires the explicit `MOON_STRATEGY_RULES_HOT_RELOAD` environment variable; production
//! does not poll the filesystem every second.
//!
//! Section activity has no separate configuration: `section_active` evaluates these dependencies
//! and keeps a section active when more than one of its fields is dependency-active.

use std::time::SystemTime;

use moon_core::feed::strategy_deps::{EXTERNAL, FieldDeps};

pub use moon_core::feed::strategy_deps::Values;

pub struct Rules {
    /// The parsed rules.
    deps: FieldDeps,
    /// Modification time of the external file, used for hot reload.
    mtime: Option<SystemTime>,
}

impl Rules {
    /// Load rules from the external file when present, otherwise from the bundled fallback.
    pub fn load() -> Self {
        let rules = match std::fs::read_to_string(EXTERNAL) {
            Ok(content) => Rules {
                deps: FieldDeps::parse(&content),
                mtime: file_mtime(),
            },
            Err(_) => Rules {
                deps: FieldDeps::bundled(),
                mtime: None,
            },
        };
        rules.log_count();
        rules
    }

    /// One line per load, as the window always logged it.
    fn log_count(&self) {
        log::info!(
            "strategy param rules: {} fields with dependencies",
            self.deps.len()
        );
    }

    /// Reload the external file when it changes, returning true when a new frame is needed.
    pub fn reload_if_changed(&mut self) -> bool {
        let m = file_mtime();
        if m.is_some()
            && m != self.mtime
            && let Ok(content) = std::fs::read_to_string(EXTERNAL)
        {
            self.mtime = m;
            self.deps = FieldDeps::parse(&content);
            self.log_count();
            return true;
        }
        false
    }

    /// Return whether a field is active and editable under the current values
    /// ([`FieldDeps::field_active`]).
    pub fn field_active(&self, name: &str, values: &Values) -> bool {
        self.deps.field_active(name, values)
    }
}

/// Return the external rules file's modification time, or None when the file is absent.
fn file_mtime() -> Option<SystemTime> {
    std::fs::metadata(EXTERNAL)
        .ok()
        .and_then(|m| m.modified().ok())
}
