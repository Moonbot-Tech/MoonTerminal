//! MoonTerminal backend core — the UI-agnostic core of the terminal.
//!
//! Contains everything independent of the rendering engine (GPUI/DX11): the data stream
//! from the Moonbot core, multiple-core support and market-data deduplication, configuration
//! with secrets, the local reports database, and domain types. It communicates with the UI
//! through `feed::FeedMsg` (core → UI) and `feed::CoreCmd` (UI → core); the UI never calls
//! the transport (moonproto) directly.
//!
//! Used by the `moonterminal` GPUI shell (which depends on the core, not vice versa). The old
//! `moon-terminal` egui shell has been removed.

pub mod alert_blob;
pub mod applog;
mod backup_store;
pub mod backups;
pub mod coin_naming;
pub mod config;
pub mod data;
pub mod db;
pub mod detect_diag;
pub mod diagnostics;
pub mod feed;
pub mod figures;
pub mod fixture;
pub mod hl_diag;
pub mod market;
pub mod metrics;
pub mod order_diag;
pub mod palette;
pub mod session;
pub mod strat_db;
pub mod symbol;
pub mod update;
pub mod util;
pub mod venue;
