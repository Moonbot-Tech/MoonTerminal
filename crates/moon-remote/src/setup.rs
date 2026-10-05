//! Preparing a server, in the order `docs-internal/STATION.md` §5.1 fixes: get in with what the
//! provider gave → an administrator reached by the app's key → close the server → the station's
//! account and unit. Core keys are not part of it; they go later, to a closed server only
//! (`station::push_cores`, additive; only `station::remove_cores` removes named cores).
//!
//! Before any of it the server's host key is read without a credential ([`probe_host_key`]) and
//! confirmed by the user; only then does the first login go out. The probe is read-only and refuses
//! a server this setup cannot finish (no apt-get, no sshd_config.d include, old systemd) before
//! anything on it changes. The firewall opens the SSH ports in use — the one the setup dials and the
//! ones sshd listens on, which a provider's NAT may make different; an already active ufw only
//! gains those rules, and the rollback undoes exactly the rules and policy the step reported.
//!
//! The first login is held open to the end as the lifeline: every change to how one logs in is
//! verified through a NEW connection, and rolled back through the lifeline when that fails.

use std::path::PathBuf;

use crate::error::StationError;
use crate::progress::{Progress, Step};
use anyhow::Context;
use russh::keys::PrivateKey;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::app_key;
use crate::hosts::Hosts;
use crate::script::{self, APT_TIMEOUT, HELPER_PATH, Privilege, STEP_TIMEOUT};
use crate::ssh::{Auth, Conn, OpenError, Target};

/// Oldest systemd with `systemd-creds` and `LoadCredentialEncrypted=`.
const MIN_SYSTEMD: u32 = 250;
/// The administrator every setup creates: one name for every server, never shown or asked.
pub const ADMIN: &str = "moon";

/// A server set up before the administrator lost its password (2026-09-30): its sudo still asks
/// for it, once, to move the server onto key-only sudo.
#[derive(Debug)]
pub struct NeedsAdminPassword {
    pub admin: String,
}

impl std::fmt::Display for NeedsAdminPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}'s sudo still asks for a password: this server was set up before key-only sudo — \
             give that password once to move it over",
            self.admin
        )
    }
}

impl std::error::Error for NeedsAdminPassword {}

/// What the provider gave for the first login. Used for the setup only, never stored.
pub enum FirstAccess {
    Password {
        user: String,
        password: Zeroizing<String>,
    },
    /// A key login; `sudo_password` only when the user is not root and sudo asks for one.
    Key {
        user: String,
        key: Box<PrivateKey>,
        sudo_password: Option<Zeroizing<String>>,
    },
}

/// The station binary a setup installs on the way.
pub enum StationBinary {
    /// None: a server set up again keeps the one it has.
    Keep,
    /// The newest MoonTerminal release's, for the server's architecture — how a server gets its
    /// first station (the terminal's Settings).
    Release,
    /// A file of this machine — a developer's build (`moon-remote setup --station-bin`).
    File(PathBuf),
}

pub struct Setup {
    pub target: Target,
    pub first: FirstAccess,
    /// The old administrator password of a server set up before key-only sudo
    /// ([`NeedsAdminPassword`]); `None` everywhere else.
    pub legacy_admin_password: Option<Zeroizing<String>>,
    /// The station binary to install on the way.
    pub station: StationBinary,
    /// The host key fingerprint the user confirmed ([`probe_host_key`]); `None` when nothing was
    /// confirmed. A server not pinned yet is never logged into without it.
    pub host_key: Option<String>,
}

/// The key a server presents, shown to the user before anything is sent to it.
#[derive(Clone)]
pub struct HostKey {
    pub target: Target,
    pub fingerprint: String,
}

/// Read the host key `target` presents without sending any credential. A server set up here that
/// now presents another key is refused, as every connection to it would be; the pin of a server
/// whose setup never finished is not trusted over what the user confirms now.
pub fn probe_host_key(target: Target) -> anyhow::Result<HostKey> {
    let hosts = Hosts::load(&Hosts::path())?;
    let fingerprint = crate::ssh::presented_key(&target)?;
    if let Some(host) = hosts.get(&target.addr()).filter(|h| h.admin.is_some()) {
        if host.fingerprint != fingerprint {
            return Err(OpenError::HostKeyChanged {
                pinned: host.fingerprint.clone(),
                presented: fingerprint,
            }
            .into());
        }
    }
    Ok(HostKey {
        target,
        fingerprint,
    })
}

