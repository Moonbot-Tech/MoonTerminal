//! Moving a MoonBot core to a Linux server: everything about it that needs no SSH.
//!
//! - [`key_packet`] — the connection packet: the core keeps its MoonProto key, only the endpoint
//!   changes, so every terminal that knows the core changes just the address;
//! - [`report_db`] — the core's report database rebuilt from this terminal's replica, with the
//!   identity terminals resume by;
//! - [`archive`] — the settings archive the new core starts from;
//! - [`snapshot`] — the settings snapshot taken straight from the running core, the same ZIP
//!   MoonBot's VPS wizard saves;
//! - [`strategies`] — the old core's strategy set written to the new one over MoonProto.
//!
//! The server side (install, start, roll back) lives in `moon-remote`.

pub mod archive;
pub mod key_packet;
pub mod report_db;
pub mod snapshot;
pub mod strategies;
