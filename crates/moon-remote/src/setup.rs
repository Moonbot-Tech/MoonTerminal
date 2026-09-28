//! Preparing a server, in the order `docs-internal/STATION.md` §5.1 fixes: get in with what the
//! provider gave → an administrator reached by the app's key → close the server → the station's
//! account and unit. Core keys are not part of it; they go later, to a closed server only
//! (`station::push_cores`).
//!
//! The first login is held open to the end as the lifeline: every change to how one logs in is
//! verified through a NEW connection, and rolled back through the lifeline when that fails.

use std::path::PathBuf;

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
/// The distributions' own floor for a password (`pam_pwquality` minlen).
const MIN_ADMIN_PASSWORD: usize = 8;

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

pub struct Setup {
    pub target: Target,
    pub first: FirstAccess,
    /// The administrator's login, chosen by the user.
    pub admin: String,
    /// The administrator's password, chosen by the user: for sudo by hand, never for SSH.
    pub admin_password: Zeroizing<String>,
    /// A station binary to install on the way.
    pub station_bin: Option<PathBuf>,
}

/// Prepare the server. `say` receives one line per thing done or found.
pub fn run(setup: &Setup, say: &mut dyn FnMut(&str)) -> anyhow::Result<()> {
    anyhow::ensure!(
        !setup.admin_password.contains('\n'),
        "the administrator password must be one line"
    );
    // Debian and Ubuntu refuse shorter ones in `chpasswd` (pam_pwquality) — after the user was
    // already created. Refused here, before anything on the server changes.
    anyhow::ensure!(
        setup.admin_password.chars().count() >= MIN_ADMIN_PASSWORD,
        "the administrator password needs at least {MIN_ADMIN_PASSWORD} characters"
    );
    // A password is the first stdin line of `sudo -S`: a second line would spill into the
    // script's own input.
    let first_password = match &setup.first {
        FirstAccess::Password { password, .. } => Some(password),
        FirstAccess::Key { sudo_password, .. } => sudo_password.as_ref(),
    };
    anyhow::ensure!(
        first_password.is_none_or(|p| !p.contains('\n')),
        "the login password must be one line"
    );
    let hosts_path = Hosts::path();
    let mut hosts = Hosts::load(&hosts_path)?;
    let addr = setup.target.addr();
    let pin = hosts.get(&addr).map(|h| h.fingerprint.clone());
    let app = app_key::load_or_create()?;

    // 1. In. A server this tool already closed takes only the administrator's key; anything else
    // comes in with what the provider gave.
    let known_admin = hosts.get(&addr).and_then(|h| h.admin.clone());
    let (lifeline, privilege) = match known_admin.filter(|a| *a == setup.admin) {
        Some(admin) => match Conn::open(
            &setup.target,
            &Auth::Key {
                user: &admin,
                key: &app,
            },
            pin.as_deref(),
        ) {
            Ok(conn) => {
                say(&format!("in as {admin} by the app key"));
                let privilege = Privilege::detect(&conn, Some(&setup.admin_password))?;
                // On a server already set up, sudo takes the administrator's CURRENT password;
                // a different one would fail deep inside the first step with sudo's own words.
                privilege.run(&conn, "true", &[], STEP_TIMEOUT).context(
                    "sudo refused the administrator password: a re-run needs the current one",
                )?;
                (conn, privilege)
            }
            Err(OpenError::Refused { .. }) => first_login(setup, pin.as_deref(), say)?,
            Err(e) => return Err(e.into()),
        },
        None => first_login(setup, pin.as_deref(), say)?,
    };
    if pin.is_none() {
        hosts.pin(&addr, lifeline.fingerprint())?;
        hosts.save(&hosts_path)?;
        say(&format!("host key pinned: {}", lifeline.fingerprint()));
    }
    let pin = Some(lifeline.fingerprint().to_owned());
    let root =
        |command: String, stdin: &[u8]| privilege.run(&lifeline, &command, stdin, STEP_TIMEOUT);

    // 2. What is there.
    let probe = root(script::bootstrap("probe", &[]), &[])?.stdout_text();
    for line in probe.lines() {
        say(&format!("probe: {line}"));
    }
    let systemd: u32 = script::value(&probe, "systemd")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("no systemd on this server"))?;
    anyhow::ensure!(
        systemd >= MIN_SYSTEMD,
        "systemd {systemd} is older than {MIN_SYSTEMD}: no encrypted credentials"
    );

    // 3. The administrator, and the one script it may run without a password.
    let mut input = Zeroizing::new(Vec::new());
    input.extend_from_slice(setup.admin_password.as_bytes());
    input.push(b'\n');
    input.extend_from_slice(app_key::authorized_line(&app)?.as_bytes());
    input.push(b'\n');
    let out = root(script::bootstrap("admin", &[&setup.admin]), &input)?;
    say_values(say, &out.stdout_text());
    let out = root(
        script::bootstrap("helper", &[&setup.admin]),
        script::HELPER.as_bytes(),
    )?;
    say_values(say, &out.stdout_text());
    admin_works(setup, &app, pin.as_deref()).context("the new administrator does not work")?;
    hosts.set_admin(&addr, &setup.admin)?;
    hosts.save(&hosts_path)?;
    say(&format!(
        "{} logs in by the app key; sudo for {HELPER_PATH} works",
        setup.admin
    ));

    // 4. The station's account, directories and unit — before closing, so a failure here still
    // leaves every way in open.
    let out = root(script::bootstrap("service", &[]), script::UNIT.as_bytes())?;
    say_values(say, &out.stdout_text());

    // 5. Close the server. Verified from outside when sshd changed, rolled back through the
    // lifeline. Unchanged, `sshd -T` in the step already vouched for it, and the refused password
    // logins are not repeated: under fail2ban every re-run would count toward banning the very
    // address the administrator comes from.
    let out = root(script::bootstrap("harden", &[]), &[])?;
    say_values(say, &out.stdout_text());
    let changed = script::value(&out.stdout_text(), "sshd") != Some("unchanged");
    let verified = match changed {
        true => closed_properly(setup, &app, pin.as_deref()),
        false => admin_works(setup, &app, pin.as_deref()),
    };
    if let Err(e) = verified {
        let undo = root(script::bootstrap("unharden", &[]), &[]);
        return Err(match undo {
            Ok(_) => e.context("closing the server failed; sshd is back as it was"),
            Err(undo) => e.context(format!(
                "closing the server failed AND the rollback failed: {undo:#}"
            )),
        });
    }
    say(match changed {
        true => "sshd: keys only, no root; a password login is refused",
        false => "sshd: already keys only, no root",
    });

    // May install ufw first: the package deadline, not a step's.
    let out = privilege.run(
        &lifeline,
        &script::bootstrap("firewall", &[]),
        &[],
        APT_TIMEOUT,
    )?;
    for line in out.stdout_text().lines() {
        say(line);
    }
    if let Err(e) = admin_works(setup, &app, pin.as_deref()) {
        let undo = root(script::bootstrap("firewall-off", &[]), &[]);
        return Err(match undo {
            Ok(_) => e.context("the firewall cut the administrator off; it is disabled again"),
            Err(undo) => e.context(format!(
                "the firewall cut the administrator off AND disabling it failed: {undo:#}"
            )),
        });
    }
    let out = privilege.run(
        &lifeline,
        &script::bootstrap("extras", &[]),
        &[],
        APT_TIMEOUT,
    )?;
    say_values(say, &out.stdout_text());

    // 6. The binary, if one was given — through the narrow path, as every later update will.
    if let Some(path) = &setup.station_bin {
        install_station(&setup.target, &setup.admin, &app, pin.as_deref(), path, say)?;
    }
    Ok(())
}

