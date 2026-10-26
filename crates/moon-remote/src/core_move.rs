//! Installing a moved MoonBot core on a Linux server over SSH, and taking it back out.
//!
//! The server side is one POSIX script, `remote/moonbot-move.sh`, carried in this crate so the
//! binary that runs a move is the one whose script runs on the server. Each call runs one of its
//! steps as root: [`check`] the server, [`install`] the core, [`finish`] once the core is
//! confirmed working, or [`remove`] it again as the rollback. The settings archive and the
//! connection packet are built by `moon_core::core_move`.
//!
//! Nothing here touches the old core: stopping it is the caller's decision, taken only after the
//! new one answered.

use std::time::Duration;

use anyhow::Context;

use crate::script::{self, Privilege, STEP_TIMEOUT, sh_quote};
use crate::ssh::Conn;

/// The server-side script.
pub const SCRIPT: &str = include_str!("../remote/moonbot-move.sh");
/// The MoonBot Linux test package announced with 7.72; any other package URL may be given.
pub const TEST_PACKAGE: &str = "https://api.moon-bot.com/files/linux/MoonBot-L2.zip";
/// Download, unpack and first start of the core together.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(1200);
/// Uploading the settings archive, which may carry a large report database.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(1800);
/// The prefix every work directory of a move has on the server.
const WORK_PREFIX: &str = "/tmp/moonbot-move.";

/// One step of a move, for the caller to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveStep {
    /// The server can run MoonBot and the UDP port is free.
    Checked,
    /// The settings archive and the connection packet are on the server.
    Uploaded,
    /// The core runs and listens for MoonProto.
    Started,
    /// The first-start INI switch is gone again.
    Finished,
    /// The instance is stopped and deleted.
    Removed,
}

/// What [`install`] puts on the server.
pub struct Install<'a> {
    /// The instance directory: absolute, or relative to `/root`. A taken name gets `-2`, `-3`…
    pub dir: &'a str,
    /// The UDP port the core's MoonProto server listens on.
    pub proto_port: u16,
    /// The MoonBot Linux package to download on the server.
    pub package_url: &'a str,
    /// The settings archive (`moon_core::core_move::archive::compose`).
    pub archive: &'a [u8],
    /// The connection packet the core takes its MoonProto key and port from.
    pub key_packet: &'a str,
    /// Whether the archive added the first-start INI switch, which [`finish`] then removes.
    pub ini_flag_added: bool,
}

/// Where [`install`] put the core.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The instance directory actually used.
    pub dir: String,
    /// Its systemd unit.
    pub unit: String,
}

/// The command line running `step` of the script with `args`.
fn command(step: &str, args: &[&str]) -> String {
    let mut line = format!("sh -c {} moonbot-move {step}", sh_quote(SCRIPT));
    for arg in args {
        line.push(' ');
        line.push_str(&sh_quote(arg));
    }
    line
}

/// Check that the server can run MoonBot (x86_64, glibc 2.38+, systemd) and that `port` is free;
/// missing `curl`/`unzip`/`ss` are installed through apt.
pub fn check(
    conn: &Conn,
    privilege: &Privilege,
    port: u16,
    say: &mut dyn FnMut(MoveStep),
) -> anyhow::Result<()> {
    let out = privilege.run(
        conn,
        &command("check", &[&port.to_string()]),
        &[],
        script::APT_TIMEOUT,
    )?;
    anyhow::ensure!(
        out.stdout_text().contains("CHECK_OK"),
        "the server check did not finish: {}",
        out.stdout_text().trim()
    );
    say(MoveStep::Checked);
    Ok(())
}

/// Upload the settings and the connection packet, install the package, start the core and wait
/// until it took its key and listens.
///
/// A failure leaves no instance and no work directory behind, with two exceptions: the package's
/// `moonbot@.service` template is shared by every instance and stays installed, and a connection
/// that drops or times out mid-install may leave the script to finish on its own. The script then
/// removes its work directory itself, and [`remove`] takes out an instance it completed.
pub fn install(
    conn: &Conn,
    privilege: &Privilege,
    install: &Install<'_>,
    say: &mut dyn FnMut(MoveStep),
) -> anyhow::Result<Installed> {
    let work = privilege
        .run(
            conn,
            "mktemp -d /tmp/moonbot-move.XXXXXXXX",
            &[],
            STEP_TIMEOUT,
        )?
        .stdout_text()
        .trim()
        .to_owned();
    anyhow::ensure!(
        work.starts_with(WORK_PREFIX) && !work.contains(['\n', '\'', ' ']),
        "unexpected work directory {work:?}"
    );
    // The work directory holds the exchange API keys: best effort to drop it on every failure.
    // The script drops it as well; this covers a failure before or around the script's own trap.
    let drop_work = || {
        let _ = privilege.run_raw(
            conn,
            &format!("rm -rf -- {}", sh_quote(&work)),
            &[],
            STEP_TIMEOUT,
        );
    };
    if let Err(e) = stage(conn, privilege, &work, install) {
        drop_work();
        return Err(e);
    }
    say(MoveStep::Uploaded);
    let port = install.proto_port.to_string();
    let out = match privilege.run(
        conn,
        &command("install", &[&work, install.dir, &port, install.package_url]),
        &[],
        INSTALL_TIMEOUT,
    ) {
        Ok(out) => out,
        Err(e) => {
            drop_work();
            return Err(e.context("install the core"));
        }
    };
    let text = out.stdout_text();
    let dir = script::value(&text, "MOVE_DIRECTORY")
        .ok_or_else(|| anyhow::anyhow!("the install did not report its directory: {text}"))?;
    let unit = script::value(&text, "MOVE_SERVICE")
        .ok_or_else(|| anyhow::anyhow!("the install did not report its service: {text}"))?;
    say(MoveStep::Started);
    Ok(Installed {
        dir: dir.to_owned(),
        unit: unit.to_owned(),
    })
}

/// Put the archive, the packet and the INI-switch note into the work directory.
fn stage(
    conn: &Conn,
    privilege: &Privilege,
    work: &str,
    install: &Install<'_>,
) -> anyhow::Result<()> {
    let upload = |name: &str, data: &[u8], timeout: Duration| -> anyhow::Result<()> {
        let path = format!("{work}/{name}");
        privilege
            .run(
                conn,
                &format!("sh -c 'umask 077; cat > \"$1\"' upload {}", sh_quote(&path)),
                data,
                timeout,
            )
            .with_context(|| format!("upload {name}"))?;
        Ok(())
    };
    upload("state.zip", install.archive, UPLOAD_TIMEOUT)?;
    upload("key", install.key_packet.trim().as_bytes(), STEP_TIMEOUT)?;
    if install.ini_flag_added {
        upload("ini-flag", b"", STEP_TIMEOUT)?;
    }
    Ok(())
}

/// Drop the first-start INI switch the move added, once the core is confirmed working.
pub fn finish(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    say: &mut dyn FnMut(MoveStep),
) -> anyhow::Result<()> {
    privilege.run(conn, &command("finish", &[dir]), &[], STEP_TIMEOUT)?;
    say(MoveStep::Finished);
    Ok(())
}

/// The rollback: stop and delete an instance a move installed, with the firewall rule it opened.
/// An instance this module did not install is refused.
pub fn remove(
    conn: &Conn,
    privilege: &Privilege,
    dir: &str,
    say: &mut dyn FnMut(MoveStep),
) -> anyhow::Result<()> {
    privilege.run(conn, &command("remove", &[dir]), &[], STEP_TIMEOUT)?;
    say(MoveStep::Removed);
    Ok(())
}
