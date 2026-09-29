//! The terminal's venue captions: [`moon_core::venue::caption`] with this crate's translator.
//!
//! The caption rules — directory name first, the reported name only for an unknown ordinal, the
//! HIP-3 DEX suffix, the display bounds — live in `moon-core`, so any crate captions a venue the
//! same way. `t!` is per crate, so this crate binds its own dictionary here; nothing else is
//! decided in this file.

use std::borrow::Cow;

use moon_core::feed::ExchangeId;
use moon_core::venue::CoreVenue;
use moon_core::venue::caption;
use rust_i18n::t;

/// Translate one caption key through the terminal's dictionary.
fn tr(key: &str) -> Cow<'_, str> {
    t!(key)
}

/// Caption for one identified core's venue ([`caption::venue_label`]).
pub(crate) fn venue_label(venue: &CoreVenue) -> String {
    caption::venue_label(venue, &tr)
}

/// Caption for one exchange section, unidentified group included ([`caption::venue_section_label`]).
pub(crate) fn venue_section_label(venue: Option<&CoreVenue>) -> String {
    caption::venue_section_label(venue, &tr)
}

/// Caption for a venue known only by its identity ([`caption::venue_id_label`]).
pub(crate) fn venue_id_label(id: ExchangeId) -> String {
    caption::venue_id_label(id, &tr)
}

#[cfg(test)]
mod tests;
