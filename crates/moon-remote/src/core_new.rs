//! A fresh MoonBot core on a Linux server, set up through the core's own command line.
//!
//! Since the L6 package a running core takes its first settings from `mb` commands: MoonProto
//! (`--setup-moonproto`, which hands back the connection key), the exchange (`--exchanges`,
//! `--exchange`), the API keys (`--set-api-keys`, JSON on stdin only) and a new login
//! (`--retry-login`); `--status` reports where it stands. They work before the core is logged in
//! to its exchange and without a restart. Nothing here writes the core's settings file or reads
//! its log.
//!
//! The package and the service go on through the same server-side script as a moved core
//! (`remote/moonbot-move.sh`, step `install-fresh`), so [`crate::core_move::check`] and
//! [`crate::core_move::remove`] serve both. Reconfiguring commands reach only an instance that
//! script made; hand-installed cores accept only status and login retry. A command runs under the
//! account the core runs as, the only one its command channel answers.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::core_move::{INSTALL_TIMEOUT, Installed, command};
use crate::script::{self, Privilege, STEP_TIMEOUT};
use crate::ssh::Conn;

/// How often [`wait_settled`] asks the core again.
const POLL: Duration = Duration::from_secs(2);
/// One status ask: the core answers it at once; the rest is the SSH round trip.
const STATUS_TIMEOUT: Duration = Duration::from_secs(20);
/// Failed status asks in a row [`wait_settled`] lets pass — about ten seconds' worth.
const STATUS_FAILURES: u32 = 5;
/// How long a login usually takes to settle: the exchange login plus MoonBot's registration.
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(180);

/// One exchange of `mb --exchanges`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Exchange {
    /// What `mb --exchange` takes, e.g. `binance-futures`.
    pub id: String,
    /// The name MoonBot shows, e.g. `Binance Futures`.
    pub name: String,
    /// The passphrase the key must be created with on the exchange (Bitget, OKX), empty when the
    /// exchange asks none. The core uses this value itself: the user only has to set it there.
    #[serde(default)]
    pub required_passphrase: String,
    /// The exchange takes an RSA key bundle as well as a key pair (Binance).
    #[serde(default)]
    pub rsa_supported: bool,
}

/// `state` of `mb --status`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreState {
    Initializing,
    Ready,
    /// The selected exchange has no keys yet.
    NeedsKeys,
    /// The exchange refused the keys; `login_error` says why.
    LoginError,
    /// MoonBot's registration of the key failed; no login without it.
    RegistrationError,
    /// The user has to do something on the exchange first (approve the Hyperliquid builder fee).
    ActionRequired,
    /// A state this build does not know yet.
    Other(String),
}

impl From<String> for CoreState {
    /// Read the wire name; an unknown one is kept, not refused, so a newer core still reports.
    fn from(name: String) -> Self {
        match name.as_str() {
            "initializing" => Self::Initializing,
            "ready" => Self::Ready,
            "needs_keys" => Self::NeedsKeys,
            "login_error" => Self::LoginError,
            "registration_error" => Self::RegistrationError,
            "action_required" => Self::ActionRequired,
            _ => Self::Other(name),
        }
    }
}

impl<'de> Deserialize<'de> for CoreState {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Self::from)
    }
}

/// `mb --status`: where the core stands.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CoreStatus {
    pub state: CoreState,
    /// The selected exchange's id, as `--exchanges` lists it.
    #[serde(default)]
    pub exchange: String,
    #[serde(default)]
    pub exchange_name: String,
    #[serde(default)]
    pub has_api_keys: bool,
    /// The exchange's own refusal, when `state` is a login error.
    #[serde(default)]
    pub login_error: String,
    #[serde(default)]
    pub moonproto_enabled: bool,
    /// The core's MoonProto listener runs on the server. Whether it can be reached from outside
    /// (a provider's firewall) only a connection tells.
    #[serde(default)]
    pub moonproto_listening: bool,
    #[serde(default)]
    pub moonproto_error: String,
    #[serde(default)]
    pub moonproto_port: u16,
    /// The emulator mode; reported only by builds newer than L6, `None` before.
    #[serde(default)]
    pub emulator: Option<bool>,
}

