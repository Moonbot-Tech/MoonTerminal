//! The running station's control API (`moon_core::station_api`), reached through the helper's
//! `ctl`: the request goes on the SSH channel's stdin, the station's own binary relays it to the
//! service's socket and prints the reply.

use crate::error::StationError;
use anyhow::Context;
use moon_core::config::CoreGroup;
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
        base: Box::new(base.clone()),
        access: Box::new(access.clone()),
    };
    match call(&conn, &request)? {
        Answer::Access(saved) => Ok(saved),
        other => anyhow::bail!("the station answered a change of chats with {other:?}"),
    }
}

/// Make `zone` the zone the station's reports are cut in, live: the chats as read now go back
/// unchanged with it.
///
/// Returns:
///     The station's access after the change; `Ok(None)` from a station that predates the bot's
///     settings, which keeps the zone of its `station.toml` and is not changed.
pub fn set_zone(target: &Target, zone: &str) -> anyhow::Result<Option<Access>> {
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    let read = match call(&conn, &Request::AccessGet)? {
        Answer::Access(access) => access,
        other => anyhow::bail!("the station answered a read of chats with {other:?}"),
    };
    if read.bot.is_none() {
        return Ok(None);
    }
    // The zone alone: the chats and the menu go back as read, the chats' notifications not at
    // all — sent back they would be saved again over any change made since the read.
    let read = Access {
        notify: None,
        ..read
    };
    let request = Request::AccessSet {
        access: Box::new(Access {
            zone: Some(zone.to_owned()),
            ..read.clone()
        }),
        base: Box::new(read),
    };
    match call(&conn, &request)? {
        Answer::Access(saved) => Ok(Some(saved)),
        other => anyhow::bail!("the station answered a change of zone with {other:?}"),
    }
}

/// Send the terminal's saved core groups to the station, for the bot's report by groups: the
/// station's set is replaced whole. Only on the user's word — another terminal's groups are not
/// overwritten by one that has none.
///
/// Returns:
///     The station's access after the change, or `None` for a station that predates the groups
///     (its read carries none) — nothing is sent to it.
pub fn set_groups(target: &Target, groups: &[CoreGroup]) -> anyhow::Result<Option<Access>> {
    let conn = admin_conn(target)?;
    current_helper_status(&conn)?;
    let read = match call(&conn, &Request::AccessGet)? {
        Answer::Access(access) => access,
        other => anyhow::bail!("the station answered a read of chats with {other:?}"),
    };
    if read.groups.is_none() {
        return Ok(None);
    }
    // The groups alone, as `set_zone` sends the zone alone.
    let read = Access {
        notify: None,
        ..read
    };
    let request = Request::AccessSet {
        access: Box::new(Access {
            groups: Some(groups.to_vec()),
            ..read.clone()
        }),
        base: Box::new(read),
    };
    match call(&conn, &request)? {
        Answer::Access(saved) => Ok(Some(saved)),
        other => anyhow::bail!("the station answered a change of groups with {other:?}"),
    }
}