/// Log in with what the provider gave.
fn first_login(
    setup: &Setup,
    pin: Option<&str>,
    say: &mut dyn FnMut(&str),
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
    say(&format!("in as {} with the provider's login", conn.user()));
    let privilege = Privilege::detect(&conn, sudo_password.as_ref())?;
    Ok((conn, privilege))
}

/// A NEW login as the administrator by the app key, and its passwordless sudo for the helper.
fn admin_works(setup: &Setup, app: &PrivateKey, pin: Option<&str>) -> anyhow::Result<()> {
    let conn = Conn::open(
        &setup.target,
        &Auth::Key {
            user: &setup.admin,
            key: app,
        },
        pin,
    )?;
    script::checked(conn.run(&script::helper("status", &[]), &[], STEP_TIMEOUT)?)?;
    Ok(())
}

/// The closed server, seen from outside: the administrator's key still works, and a password is
/// refused — the administrator's, and root's when that is what the provider gave.
fn closed_properly(setup: &Setup, app: &PrivateKey, pin: Option<&str>) -> anyhow::Result<()> {
    admin_works(setup, app, pin)?;
    must_refuse(&setup.target, &setup.admin, &setup.admin_password, pin)?;
    if let FirstAccess::Password { user, password } = &setup.first {
        must_refuse(&setup.target, user, password, pin)?;
    }
    Ok(())
}

fn must_refuse(
    target: &Target,
    user: &str,
    password: &str,
    pin: Option<&str>,
) -> anyhow::Result<()> {
    match Conn::open(target, &Auth::Password { user, password }, pin) {
        Err(OpenError::Refused {
            password_offered: false,
        }) => Ok(()),
        Err(OpenError::Refused {
            password_offered: true,
        }) => anyhow::bail!("{user} was refused, but the server still offers password logins"),
        Ok(_) => anyhow::bail!("a password login as {user} still works"),
        Err(e) => Err(anyhow::Error::new(e).context(format!("checking that {user} is refused"))),
    }
}

/// Send a station binary through the helper; sha256 checked on the server before it replaces
/// anything.
pub fn install_station(
    target: &Target,
    admin: &str,
    app: &PrivateKey,
    pin: Option<&str>,
    path: &std::path::Path,
    say: &mut dyn FnMut(&str),
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
    let out = script::checked(conn.run(
        &script::helper("install-bin", &[&digest]),
        &bytes,
        APT_TIMEOUT,
    )?)?;
    say_values(say, &out.stdout_text());
    Ok(())
}

fn say_values(say: &mut dyn FnMut(&str), text: &str) {
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        say(line);
    }
}
