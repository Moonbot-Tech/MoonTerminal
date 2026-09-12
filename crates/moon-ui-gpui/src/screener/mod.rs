//! Screener window: a Moonbot-style coin table.
//!
//! A separate singleton OS window, following the Strategies-window pattern, hosts a `MoonDataTable`
//! for every market on connected exchanges. Headers control sorting, while Market and Vol. filters
//! sit in the footer. Multi-core data is grouped by the market-data provider returned by
//! `MarketDataSource::provider_of`: cores on one exchange share a provider in deduplicated mode,
//! while per-core mode keeps separate providers. Market columns are read once per provider group;
//! every member core then gets its own row per market, with its own Orders, PnL, Session, Pos and
//! active `leverage_x`/`isolated` (the provider supplies `max_leverage`). The Core column names that
//! core, and a chart opens on it. Rows come from moon-core's `MarketDataSource::screener_rows`.
//!
//! [`view`] owns state, window rendering, and [`open`]; [`table`] owns the column schema, formatting,
//! sorting, and row rendering.

mod table;
mod view;

pub use view::open;
