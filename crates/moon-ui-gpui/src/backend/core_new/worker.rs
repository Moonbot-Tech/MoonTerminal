//! The threads of a new core's install: the host-key probe, and the install itself, which waits
//! for the user between its steps.

use std::net::Ipv4Addr;
use std::sync::mpsc;

use moon_core::diagnostics::CORE_MOVE_TARGET;
use moon_remote::core_move;
use moon_remote::core_new::{self, ApiKeys, CoreStatus};
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
                    dir = Some(done);
                    Ok(())
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
    let reason = if matches!(
        error.downcast_ref::<moon_remote::ssh::OpenError>(),
        Some(moon_remote::ssh::OpenError::Refused { .. })
    ) && let FirstAccess::Password { user, .. } = first
    {
        t!("connect_wizard.server.login_refused", user = user).to_string()
    } else {
        format!("{error:#}")
    };
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

/// Install the package and the service, start the core, turn MoonProto on, read its exchanges.
fn install(form: &NewForm, send: &dyn Fn(Event)) -> Result<String, Installing> {
    let (conn, privilege) = login(form).map_err(Installing::Nothing)?;
    send(Event::Line(
        t!("connect_wizard.line.downloading").to_string(),
    ));
    let installed = core_new::install(
        &conn,
        &privilege,
        &form.dir,
        form.proto_port,
        core_move::TEST_PACKAGE,
    )
    .map_err(Installing::Nothing)?;
    let dir = installed.dir;
    send(Event::Line(
        t!("connect_wizard.line.installed", dir = &dir).to_string(),
    ));
    let after = || -> anyhow::Result<_> {
        let key =
            core_new::setup_moonproto(&conn, &privilege, &dir, form.public_ip, form.proto_port)?;
        send(Event::Line(
            t!(
                "connect_wizard.line.key",
                address = format!("{}:{}", form.public_ip, form.proto_port)
            )
            .to_string(),
        ));
        let exchanges = core_new::exchanges(&conn, &privilege, &dir)?;
        Ok((key, exchanges))
    };
    match after() {
        Ok((key, exchanges)) => {
            send(Event::Installed {
                dir: dir.clone(),
                key,
                exchanges,
            });
            Ok(dir)
        }
        Err(e) => Err(Installing::Left(dir, e)),
    }
}

/// Select the exchange, give it the keys and wait for the login to settle.
fn set_keys(
    form: &NewForm,
    dir: &str,
    exchange: &str,
    keys: &ApiKeys,
    send: &dyn Fn(Event),
) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    core_new::select_exchange(&conn, &privilege, dir, exchange)?;
    core_new::set_api_keys(&conn, &privilege, dir, keys)?;
    send(Event::Line(
        t!("connect_wizard.line.keys_sent", exchange = exchange).to_string(),
    ));
    settle(&conn, &privilege, dir, send)
}

/// Log in again with the stored keys and wait for it to settle.
fn retry(form: &NewForm, dir: &str, send: &dyn Fn(Event)) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    core_new::retry_login(&conn, &privilege, dir)?;
    send(Event::Line(t!("connect_wizard.line.retry").to_string()));
    settle(&conn, &privilege, dir, send)
}

/// Report every status while the login settles, then the one it settled on.
fn settle(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    send: &dyn Fn(Event),
) -> anyhow::Result<()> {
    send(Event::Busy(NewPhase::LoggingIn));
    let mut last: Option<CoreStatus> = None;
    let end = core_new::wait_settled(
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
    )?;
    send(Event::Settled(end));
    Ok(())
}

/// Take the instance out of the server, with the firewall rule its install opened.
fn remove(form: &NewForm, dir: &str) -> anyhow::Result<()> {
    let (conn, privilege) = login(form)?;
    core_move::remove(&conn, &privilege, dir, &mut |_| {})
}
