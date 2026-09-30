//! moon-remote: prepares a Linux server for the station over SSH, from the terminal's machine.
//!
//! The mechanics of `docs-internal/STATION.md` §4.6 and §5.1, without the UI yet: the `moon-remote`
//! binary drives them from a command line; the terminal's "Server" page will call the same
//! functions. The server-side half is two POSIX scripts under `remote/`, carried inside this
//! crate so the binary that runs a setup is the one whose scripts run on the server.

pub mod app_key;
pub mod hosts;
pub mod keys;
pub mod script;
pub mod setup;
pub mod ssh;
pub mod station;
