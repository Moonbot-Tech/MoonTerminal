//! The running station's control API (`moon_core::station_api`), reached through the helper's
//! `ctl`: the request goes on the SSH channel's stdin, the station's own binary relays it to the
//! service's socket and prints the reply.

use crate::error::StationError;
use anyhow::Context;
use moon_core::station_api::{Access, Answer, Hello, PROTO_VERSION, PairingCode, Reply, Request};

use super::{admin_conn, current_helper_status};
use crate::script::{self, STEP_TIMEOUT};
use crate::ssh::{Conn, Target};

/// One request on an open administrator connection whose helper is current.
///
/// The station's hello is judged before its reply is read: a station of another API version is
/// refused by name — update whichever side is older — never misread.
pub(crate) fn call(conn: &Conn, request: &Request) -> anyhow::Result<Answer> {
    let body = serde_json::to_vec(request)?;
    let out = script::checked(conn.run(&script::helper("ctl", &[]), &body, STEP_TIMEOUT)?)?;
    let output: serde_json::Value =
        serde_json::from_str(out.stdout_text().trim()).context("read the station's answer")?;
    let hello: Hello =
        serde_json::from_value(output["hello"].clone()).context("read the station's hello")?;
    // Whichever side is older is the one to update: a station set up by a newer terminal is not
    // fixed by updating it again.
    anyhow::ensure!(
        hello.proto_version == PROTO_VERSION,
        if hello.proto_version > PROTO_VERSION {
            StationError::TerminalTooOld
        } else {
            StationError::ServiceTooOld
        }
    );
    match serde_json::from_value(output["reply"].clone()).context("read the station's reply")? {
        Reply::Ok(answer) => Ok(answer),
        Reply::Err(reason) => anyhow::bail!("the station: {reason}"),
    }
}

/// A fresh ten-minute pairing code from the station's bot, for one more chat.
pub fn issue_pairing(target: &Target) -> anyhow::Result<PairingCode> {
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    match call(&conn, &Request::PairIssue)? {
        Answer::Pairing(code) => Ok(code),
        other => anyhow::bail!("the station answered a pairing request with {other:?}"),
    }
}

/// Replace the station's paired chats with `access`, only while they are still `base`.
pub fn set_access(target: &Target, base: &Access, access: &Access) -> anyhow::Result<Access> {
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    let request = Request::AccessSet {
        base: base.clone(),
        access: access.clone(),
    };
    match call(&conn, &request)? {
        Answer::Access(saved) => Ok(saved),
        other => anyhow::bail!("the station answered a change of chats with {other:?}"),
    }
}