impl CoreStatus {
    /// Logged in and listening: a terminal can connect now.
    pub fn connectable(&self) -> bool {
        self.state == CoreState::Ready && self.moonproto_listening
    }

    /// The core stopped on something only the user can change.
    pub fn needs_user(&self) -> bool {
        matches!(
            self.state,
            CoreState::LoginError | CoreState::RegistrationError | CoreState::ActionRequired
        )
    }
}

/// A refusal or unreadable reply returned by an established core command channel.
#[derive(Debug)]
pub struct CoreRefusal {
    reason: String,
}

impl CoreRefusal {
    /// Preserve the core's own safe failure description for the command window.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for CoreRefusal {
    /// Show the core's reason rather than classifying it as an SSH transport failure.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for CoreRefusal {}

/// The exchange keys `mb --set-api-keys` takes. They travel as JSON on stdin only.
pub enum ApiKeys {
    Pair {
        api_key: Zeroizing<String>,
        api_secret: Zeroizing<String>,
    },
    /// Binance's RSA keys, as MoonBot exports them.
    Rsa { bundle: Zeroizing<String> },
}

/// The JSON `--set-api-keys` reads.
#[derive(Serialize)]
struct KeysJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_secret: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rsa_bundle: Option<&'a str>,
}

impl ApiKeys {
    /// The stdin of `--set-api-keys`, in a buffer that is wiped when dropped.
    pub(crate) fn json(&self) -> Zeroizing<Vec<u8>> {
        let body = match self {
            Self::Pair {
                api_key,
                api_secret,
            } => KeysJson {
                api_key: Some(api_key.trim()),
                api_secret: Some(api_secret.trim()),
                rsa_bundle: None,
            },
            Self::Rsa { bundle } => KeysJson {
                api_key: None,
                api_secret: None,
                rsa_bundle: Some(bundle.trim()),
            },
        };
        // Reserved up front, so the serializer never reallocates and leaves a copy behind.
        let size = match self {
            Self::Pair {
                api_key,
                api_secret,
            } => api_key.len() + api_secret.len(),
            Self::Rsa { bundle } => bundle.len(),
        };
        let mut buf = Zeroizing::new(Vec::with_capacity(2 * size + 64));
        // Writing into a Vec cannot fail.
        let _ = serde_json::to_writer(&mut *buf, &body);
        buf
    }
}

/// Install the package and the service in `dir` (a taken name gets `-2`, `-3`…), open `proto_port`
/// in an active ufw, and start the core on its default settings. Returns once the core answers
/// its commands. A failure leaves nothing behind but what every instance shares: the
/// `moonbot@.service` template and the `mb` command in `/usr/local/bin`.
pub fn install(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    proto_port: u16,
    package_url: &str,
) -> anyhow::Result<Installed> {
    let port = proto_port.to_string();
    let out = privilege.run(
        conn,
        &command("install-fresh", &[dir, &port, package_url]),
        &[],
        INSTALL_TIMEOUT,
    )?;
    let text = out.stdout_text();
    let dir = script::value(&text, "MOVE_DIRECTORY")
        .ok_or_else(|| anyhow::anyhow!("the install did not report its directory: {text}"))?;
    let unit = script::value(&text, "MOVE_SERVICE")
        .ok_or_else(|| anyhow::anyhow!("the install did not report its service: {text}"))?;
    Ok(Installed {
        dir: dir.to_owned(),
        unit: unit.to_owned(),
    })
}

