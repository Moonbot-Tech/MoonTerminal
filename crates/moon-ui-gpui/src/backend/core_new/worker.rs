//! The threads of a new core's install: the host-key probe, and the install itself, which waits
//! for the user between its steps.

use std::net::Ipv4Addr;
use std::sync::mpsc;

use moon_core::diagnostics::CORE_MOVE_TARGET;
use moon_remote::core_move;
use moon_remote::core_new::{self, ApiKeys, CoreState, CoreStatus};
use moon_remote::key_only::{self, Closed};
use moon_remote::script::Privilege;
use moon_remote::setup::{self, FirstAccess};
use moon_remote::ssh::{Conn, Target};
use rust_i18n::t;

use super::{Event, NewPhase};

#[cfg(test)]
mod tests;

/// What the user's form gives an install.
pub(crate) struct NewForm {
    pub(crate) target: Target,
    /// The host key the user confirmed.
    pub(crate) pin: String,
    /// The provider's login: held in memory while the install lives — every step logs in
    /// again — and never stored.
    pub(crate) first: FirstAccess,
    /// The server's public address, the one the core is reached at.
    pub(crate) public_ip: Ipv4Addr,
    /// The instance folder on the server.
    pub(crate) dir: String,
    /// The core's MoonProto UDP port.
    pub(crate) proto_port: u16,
    /// Start the core on MoonBot's demo settings: it logs in to MoonBot's demo exchange account by
    /// itself, in the emulator, so the install goes on to the connection without asking for keys.
    pub(crate) demo: bool,
    /// Turn the server's password logins off once the core is installed; set only with a key
    /// login in `first`.
    pub(crate) close_password: bool,
}

/// Turn the server's password logins off behind the key login `first`, and word the outcome as a
/// progress line. It never fails its caller: a server left open is said so, and the core stays.
///
/// Returns `None` for a password login — it has no key to come back in with, so nothing is
/// closed.
pub(crate) fn close_password_line(
    conn: &Conn,
    privilege: &Privilege,
    target: &Target,
    first: &FirstAccess,
    pin: &str,
) -> Option<String> {
    let FirstAccess::Key { user, key, .. } = first else {
        return None;
    };
    let line = match key_only::close(conn, privilege, target, user, key, pin) {
        Ok(Closed::Now) => t!("connect_wizard.line.password_closed"),
        Ok(Closed::Already) => t!("connect_wizard.line.password_already_closed"),
        Err(e) => t!(
            "connect_wizard.line.password_not_closed",
            reason = format!("{e:#}")
        ),
    };
    Some(line.to_string())
}

/// The user's next step.
pub(crate) enum NewCommand {
    /// Install the core and turn on its MoonProto server.
    Install,
    /// Select `exchange` and give it `keys`; the core logs in.
    SetKeys { exchange: String, keys: ApiKeys },
    /// Log in again with the stored keys.
    Retry,
    /// Take the install out of the server again.
    Remove,
}

/// Read the server's host key and report it; sends no credential.
pub(super) fn probe(target: Target, tx: &mpsc::Sender<Event>) {
    let event = match setup::probe_host_key(target) {
        Ok(key) => Event::Probed {
            target: key.target,
            fingerprint: key.fingerprint,
        },
        Err(e) => {
            Event::Failed(t!("connect_wizard.probe_failed", reason = format!("{e:#}")).to_string())
        }
    };
    let _ = tx.send(event);
}

