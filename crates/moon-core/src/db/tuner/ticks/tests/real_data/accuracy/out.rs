//! Append-only JSON-lines output with explicit flush errors.

use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};

/// Optional durable output; console summaries remain available without a destination.
pub(super) struct Sink(Option<BufWriter<File>>);

impl Sink {
    /// Open the requested artifact without truncating a previous measurement.
    pub(super) fn new() -> Self {
        Self(std::env::var_os("MOON_TICKS_OUT").map(|p| {
            BufWriter::new(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .expect("open measurement output"),
            )
        }))
    }

    /// Serialize one complete record; any failed artifact write fails the measurement.
    pub(super) fn line(&mut self, value: Value) {
        if let Some(writer) = &mut self.0 {
            serde_json::to_writer(&mut *writer, &value).expect("write JSON record");
            writer.write_all(b"\n").expect("write record separator");
        }
    }

    /// Record the revision and pair-admission rule before any result rows.
    pub(super) fn meta(&mut self, rule: &str) {
        self.line(json!({"type":"meta", "model":super::compat::MODEL,
            "sha":std::env::var("MOON_TICKS_SHA").ok(), "pair_ms":super::pair_ms(), "rule":rule}));
    }

    /// Surface buffered I/O errors before the test can report success.
    pub(super) fn finish(mut self) {
        if let Some(writer) = &mut self.0 {
            writer.flush().expect("flush measurement output");
        }
    }
}
