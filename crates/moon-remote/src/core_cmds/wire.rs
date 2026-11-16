//! Data and pure parsers for the server-side core command wire protocol.

use crate::script;

/// The instance directory and systemd unit that own a core's MoonProto UDP port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    /// Absolute instance directory resolved on the server.
    pub dir: String,
    /// Systemd unit that owns the core process.
    pub unit: String,
    /// Whether the instance carries MoonTerminal's matching installation marker.
    pub ours: bool,
    /// Whether the instance has an executable `mb` command wrapper.
    pub has_mb: bool,
    /// Whether the core is active, or `None` when activity was not part of the reply.
    pub active: Option<bool>,
    /// Whether the resolved installation is confirmed to use the requested MoonProto port.
    pub port_known: bool,
}

/// The possible outcomes of resolving a core by its MoonProto UDP port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocateResult {
    /// One running or uniquely identified stopped core.
    Found(Located),
    /// Stopped hand-installed cores awaiting port confirmation or unit selection.
    Candidates(Vec<Located>),
    /// No core or no complete location reply.
    None,
}

/// A failed service operation, with its journal tail kept out of `Display` for the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceFailed {
    /// The last journal lines printed by the server-side service step.
    pub tail: Vec<String>,
}

impl std::fmt::Display for ServiceFailed {
    /// Describe the failure without exposing journal output in ordinary error logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the service operation failed")
    }
}

impl std::error::Error for ServiceFailed {}

/// Service state and resource measurements for a located core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    /// The resolved instance and unit.
    pub located: Located,
    /// Systemd's current `ActiveState` value.
    pub active: String,
    /// Seconds since the service became active, when systemd has a timestamp.
    pub uptime_s: Option<u64>,
    /// Current service memory in bytes, when systemd reports it.
    pub memory: Option<u64>,
}

/// A supported systemd service state change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    /// The action name accepted by the server-side script.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

/// The bounded log tail sizes accepted by the server-side script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLines {
    L200,
    L500,
    L1000,
}

impl LogLines {
    /// The number of lines requested from the newest log.
    pub const fn count(self) -> u16 {
        match self {
            Self::L200 => 200,
            Self::L500 => 500,
            Self::L1000 => 1000,
        }
    }
}

/// Raw lines from a core log, without the wire protocol's marker or command output prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogTail {
    pub lines: Vec<String>,
}

/// One daily trading log a core still has on its server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogDay {
    /// The server's own calendar day, `YYYY-MM-DD`, as the core names the file.
    pub date: String,
    /// Size of the file in bytes at the moment it was listed.
    pub bytes: u64,
}

/// What the server answered when asked for one day's trading log.
#[derive(Debug, PartialEq, Eq)]
pub enum LogDayReply<'a> {
    /// The core has no file for that day.
    Absent,
    /// The whole file, gzipped, exactly as it followed the marker line.
    Gzip(&'a [u8]),
    /// The file is larger than the limit sent with the request; nothing of it was transferred.
    TooBig(u64),
}

/// A day's log that reached this machine and cannot be used: cut short, not what was asked for, or
/// over [`LOG_DAY_MAX_BYTES`].
///
/// Typed apart from a transport failure, because the connection is fine and reopening it — what a
/// caller does on a transport failure — would only repeat the same answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogDayUnusable(pub String);

impl std::fmt::Display for LogDayUnusable {
    /// The reason, in words fit for the window that shows it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LogDayUnusable {}

/// Largest unpacked day accepted, so a damaged or hostile stream cannot fill this machine's memory.
/// A busy core writes tens of megabytes a day; this leaves an order of magnitude above that.
pub const LOG_DAY_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// A command the terminal may request for a core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    Status,
    RetryLogin,
    Log,
    Restart,
    Stop,
    Start,
    SwitchExchange,
    ApiKeys,
    ConnectionKey,
    MoonProtoAddress,
    UpdateCore,
    /// List the core's daily trading logs, or pull one of them onto this computer.
    LogDay,
}

/// Why a located core cannot perform a requested command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Reconfiguration is restricted to instances installed by MoonTerminal.
    NotOurs,
    /// The core has no executable command wrapper.
    NoMb,
}

/// Parse a `locate` reply, retaining every stopped hand-installed candidate.
pub fn parse_locate(text: &str) -> LocateResult {
    let candidates: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix("LOCATE_CANDIDATE="))
        .filter_map(parse_candidate)
        .collect();
    if !candidates.is_empty() {
        return LocateResult::Candidates(candidates);
    }
    if script::value(text, "LOCATE_NONE") == Some("1") {
        return LocateResult::None;
    }
    let Some(dir) = script::value(text, "LOCATE_DIRECTORY") else {
        return LocateResult::None;
    };
    let Some(unit) = script::value(text, "LOCATE_SERVICE") else {
        return LocateResult::None;
    };
    let Some(ours) = script::value(text, "LOCATE_OURS").and_then(parse_flag) else {
        return LocateResult::None;
    };
    let Some(has_mb) = script::value(text, "LOCATE_MB").and_then(parse_flag) else {
        return LocateResult::None;
    };
    LocateResult::Found(Located {
        dir: dir.to_owned(),
        unit: unit.to_owned(),
        ours,
        has_mb,
        active: None,
        port_known: true,
    })
}

/// Parse one stopped candidate's unit, directory, `mb` flag and port confirmation.
fn parse_candidate(text: &str) -> Option<Located> {
    let (unit, rest) = text.split_once('|')?;
    let (rest, port_known) = rest.rsplit_once('|')?;
    let (dir, has_mb) = rest.rsplit_once('|')?;
    if unit.is_empty() || dir.is_empty() {
        return None;
    }
    Some(Located {
        dir: dir.to_owned(),
        unit: unit.to_owned(),
        ours: false,
        has_mb: parse_flag(has_mb)?,
        active: Some(false),
        port_known: parse_flag(port_known)?,
    })
}