/// Check the server, then carry out the user's commands until the install is removed or the
/// backend drops it.
pub(super) fn run(form: NewForm, tx: &mpsc::Sender<Event>, commands: &mpsc::Receiver<NewCommand>) {
    let send = |event: Event| {
        let _ = tx.send(event);
    };
    match check(&form, &send) {
        Ok(found) => send(Event::Checked(found)),
        Err(e) => {
            send(Event::Failed(check_failure(&e, &form.first)));
            return;
        }
    }
    let mut dir: Option<String> = None;
    // A dropped channel is the backend forgetting the install: the login goes with this thread.
    while let Ok(command) = commands.recv() {
        let result = match (&command, dir.as_deref()) {
            (NewCommand::Install, None) => match install(&form, &send) {
                Ok(done) => {
                    // A demo core is past its login already: nothing is asked of the user.
                    let connected = if form.demo {
                        connect_demo(&form, &done, &send)
                    } else {
                        Ok(())
                    };
                    dir = Some(done);
                    connected
                }
                Err(Installing::Nothing(e)) => {
                    send(Event::Failed(
                        t!("connect_wizard.install_failed", reason = format!("{e:#}")).to_string(),
                    ));
                    return;
                }
                Err(Installing::Left(left, e)) => {
                    dir = Some(left);
                    Err(e)
                }
            },
            (NewCommand::SetKeys { exchange, keys }, Some(at)) => {
                set_keys(&form, at, exchange, keys, &send)
            }
            (NewCommand::Retry, Some(at)) => retry(&form, at, &send),
            (NewCommand::Remove, Some(at)) => match remove(&form, at) {
                Ok(()) => {
                    send(Event::Removed);
                    return;
                }
                Err(e) => Err(e),
            },
            // A command for another stage: the backend never sends one.
            _ => Ok(()),
        };
        if let Err(e) = result {
            send(Event::StepFailed(format!("{e:#}")));
        }
    }
    log::info!(target: CORE_MOVE_TARGET, "core install: the wizard let go of the login");
}

/// Explain a refused password with the attempted login, retaining the existing check error path.
fn check_failure(error: &anyhow::Error, first: &FirstAccess) -> String {
    let reason = super::login_failure_text(error, first);
    t!("connect_wizard.check_failed", reason = reason).to_string()
}

/// What an install failure leaves behind.
enum Installing {
    /// Nothing on the server.
    Nothing(anyhow::Error),
    /// The core runs in this folder without its MoonProto key.
    Left(String, anyhow::Error),
}

/// Log in with the provider's login against the confirmed host key.
fn login(form: &NewForm) -> anyhow::Result<(Conn, Privilege)> {
    setup::provider_login(&form.target, &form.first, &form.pin)
}

/// Log in and check that the server can run the core on its port.
fn check(
    form: &NewForm,
    send: &dyn Fn(Event),
) -> anyhow::Result<moon_remote::core_move::ServerCheck> {
    let (conn, privilege) = login(form)?;
    send(Event::Line(
        t!("connect_wizard.line.logged_in", user = conn.user()).to_string(),
    ));
    core_move::check(&conn, &privilege, form.proto_port, &mut |_| {})
}

/// Install the package and the service, start the core, wait out its initialization and read its
/// exchanges. MoonProto waits for the exchange: MoonBot keeps its settings per exchange, so the key
/// handed out now would belong to the default exchange rather than the user's. A demo core starts
/// on MoonBot's demo settings, so its initialization is its login to the demo account as well.
fn install(form: &NewForm, send: &dyn Fn(Event)) -> Result<String, Installing> {
    let (conn, privilege) = login(form).map_err(Installing::Nothing)?;
    send(Event::Line(
        if form.demo {
            t!("connect_wizard.line.downloading_demo")
        } else {
            t!("connect_wizard.line.downloading")
        }
        .to_string(),
    ));
    let installed = core_new::install(
        &conn,
        &privilege,
        &form.dir,
        form.proto_port,
        core_move::RELEASE_PACKAGE,
        form.demo.then_some(core_new::DEMO_SETTINGS),
    )
    .map_err(Installing::Nothing)?;
    let dir = installed.dir;
    send(Event::Line(
        t!("connect_wizard.line.installed", dir = &dir).to_string(),
    ));
    // After the install, so a server the core never got onto is left exactly as it was found.
    if form.close_password
        && let Some(line) =
            close_password_line(&conn, &privilege, &form.target, &form.first, &form.pin)
    {
        send(Event::Line(line));
    }
    let after = || -> anyhow::Result<_> {
        core_new::wait_initialized(&conn, &privilege, &dir, None, core_new::SETTLE_TIMEOUT)?;
        core_new::exchanges(&conn, &privilege, &dir)
    };
    match after() {
        Ok(exchanges) => {
            send(Event::Installed {
                dir: dir.clone(),
                exchanges,
            });
            Ok(dir)
        }
        Err(e) => {
            // The core is on the server all the same: the backend learns its folder, so the
            // stuck screen can retry it and name its row; the list stays empty until a retry
            // reads it.
            send(Event::Installed {
                dir: dir.clone(),
                exchanges: Vec::new(),
            });
            Err(Installing::Left(dir, e))
        }
    }
}

