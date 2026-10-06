//! Two-pass caption fit results, keyed before allocating the joined caption.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Borrowed inputs that determine the exact fit or wrap result.
#[derive(Clone, Copy)]
pub(super) struct FitInput<'a> {
    pub prefix: &'a str,
    pub text: &'a str,
    pub budget: f32,
    pub size: f32,
    pub wraps: bool,
}

/// Owned key and result; comparing the key makes hash collisions harmless.
struct Entry {
    prefix: String,
    text: String,
    budget: u32,
    size: u32,
    wraps: bool,
    lines: Vec<(String, f32)>,
}

impl Entry {
    /// Verify all inputs rather than trusting the compact map key.
    fn matches(&self, input: FitInput<'_>) -> bool {
        self.prefix == input.prefix
            && self.text == input.text
            && self.budget == input.budget.to_bits()
            && self.size == input.size.to_bits()
            && self.wraps == input.wraps
    }
}

/// Keeps only captions used in this pass or the immediately preceding pass.
#[derive(Default)]
pub(in crate::chartdx) struct FitMemo {
    cur: HashMap<u64, Entry>,
    prev: HashMap<u64, Entry>,
    stamp: Option<(String, u32, u32)>,
}

impl FitMemo {
    /// Start one frame, expiring unused entries and invalidating changed measurement metrics.
    pub(in crate::chartdx) fn begin_pass(&mut self, family: &str, scale: f32, zoom: f32) {
        if self.stamp.as_ref().is_none_or(|(name, bits, zoom_bits)| {
            name != family || *bits != scale.to_bits() || *zoom_bits != zoom.to_bits()
        }) {
            self.cur.clear();
            self.prev.clear();
            self.stamp = Some((family.to_owned(), scale.to_bits(), zoom.to_bits()));
        }
        std::mem::swap(&mut self.cur, &mut self.prev);
        self.cur.clear();
    }

    /// Return the production algorithm's owned result, measuring only on a verified miss.
    pub(super) fn lookup(
        &mut self,
        input: FitInput<'_>,
        measure: impl FnMut(&str, f32) -> f32,
    ) -> Vec<(String, f32)> {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        (
            input.prefix,
            input.text,
            input.budget.to_bits(),
            input.size.to_bits(),
            input.wraps,
        )
            .hash(&mut hash);
        let key = hash.finish();
        if let Some(entry) = self.cur.get(&key).filter(|entry| entry.matches(input)) {
            return entry.lines.clone();
        }
        if self
            .prev
            .get(&key)
            .is_some_and(|entry| entry.matches(input))
        {
            let entry = self.prev.remove(&key).expect("verified previous entry");
            let lines = entry.lines.clone();
            self.cur.insert(key, entry);
            return lines;
        }
        let glued = format!("{}{}", input.prefix, input.text);
        let measure = std::cell::RefCell::new(measure);
        let width = |text: &str| measure.borrow_mut()(text, input.size);
        let lines = if input.wraps {
            crate::design::wrap_text(
                &glued,
                input.budget,
                moon_core::config::LABEL_WRAP_LINES,
                width,
            )
        } else {
            vec![crate::design::fit_text(&glued, input.budget, width)]
        };
        self.cur.insert(
            key,
            Entry {
                prefix: input.prefix.to_owned(),
                text: input.text.to_owned(),
                budget: input.budget.to_bits(),
                size: input.size.to_bits(),
                wraps: input.wraps,
                lines: lines.clone(),
            },
        );
        lines
    }
}

#[cfg(test)]
mod tests;
