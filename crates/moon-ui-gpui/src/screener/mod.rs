//! Screener window: a Moonbot-style coin table.
//!
//! A separate singleton OS window, following the Strategies-window pattern, hosts a `MoonDataTable`
//! for every market on connected exchanges. Headers control sorting, while Market and Vol. filters
//! sit in the footer. Multi-core data is grouped by the market-data provider returned by
//! `MarketDataSource::provider_of`: cores on one exchange share a provider in deduplicated mode,
//! while per-core mode keeps separate providers. Market columns are read once per provider group;
//! Orders, Session, and Pos aggregate the group's member cores. Leverage is hybrid: the provider
//! supplies `max_leverage`, while member accounts supply the maximum active `leverage_x` and its
//! associated `isolated` state. Rows come from moon-core's `MarketDataSource::screener_rows`.
//!
//! [`view`] owns state, window rendering, and [`open`]; [`table`] owns the column schema, formatting,
//! sorting, and row rendering.

mod table;
mod view;

pub use view::open;