/// Connect a demo core: turn on MoonProto for the exchange its demo settings selected, hand the
/// key to the backend and wait for the login those settings started to settle. The exchange is
/// left as the settings have it — the demo account's keys belong to it.
fn connect_demo(form: &NewForm, dir: &str, send: &dyn Fn(Event)) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    let key = core_new::setup_moonproto(&conn, &privilege, dir, form.public_ip, form.proto_port)?;
    send(Event::Line(
        t!(
            "connect_wizard.line.key",
            address = format!("{}:{}", form.public_ip, form.proto_port)
        )
        .to_string(),
    ));
    send(Event::Key(key));
    settle(&conn, &privilege, dir, send)
}

/// Switch to the exchange and take its connection key, give it the keys and wait for the login to
/// settle. Every pick of an exchange — the first, or another one after a failed login — asks the
/// core for that exchange's own key: the row follows it rather than keeping the previous one.
fn set_keys(
    form: &NewForm,
    dir: &str,
    exchange: &str,
    keys: &ApiKeys,
    send: &dyn Fn(Event),
) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    let key = core_new::switch_exchange(
        &conn,
        &privilege,
        dir,
        exchange,
        form.public_ip,
        form.proto_port,
    )?;
    send(Event::Line(
        t!(
            "connect_wizard.line.key",
            address = format!("{}:{}", form.public_ip, form.proto_port)
        )
        .to_string(),
    ));
    send(Event::Key(key));
    core_new::set_api_keys(&conn, &privilege, dir, keys)?;
    // Only now does a demo core hold keys of the user's own. Until then whatever it can log in
    // with — on the exchange it started on or the one just switched to — came with the demo
    // settings, so a step that failed above leaves the install a demo one.
    send(Event::OwnKeys);
    send(Event::Line(
        t!("connect_wizard.line.keys_sent", exchange = exchange).to_string(),
    ));
    settle(&conn, &privilege, dir, send)
}

/// Log in again with the stored keys and wait for it to settle.
///
/// Once the login has settled, the exchange list and the selected exchange's key are read again
/// before the result is reported: the step that failed may have switched the exchange without
/// handing out its key, or an install may have stopped before the list came, and a row left with
/// another exchange's key would never connect. Read after the login, not before it, so a core
/// stuck initializing is still sent its new login at once; it is left as it is while it still
/// initializes, since it would refuse the setup.
fn retry(form: &NewForm, dir: &str, send: &dyn Fn(Event)) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    core_new::retry_login(&conn, &privilege, dir)?;
    send(Event::Line(t!("connect_wizard.line.retry").to_string()));
    let end = wait_login(&conn, &privilege, dir, send)?;
    if end.state != CoreState::Initializing {
        send(Event::Exchanges(core_new::exchanges(
            &conn, &privilege, dir,
        )?));
        let key =
            core_new::setup_moonproto(&conn, &privilege, dir, form.public_ip, form.proto_port)?;
        send(Event::Key(key));
    }
    send(Event::Settled(end));
    Ok(())
}

/// Report every status while the login settles, then the one it settled on.
fn settle(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    send: &dyn Fn(Event),
) -> anyhow::Result<()> {
    let end = wait_login(conn, privilege, dir, send)?;
    send(Event::Settled(end));
    Ok(())
}

/// Report every status while the login settles and return the one it settled on, unreported.
fn wait_login(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    send: &dyn Fn(Event),
) -> anyhow::Result<CoreStatus> {
    send(Event::Busy(NewPhase::LoggingIn));
    let mut last: Option<CoreStatus> = None;
    core_new::wait_settled(
        conn,
        privilege,
        dir,
        core_new::SETTLE_TIMEOUT,
        &mut |status| {
            if last.as_ref() != Some(status) {
                send(Event::Status(status.clone()));
                last = Some(status.clone());
            }
        },
    )
}

/// Take the instance out of the server, with the firewall rule its install opened.
fn remove(form: &NewForm, dir: &str) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    core_move::remove(&conn, &privilege, dir, &mut |_| {})
}
