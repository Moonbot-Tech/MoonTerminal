//! Backend queue and session-login methods for core server commands.

mod model;
pub(crate) use model::*;

use crate::Backend;

#[allow(dead_code)] // used from batch 3
impl Backend {
    /// Queue `req` for the command worker associated with core `uid`.
    pub(crate) fn core_cmds_send(&mut self, uid: u64, req: CmdRequest) {
        self.core_cmds.pending.push((uid, req));
    }

    /// Store a confirmed provider login for `ip` for the rest of this terminal session.
    pub(crate) fn core_cmds_add_login(&mut self, ip: String, login: SessionLogin) {
        self.core_cmds.logins.insert(ip, login);
    }
}

#[cfg(test)]
mod tests;