/// The pin a setup connects with: the stored one, or the one the user confirmed. A confirmation
/// that disagrees with the pin of a server set up here (`set_up`) is a changed key; over the pin
/// of a setup that never finished, the confirmation wins. Neither is no connection at all.
pub(crate) fn trusted_pin(
    pinned: Option<&str>,
    confirmed: Option<&str>,
    set_up: bool,
) -> anyhow::Result<String> {
    match (pinned, confirmed) {
        (Some(pin), None) => Ok(pin.to_owned()),
        (Some(pin), Some(seen)) if pin == seen => Ok(pin.to_owned()),
        (Some(_), Some(seen)) if !set_up => Ok(seen.to_owned()),
        (Some(pin), Some(seen)) => Err(OpenError::HostKeyChanged {
            pinned: pin.to_owned(),
            presented: seen.to_owned(),
        }
        .into()),
        (None, Some(seen)) => Ok(seen.to_owned()),
        (None, None) => Err(StationError::HostKeyUnconfirmed.into()),
    }
}

/// Prepare the server. `say` receives typed steps and separate helper diagnostics.
pub fn run(setup: &Setup, say: &mut dyn FnMut(Progress)) -> anyhow::Result<()> {
    // A password is the first stdin line of `sudo -S`: a second line would spill into the
    // script's own input.
    let first_password = match &setup.first {
        FirstAccess::Password { password, .. } => Some(password),
        FirstAccess::Key { sudo_password, .. } => sudo_password.as_ref(),
    };
    anyhow::ensure!(
        first_password
            .into_iter()
            .chain(&setup.legacy_admin_password)
            .all(|p| !p.contains('\n')),
        StationError::PasswordOneLine
    );
    let hosts_path = Hosts::path();
    let mut hosts = Hosts::load(&hosts_path)?;
    let addr = setup.target.addr();
    let known = hosts.get(&addr).cloned();
    let pinned = known.as_ref().map(|h| h.fingerprint.clone());
    let known_admin = known.and_then(|h| h.admin);
    // Before the app key, before any login: no credential leaves for an unconfirmed key.
    let pin = trusted_pin(
        pinned.as_deref(),
        setup.host_key.as_deref(),
        known_admin.is_some(),
    )?;
    let app = app_key::load_or_create()?;

    // 1. In. A server this tool already closed takes only the administrator's key; anything else
    // comes in with what the provider gave.
    // A server set up earlier keeps its administrator, whatever it was called then.
    let admin = known_admin.clone().unwrap_or_else(|| ADMIN.to_owned());
    let (lifeline, privilege) = match known_admin {
        Some(admin) => match Conn::open(
            &setup.target,
            &Auth::Key {
                user: &admin,
                key: &app,
            },
            &pin,
        ) {
            Ok(conn) => {
                say(Progress::step(
                    Step::Login,
                    format!("in as {admin} by the app key"),
                ));
                // Only a sudo that answered "a password is needed" asks for the old one; a dropped
                // connection or a timeout is its own error, not a question about a password.
                let sudo_free = conn.run("sudo -n true", &[], STEP_TIMEOUT)?.ok();
                if !sudo_free && setup.legacy_admin_password.is_none() {
                    return Err(NeedsAdminPassword { admin }.into());
                }
                let privilege = Privilege::detect(&conn, setup.legacy_admin_password.as_ref())?;
                // The old password, when one was needed, must be the current one: a wrong one
                // would fail deep inside the first step with sudo's own words.
                privilege
                    .run(&conn, "true", &[], STEP_TIMEOUT)
                    .context("sudo refused the old administrator password")?;
                (conn, privilege)
            }
            Err(OpenError::Refused { .. }) => first_login(setup, &pin, say)?,
            Err(e) => return Err(e.into()),
        },
        None => first_login(setup, &pin, say)?,
    };
    let root =
        |command: String, stdin: &[u8]| privilege.run(&lifeline, &command, stdin, STEP_TIMEOUT);

    // 2. What is there. Read-only; a server this setup cannot finish is refused here, before
    // anything on it changes — and before it is pinned.
    let ssh_port = setup.target.port.to_string();
    let probe = root(script::bootstrap("probe", &[&ssh_port]), &[])?.stdout_text();
    for line in probe.lines() {
        say(Progress::Diagnostic(format!("probe: {line}")));
    }
    say(Progress::step(Step::Probe, "server probed"));
    check_probe(&probe)?;
    let ufw_before = script::value(&probe, "ufw").unwrap_or("none").to_owned();
    if ufw_before != "active" {
        say_closing_ports(&probe, say);
    }
    if pinned.as_deref() != Some(pin.as_str()) {
        // The stale pin of a setup that never finished gives way to the key the user confirmed.
        hosts.forget(&addr);
        hosts.pin(&addr, &pin)?;
        hosts.save(&hosts_path)?;
        say(Progress::step(Step::Pin, format!("host key pinned: {pin}")));
    }
    // The release's binary is fetched now, before anything on the server changes: no release for
    // its architecture, or GitHub out of reach, stops the setup here rather than after the server
    // was closed.
    // Its own directory per process, gone on every way out of the setup.
    let release_dir =
        DropDir(std::env::temp_dir().join(format!("moon-station-release-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&release_dir.0);
    let release_bin = match &setup.station {
        StationBinary::Release => {
            let arch = script::value(&probe, "arch")
                .ok_or_else(|| anyhow::anyhow!("the server did not report its architecture"))?;
            Some(crate::release::fetch_station(arch, &release_dir.0, say)?)
        }
        _ => None,
    };

    // 3. The administrator — by key only, the app's and the user's own when the first login was
    // by key — and its sudo without a password.
    let mut input = app_key::authorized_line(&app)?;
    input.push('\n');
    if let FirstAccess::Key { key, .. } = &setup.first {
        input.push_str(&crate::keys::authorized_line(key)?);
        input.push('\n');
    }
    let out = root(script::bootstrap("admin", &[&admin]), input.as_bytes())?;
    say_values(say, &out.stdout_text());
    let out = root(
        script::bootstrap("helper", &[&admin]),
        script::HELPER.as_bytes(),
    )?;
    say_values(say, &out.stdout_text());
    say(Progress::step(Step::Helper, "step complete"));
    admin_works(&setup.target, &admin, &app, &pin)
        .context("the new administrator does not work")?;
    hosts.set_admin(&addr, &admin)?;
    hosts.save(&hosts_path)?;
    say(Progress::step(
        Step::Admin,
        format!("{admin} logs in by the app key; sudo without a password and {HELPER_PATH} work"),
    ));

    // 4. The station's account, directories and unit — before closing, so a failure here still
    // leaves every way in open.
    let out = root(script::bootstrap("service", &[]), script::UNIT.as_bytes())?;
    say_values(say, &out.stdout_text());
    say(Progress::step(Step::Service, "step complete"));

    // 5. Close the server. Verified from outside when sshd changed, rolled back through the
    // lifeline. Unchanged, `sshd -T` in the step already vouched for it, and the refused password
    // logins are not repeated: under fail2ban every re-run would count toward banning the very
    // address the administrator comes from.
    let out = root(script::bootstrap("harden", &[]), &[])?;
    say_values(say, &out.stdout_text());
    let changed = script::value(&out.stdout_text(), "sshd") != Some("unchanged");
    let verified = match changed {
        true => closed_properly(setup, &admin, &app, &pin),
        false => admin_works(&setup.target, &admin, &app, &pin),
    };
    if let Err(e) = verified {
        // Unchanged, the step left nothing of its own to undo.
        if !changed {
            return Err(e);
        }
        let undo = root(script::bootstrap("unharden", &[]), &[]);
        return Err(match undo {
            Ok(_) => e.context("closing the server failed; sshd is back as it was"),
            Err(undo) => e.context(format!(
                "closing the server failed AND the rollback failed: {undo:#}"
            )),
        });
    }
    say(Progress::step(
        Step::Harden,
        match changed {
            true => "sshd: keys only, no root; a password login is refused",
            false => "sshd: already keys only, no root",
        },
    ));

    // ufw itself first, on the package deadline; then the firewall step on a step's. A failed step
    // is rolled back as well: it may have stopped half-way, past an SSH rule, and its stdout says
    // how far it got whatever its exit.
    let out = privilege.run(
        &lifeline,
        &script::bootstrap("ufw-install", &[]),
        &[],
        APT_TIMEOUT,
    )?;
    say_values(say, &out.stdout_text());
    let firewall = privilege.run_raw(
        &lifeline,
        &firewall_command(&setup.target),
        &[],
        STEP_TIMEOUT,
    );
    let stdout = firewall
        .as_ref()
        .map(|out| out.stdout_text())
        .unwrap_or_default();
    let verified = match firewall.and_then(script::checked) {
        Ok(_) => {
            for line in stdout.lines() {
                say(Progress::Diagnostic(line.to_owned()));
            }
            say(Progress::step(Step::Firewall, "firewall configured"));
            admin_works(&setup.target, &admin, &app, &pin)
                .context("the firewall cut the administrator off")
        }
        Err(e) => Err(e.context("the firewall step failed")),
    };
    if let Err(e) = verified {
        let Some(undo) = firewall_rollback(&stdout, &ufw_before) else {
            return Err(e);
        };
        return Err(match root(undo, &[]) {
            Ok(_) => e.context("the firewall is back as it was"),
            Err(undo) => e.context(format!("AND rolling the firewall back failed: {undo:#}")),
        });
    }
    let out = privilege.run(
        &lifeline,
        &script::bootstrap("extras", &[]),
        &[],
        APT_TIMEOUT,
    )?;
    say_values(say, &out.stdout_text());
    say(Progress::step(Step::Extras, "step complete"));

    // 6. The binary — through the helper's `update`, as every later one will.
    match &setup.station {
        StationBinary::Keep => {}
        StationBinary::File(path) => {
            install_station(&setup.target, &admin, &app, &pin, path, say)?;
        }
        StationBinary::Release => {
            if let Some(file) = &release_bin {
                install_station(&setup.target, &admin, &app, &pin, file, say)?;
            }
        }
    }
    Ok(())
}

/// A directory of this machine removed when it goes out of scope, whichever way.
struct DropDir(PathBuf);

impl Drop for DropDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The ports that listen now and an inactive firewall is about to close, as the probe's
/// `listen=` lines name them; `listen=unknown` means they could not be listed.
fn say_closing_ports(probe: &str, say: &mut dyn FnMut(Progress)) {
    let listen: Vec<&str> = probe
        .lines()
        .filter_map(|line| line.strip_prefix("listen="))
        .map(str::trim)
        .collect();
    if listen.contains(&"unknown") {
        say(Progress::step(
            Step::FirewallPortsUnknown,
            "listening ports unknown; the firewall closes every incoming port except SSH",
        ));
        return;
    }
    if !listen.is_empty() {
        // The diagnostic is the port list itself: the UI puts it into its sentence.
        say(Progress::step(Step::FirewallWillClose, listen.join(", ")));
    }
}

/// Log in with what the provider gave.
/// Refuse a server this setup cannot finish, from the probe's read-only report, before anything on
/// it changes.
pub(crate) fn check_probe(probe: &str) -> Result<(), StationError> {
    let systemd: u32 = script::value(probe, "systemd")
        .and_then(|v| v.parse().ok())
        .ok_or(StationError::SystemdMissing)?;
    if systemd < MIN_SYSTEMD {
        return Err(StationError::SystemdTooOld);
    }
    if script::value(probe, "apt") != Some("yes") {
        return Err(StationError::NoAptGet);
    }
    if script::value(probe, "sshd_include") != Some("yes") {
        return Err(StationError::SshdNoInclude);
    }
    Ok(())
}

/// The firewall step, opening the SSH port this setup actually connects to.
pub(crate) fn firewall_command(target: &Target) -> String {
    script::bootstrap("firewall", &[&target.port.to_string()])
}

/// The command that undoes the firewall step, from what it reported, even half-way: the rules it
/// added (`firewall_added=`), the incoming policy it found (`firewall_prev_in=`), how it ended
/// (`firewall=`). Without an end line, from the state the probe saw before it: a ufw that was not
/// active is always turned back off (the step may have enabled it unreported), and a rule that
/// may have existed before is never removed. `None`: nothing to undo.
pub(crate) fn firewall_rollback(stdout: &str, ufw_before: &str) -> Option<String> {
    let added: Vec<&str> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("firewall_added="))
        .map(str::trim)
        .collect();
    let prev_in = script::value(stdout, "firewall_prev_in");
    let off = || {
        let mut args = vec![prev_in.unwrap_or("unknown")];
        args.extend(&added);
        script::bootstrap("firewall-off", &args)
    };
    let unallow = || script::bootstrap("firewall-unallow", &added);
    match script::value(stdout, "firewall") {
        Some("enabled") => Some(off()),
        Some("rule-added") => Some(unallow()),
        Some(_) => None,
        None if !added.is_empty() => match ufw_before {
            "active" => Some(unallow()),
            _ => Some(off()),
        },
        // A step that died may have turned ufw on without reporting it; undoing an off ufw is
        // harmless, while an active one may hold rules the user already had.
        None => match ufw_before {
            "active" => None,
            _ => Some(off()),
        },
    }
}

fn first_login(
    setup: &Setup,
    pin: &str,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<(Conn, Privilege)> {
    let (conn, sudo_password) = match &setup.first {
        FirstAccess::Password { user, password } => {
            let conn = Conn::open(&setup.target, &Auth::Password { user, password }, pin)?;
            (conn, Some(password.clone()))
        }
        FirstAccess::Key {
            user,
            key,
            sudo_password,
        } => {
            let conn = Conn::open(&setup.target, &Auth::Key { user, key }, pin)?;
            (conn, sudo_password.clone())
        }
    };
    say(Progress::step(
        Step::Login,
        format!("in as {} with the provider's login", conn.user()),
    ));
    let privilege = Privilege::detect(&conn, sudo_password.as_ref())?;
    Ok((conn, privilege))
}

/// A NEW login as the administrator by the app key, its sudo without a password, and the helper.
fn admin_works(target: &Target, admin: &str, app: &PrivateKey, pin: &str) -> anyhow::Result<()> {
    let conn = Conn::open(
        target,
        &Auth::Key {
            user: admin,
            key: app,
        },
        pin,
    )?;
    script::checked(conn.run("sudo -n true", &[], STEP_TIMEOUT)?)
        .context("sudo still asks for a password")?;
    script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?;
    Ok(())
}

/// The closed server, seen from outside: the administrator's key still works, and the password
/// the provider gave is refused.
fn closed_properly(setup: &Setup, admin: &str, app: &PrivateKey, pin: &str) -> anyhow::Result<()> {
    admin_works(&setup.target, admin, app, pin)?;
    if let FirstAccess::Password { user, password } = &setup.first {
        must_refuse(&setup.target, user, password, pin)?;
    }
    Ok(())
}

fn must_refuse(target: &Target, user: &str, password: &str, pin: &str) -> anyhow::Result<()> {
    match Conn::open(target, &Auth::Password { user, password }, pin) {
        Err(OpenError::Refused {
            password_offered: false,
        }) => Ok(()),
        Err(OpenError::Refused {
            password_offered: true,
        }) => anyhow::bail!(StationError::PasswordStillOffered),
        Ok(_) => anyhow::bail!(StationError::PasswordStillWorks),
        Err(e) => Err(anyhow::Error::new(e).context(format!("checking that {user} is refused"))),
    }
}

/// Send a station binary through the helper's `update`: sha256 checked on the server before it
/// replaces anything; a station already running is restarted on it and must stay up, or the
/// previous binary goes back and this fails.
pub fn install_station(
    target: &Target,
    admin: &str,
    app: &PrivateKey,
    pin: &str,
    path: &std::path::Path,
    say: &mut dyn FnMut(Progress),
) -> anyhow::Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let conn = Conn::open(
        target,
        &Auth::Key {
            user: admin,
            key: app,
        },
        pin,
    )?;
    // The connection, not the update, failed — an error, or an end without an exit status: the
    // update goes on without it (the helper detaches it), and what the server says it did is the
    // answer, read through a new one. `update=running` there means it is still under way.
    let run = conn.run(&script::helper("update", &[&digest]), &bytes, APT_TIMEOUT);
    let out = match run {
        Ok(out) if out.status.is_some() => out,
        dropped => {
            let e = match dropped {
                Ok(out) => anyhow::anyhow!(
                    "the connection ended before the update did: {}",
                    out.stdout_text().trim()
                ),
                Err(e) => e,
            };
            return Err(match last_update(target, admin, app, pin) {
                Some(verdict) => e.context(format!("the server's last update says: {verdict}")),
                None => e,
            });
        }
    };
    let out = script::checked(out)?;
    say_values(say, &out.stdout_text());
    say(Progress::step(Step::Install, "station binary installed"));
    Ok(())
}

/// The last update's verdict as the server keeps it, through a fresh connection; `None` when that
/// cannot be read either.
fn last_update(target: &Target, admin: &str, app: &PrivateKey, pin: &str) -> Option<String> {
    let conn = Conn::open(
        target,
        &Auth::Key {
            user: admin,
            key: app,
        },
        pin,
    )
    .ok()?;
    let out = conn
        .run(&script::helper("status", &[]), &[], STEP_TIMEOUT)
        .ok()?;
    script::value(&out.stdout_text(), "last_update").map(str::to_owned)
}

/// Retain helper output as diagnostics without putting protocol tokens in UI progress.
fn say_values(say: &mut dyn FnMut(Progress), text: &str) {
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        say(Progress::Diagnostic(line.to_owned()));
    }
}

#[cfg(test)]
mod tests;