/// Parse an `info` reply, retaining absent uptime or memory as `None`.
pub fn parse_info(text: &str) -> Option<ServiceInfo> {
    let LocateResult::Found(mut located) = parse_locate(text) else {
        return None;
    };
    let active = script::value(text, "INFO_ACTIVE")?.to_owned();
    located.active = Some(active == "active");
    let number = |key| script::value(text, key).and_then(|value| value.parse().ok());
    Some(ServiceInfo {
        located,
        active,
        uptime_s: number("INFO_UPTIME"),
        memory: number("INFO_MEMORY"),
    })
}

/// Parse the `SERVICE_OK` state from a successful service operation.
pub fn parse_service(text: &str) -> anyhow::Result<String> {
    script::value(text, "SERVICE_OK")
        .filter(|state| !state.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("the service operation did not report its state"))
}

/// Drop command output through `LOG_BEGIN` and retain only the raw log lines.
pub fn parse_log(text: &str) -> LogTail {
    LogTail {
        lines: text
            .lines()
            .skip_while(|line| *line != "LOG_BEGIN")
            .skip(1)
            .map(str::to_owned)
            .collect(),
    }
}

/// Whether `date` has the `YYYY-MM-DD` shape the server-side script accepts as a log day.
///
/// The shape is the whole check: the date becomes part of a path on the server, and a month the
/// calendar does not have simply names a file that is not there.
pub fn is_log_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// Parse a `logdays` reply into the days the core still has, newest first.
///
/// Lines before the marker, and entries that are not a calendar day with a byte count, are dropped:
/// the listing fills a menu the user picks from, and a name that is not a day cannot be pulled.
pub fn parse_log_days(text: &str) -> Vec<LogDay> {
    let mut days: Vec<LogDay> = text
        .lines()
        .skip_while(|line| *line != "LOGDAYS_BEGIN")
        .skip(1)
        .filter_map(|line| line.strip_prefix("LOGDAY="))
        .filter_map(|entry| {
            let (date, bytes) = entry.split_once(' ')?;
            let bytes = bytes.parse().ok()?;
            is_log_date(date).then(|| LogDay {
                date: date.to_owned(),
                bytes,
            })
        })
        .collect();
    days.sort_by(|a, b| b.date.cmp(&a.date));
    days
}

/// Split a `logday` reply into "no such day", "over the limit", or the gzipped file that follows
/// the marker line.
///
/// The marker is looked for at the start of a line only, and the first one wins: everything after
/// it is binary and may contain any byte sequence, the marker's own included.
pub fn split_log_day(stdout: &[u8]) -> Option<LogDayReply<'_>> {
    const BEGIN: &[u8] = b"LOGDAY_BEGIN\n";
    const NONE: &[u8] = b"LOGDAY_NONE=1\n";
    const TOO_BIG: &[u8] = b"LOGDAY_TOO_BIG=";
    let mut line = 0;
    loop {
        let rest = stdout.get(line..)?;
        if rest.starts_with(BEGIN) {
            return Some(LogDayReply::Gzip(&rest[BEGIN.len()..]));
        }
        if rest.starts_with(NONE) {
            return Some(LogDayReply::Absent);
        }
        if let Some(size) = rest.strip_prefix(TOO_BIG) {
            let end = size.iter().position(|byte| *byte == b'\n')?;
            let size = std::str::from_utf8(&size[..end]).ok()?.parse().ok()?;
            return Some(LogDayReply::TooBig(size));
        }
        line += rest.iter().position(|byte| *byte == b'\n')? + 1;
    }
}

/// Unpack one day's log, refusing a stream that is cut short or larger than [`LOG_DAY_MAX_BYTES`].
///
/// A connection that drops mid-transfer leaves a gzip stream without its trailer, which the decoder
/// reports as an error — so half a day is never returned as the day.
pub fn gunzip_log_day(gzip: &[u8]) -> Result<Vec<u8>, LogDayUnusable> {
    use std::io::Read;

    let mut day = Vec::new();
    flate2::read::GzDecoder::new(gzip)
        .take(LOG_DAY_MAX_BYTES + 1)
        .read_to_end(&mut day)
        .map_err(|error| LogDayUnusable(format!("the log arrived damaged: {error}")))?;
    if day.len() as u64 > LOG_DAY_MAX_BYTES {
        return Err(LogDayUnusable(format!(
            "the log is larger than {} MB",
            LOG_DAY_MAX_BYTES / (1024 * 1024)
        )));
    }
    Ok(day)
}

/// Refuse commands that the located installation is not allowed to perform.
pub fn gate(located: &Located, cmd: Cmd) -> Result<(), Refusal> {
    if matches!(
        cmd,
        Cmd::SwitchExchange
            | Cmd::ApiKeys
            | Cmd::ConnectionKey
            | Cmd::MoonProtoAddress
            | Cmd::UpdateCore
    ) && !located.ours
    {
        return Err(Refusal::NotOurs);
    }
    if matches!(
        cmd,
        Cmd::Status
            | Cmd::RetryLogin
            | Cmd::SwitchExchange
            | Cmd::ApiKeys
            | Cmd::ConnectionKey
            | Cmd::MoonProtoAddress
    ) && !located.has_mb
    {
        return Err(Refusal::NoMb);
    }
    Ok(())
}

/// Read a wire boolean, rejecting values outside the frozen `0|1` contract.
fn parse_flag(value: &str) -> Option<bool> {
    match value {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