/// Turn on the core's MoonProto server for `host:port` and return its connection key. The core
/// makes the key and keeps it: asked again, it hands back the same one.
pub fn setup_moonproto(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    host: Ipv4Addr,
    port: u16,
) -> anyhow::Result<Zeroizing<String>> {
    let host = host.to_string();
    let port = port.to_string();
    let reply = mb(
        conn,
        privilege,
        dir,
        &["--setup-moonproto", "--host", &host, "--port", &port],
        &[],
    )?;
    let key = reply
        .get("key")
        .and_then(|key| key.as_str())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| anyhow::anyhow!("the core did not hand back a connection key"))?;
    Ok(Zeroizing::new(key.to_owned()))
}

/// The exchanges the core can trade on.
pub fn exchanges(conn: &Conn, privilege: &Privilege, dir: &str) -> anyhow::Result<Vec<Exchange>> {
    let reply = mb(conn, privilege, dir, &["--exchanges"], &[])?;
    parse_exchanges(reply)
}

/// Select exchange `id` with the keys stored for it, if any. Stops the core's strategies and
/// logs out of the previous exchange, as MoonBot's own switch does. Since 7.73 every exchange
/// keeps its own MoonProto settings: selecting one configured before brings its own back, so the
/// connection key may differ from the previous exchange's — read it again after the switch.
pub fn select_exchange(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    id: &str,
) -> anyhow::Result<()> {
    mb(conn, privilege, dir, &["--exchange", id], &[]).map(drop)
}

/// Give the selected exchange its keys; the core logs in with them by itself.
pub fn set_api_keys(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    keys: &ApiKeys,
) -> anyhow::Result<()> {
    mb(conn, privilege, dir, &["--set-api-keys"], &keys.json()).map(drop)
}

/// Log in again with the stored keys — after the user fixed the key's IP list or rights.
pub fn retry_login(conn: &Conn, privilege: &Privilege, dir: &str) -> anyhow::Result<()> {
    mb(conn, privilege, dir, &["--retry-login"], &[]).map(drop)
}

/// Where the core stands.
pub fn status(conn: &Conn, privilege: &Privilege, dir: &str) -> anyhow::Result<CoreStatus> {
    let reply = mb_within(conn, privilege, dir, &["--status"], &[], STATUS_TIMEOUT)?;
    Ok(serde_json::from_value(reply)?)
}

