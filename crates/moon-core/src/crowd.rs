//! What the crowd is doing, in numbers.
//!
//! The public statistics service publishes every closed trade of everybody who agreed to be
//! counted, plus two boards for the rolling day. This module is all of it: the wire it arrives on
//! ([`feed`]), the figures it adds up to ([`Minute`], [`Standing`]), the boards as they stand
//! ([`CoinDay`], [`Trader`]), and the rule that says when one coin's minute is worth announcing
//! ([`Detector`]).
//!
//! The split inside is deliberate and load-bearing. Everything except [`feed`] is arithmetic over
//! a stream of trades — no socket, no clock of its own, no window — so it is testable without any
//! of them, and [`SyntheticFeed`] gives the whole module something to run on with no network at
//! all. Only [`feed`] knows there is a service.
//!
//! Nothing identifying goes out and nothing identifying comes back: these are public pages, read
//! the way a browser reads them.
//!
//! Money is `f64` — dollars straight off the wire. Time is milliseconds on the reader's clock.

pub mod board;
pub mod detect;
pub mod feed;
pub mod minute;
pub mod rng;
pub mod standing;
pub mod trade;

pub use board::{CoinDay, DaySummary, Trader};
pub use detect::{CrowdDetect, CrowdRule, Detector};
pub use feed::{Feed, FeedConfig, STAT_ORIGIN, Wants, Wire};
pub use minute::{CoinMinute, Minute};
pub use rng::Rng;
pub use standing::Standing;
pub use trade::{EventSource, SyntheticFeed, Trade};
