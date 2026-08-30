//! Keeps every diagnostic switch and every diagnostic log file going through `moon_core::diagnostics`.
//!
//! The point of that module is that the switches are in ONE discoverable, documented place. That
//! property does not survive on good intentions: adding `std::env::var_os("MOON_NEW_DIAG")` to a
//! module is a one-line change that compiles, works for its author, and quietly restores the state
//! this module was built to end — a switch nobody but its author can find. Nothing else in the tree
//! would notice, so these two scans do.
//!
//! Both carry an ALLOW list rather than a blanket exemption, so opting out is a deliberate edit with
//! a stated reason next to it.

use std::path::{Path, PathBuf};

/// Environment variables that may still be read directly, and why.
///
/// These are not observation channels. A channel answers "what is the running program doing"; each
/// of these instead CHANGES what the program does, which is why none of them belongs behind a
/// switch in a settings file a user might flip out of curiosity.
const ENV_ALLOW: &[(&str, &str)] = &[
    (
        "MOON_ANALYTICS_PROBE",
        "also OPENS the Analytics window at startup; a checkbox in a settings file must not open windows",
    ),
    (
        "MOON_RENDER_DIAG_OPEN_FIRST_MARKET",
        "startup automation for a measurement run",
    ),
    (
        "MOON_RENDER_DIAG_OPEN_10_BTC",
        "startup automation for a measurement run",
    ),
    (
        "MOON_RENDER_DIAG_PAUSE_AFTER_OPEN",
        "startup automation for a measurement run",
    ),
    (
        "MOON_RENDER_DIAG_MARKET",
        "chooses the market a measurement run opens",
    ),
    (
        "MOON_ARCHIVE_PROBE",
        "one-off unit probe for archive reach, deliberately separate from the market channel",
    ),
];

/// Relative log-file names that may still be opened by relative path, and why.
///
/// A relative path lands in the process working directory, which is NOT the folder holding the
/// executable whenever the terminal is started from a shell. Reading the stale copy from the other
/// folder has cost real debugging time, so diagnostics files go through
/// `diagnostics::channel_line`, which puts them in `logs/`.
const PATH_ALLOW: &[(&str, &str)] = &[(
    "firetest.log",
    "FireTest writes its verdict before the app is up; moving it would break the documented harness",
)];

fn crate_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut out = Vec::new();
    for krate in ["moon-core", "moon-ui-gpui"] {
        collect(&root.join(krate).join("src"), &mut out);
    }
    assert!(
        out.len() > 100,
        "the scan found only {} files; a broken walk would make this test vacuous",
        out.len()
    );
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether the file owns the diagnostics plumbing itself, and so is allowed to name the variables.
///
/// Narrowed to `moon-core/src/diagnostics/`, the module that DEFINES the switches. A bare
/// `/diagnostics/` test would also exempt `moon-ui-gpui/src/diagnostics/`, which is the debug-window
/// module and has no business reading these variables directly — exactly the kind of file this
/// scan exists to catch.
fn is_the_diagnostics_module(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .contains("moon-core/src/diagnostics/")
}

#[test]
fn no_diagnostic_switch_is_read_straight_from_the_environment() {
    let mut offenders = Vec::new();
    for path in crate_sources() {
        if is_the_diagnostics_module(&path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_no, line) in text.lines().enumerate() {
            if !line.contains("env::var") {
                continue;
            }
            let Some(start) = line.find("\"MOON_") else {
                continue;
            };
            let name: String = line[start + 1..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if !(name.contains("DIAG") || name.contains("PROBE")) {
                continue;
            }
            if ENV_ALLOW.iter().any(|(allowed, _)| *allowed == name) {
                continue;
            }
            offenders.push(format!("{}:{} — {name}", path.display(), line_no + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "a diagnostic switch must be a key in cfg/diagnostics.toml, read through \
         moon_core::diagnostics, not a bare environment variable — otherwise it is undiscoverable \
         and cannot be flipped while the app runs. Add it to the config, or to ENV_ALLOW with a \
         reason:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn no_diagnostic_log_file_is_opened_by_relative_path() {
    let mut offenders = Vec::new();
    for path in crate_sources() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_no, line) in text.lines().enumerate() {
            // Both ways a file gets opened for writing in this tree.
            let Some((start, skip)) = line
                .find(".open(\"")
                .map(|i| (i, 7))
                .or_else(|| line.find("File::create(\"").map(|i| (i, 14)))
            else {
                continue;
            };
            let name: String = line[start + skip..]
                .chars()
                .take_while(|c| *c != '"')
                .collect();
            if !name.ends_with(".log") || name.contains('/') || name.contains('\\') {
                continue;
            }
            if PATH_ALLOW.iter().any(|(allowed, _)| *allowed == name) {
                continue;
            }
            offenders.push(format!("{}:{} — {name}", path.display(), line_no + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "a relative path puts the file in the process working directory, not beside the \
         executable, so the run writes one copy and the reader opens another. Use \
         moon_core::diagnostics::channel_line, or add it to PATH_ALLOW with a reason:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_allow_lists_carry_a_reason_for_every_entry() {
    for (name, reason) in ENV_ALLOW.iter().chain(PATH_ALLOW.iter()) {
        assert!(
            reason.len() > 20,
            "{name} is exempted without a real reason; an allow list without reasons is just a \
             disabled test"
        );
    }
}