/// Ask [`status`] until the core is done initializing — on `exchange`, when given — or `timeout`
/// passed, and return that status. The core refuses an exchange switch, keys and MoonProto setup
/// while it initializes ("Core is busy"), and an accepted switch is not a finished one: right
/// after it the status may still describe the previous exchange, which is why the exchange id is
/// compared, not only the state.
///
/// Failed asks are tolerated as in [`wait_settled`]; a timeout is an error here, since every
/// caller goes on with a command the core would refuse.
pub fn wait_initialized(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    exchange: Option<&str>,
    timeout: Duration,
) -> anyhow::Result<CoreStatus> {
    let start = Instant::now();
    let mut failures = 0;
    loop {
        match status(conn, privilege, dir) {
            Ok(now) => {
                failures = 0;
                if initialized_on(&now, exchange) {
                    return Ok(now);
                }
                if start.elapsed() >= timeout {
                    anyhow::bail!(
                        "the core is still initializing after {} s ({:?} on {})",
                        timeout.as_secs(),
                        now.state,
                        now.exchange
                    );
                }
            }
            Err(e) => {
                failures += 1;
                if failures >= STATUS_FAILURES || start.elapsed() >= timeout {
                    return Err(e);
                }
                log::debug!("core status in {dir} not read, asking again: {e:#}");
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Whether `status` is past initialization, on `exchange` when one is asked for.
pub(crate) fn initialized_on(status: &CoreStatus, exchange: Option<&str>) -> bool {
    status.state != CoreState::Initializing && exchange.is_none_or(|id| status.exchange == id)
}

/// Switch to exchange `id` and bring its MoonProto server up at `host:port`, returning that
/// exchange's connection key — the order MoonBot's Linux guide gives: the exchange first, MoonProto
/// once the switch finished. The key is never carried over from the previous exchange: a fresh
/// exchange gets the address, one configured before keeps its own key (`--setup-moonproto` without
/// `--rotate-key`).
///
/// An exchange already selected is not selected again: that would only restart its login, and a
/// status read before the restart begins would already pass the wait for it.
pub fn switch_exchange(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    id: &str,
    host: Ipv4Addr,
    port: u16,
) -> anyhow::Result<Zeroizing<String>> {
    if status(conn, privilege, dir).map_or(true, |now| now.exchange != id) {
        select_exchange(conn, privilege, dir, id)?;
    }
    wait_initialized(conn, privilege, dir, Some(id), SETTLE_TIMEOUT)?;
    setup_moonproto(conn, privilege, dir, host, port)
}

/// Ask [`status`] until the core is [`CoreStatus::connectable`], [`CoreStatus::needs_user`], or
/// `timeout` passed; `seen` gets every answer. Returns the last one — the caller tells a settled
/// core from one that is still on its way.
///
/// A failed ask is asked again, up to [`STATUS_FAILURES`] in a row: a core switching exchanges
/// may leave its command channel unanswered for a moment. More in a row is not a moment, and that
/// failure is returned.
pub fn wait_settled(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    timeout: Duration,
    seen: &mut dyn FnMut(&CoreStatus),
) -> anyhow::Result<CoreStatus> {
    let start = Instant::now();
    let mut failures = 0;
    loop {
        match status(conn, privilege, dir) {
            Ok(now) => {
                seen(&now);
                failures = 0;
                if now.connectable() || now.needs_user() || start.elapsed() >= timeout {
                    return Ok(now);
                }
            }
            Err(e) => {
                failures += 1;
                if failures >= STATUS_FAILURES || start.elapsed() >= timeout {
                    return Err(e);
                }
                log::debug!("core status in {dir} not read, asking again: {e:#}");
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Run one `mb` command in `dir` and return its JSON reply.
pub(crate) fn mb(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    args: &[&str],
    stdin: &[u8],
) -> anyhow::Result<serde_json::Value> {
    mb_within(conn, privilege, dir, args, stdin, STEP_TIMEOUT)
}

/// [`mb`] with its own deadline.
pub(crate) fn mb_within(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    args: &[&str],
    stdin: &[u8],
    timeout: Duration,
) -> anyhow::Result<serde_json::Value> {
    let mut all = vec![dir];
    all.extend_from_slice(args);
    let out = privilege.run_raw(conn, &command("mb", &all), stdin, timeout)?;
    parse_reply(out.ok(), &out.stdout_text(), &out.stderr_text())
}

/// The JSON reply of an `mb` command, or its refusal. The reply is the last line that is a JSON
/// object. An error carries the reply's own `error` or stderr, never stdout as a whole: the
/// reply of `--setup-moonproto` carries the connection key.
pub(crate) fn parse_reply(
    exited_ok: bool,
    stdout: &str,
    stderr: &str,
) -> anyhow::Result<serde_json::Value> {
    let reply = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok());
    let said = |reply: Option<&serde_json::Value>| {
        reply
            .and_then(|r| r.get("error"))
            .and_then(|e| e.as_str())
            .filter(|e| !e.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| stderr.trim().to_owned())
    };
    match reply {
        Some(reply) if exited_ok && reply.get("ok") == Some(&serde_json::Value::Bool(true)) => {
            Ok(reply)
        }
        Some(reply) => {
            Err(CoreRefusal::new(format!("the core refused: {}", said(Some(&reply)))).into())
        }
        None if stderr.trim().is_empty() => {
            anyhow::bail!("the core gave no reply ({} bytes of output)", stdout.len())
        }
        None => anyhow::bail!("the core gave no reply: {}", said(None)),
    }
}

/// The `exchanges` list of a `--exchanges` reply.
pub(crate) fn parse_exchanges(reply: serde_json::Value) -> anyhow::Result<Vec<Exchange>> {
    let list = reply
        .get("exchanges")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("the core listed no exchanges"))?;
    Ok(serde_json::from_value(list)?)
}

#[cfg(test)]
mod tests;
