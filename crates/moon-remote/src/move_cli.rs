//! The `core-move*` commands: move a MoonBot core onto a Linux server from the command line, the
//! same steps the terminal's window runs.
//!
//! ```text
//! moon-remote --data <dir> core-move --host <h> [--port 22] --login <user> --login-key <file>
//!             [--host-key <SHA256:...>] [--zip <settings snapshot>] [--exchange <name>]
//!             (--core <name|uid> [--no-report-db] [--no-strategy-sync | --strategies-off]
//!              [--from <host:port>]
//!              | --key-file <packet> | --new-key)
//!             --dir <instance dir> --proto-port <udp> [--public-ip <ipv4>]
//!             [--package <url>] [--key-out <file>] [--old-stopped]
//! moon-remote --data <dir> core-move-start  --host <h> ... --dir <instance dir> --old-stopped
//! moon-remote --data <dir> core-move-finish --host <h> ... --dir <instance dir>
//! moon-remote --data <dir> core-move-remove --host <h> ... --dir <instance dir>
//! ```
//!
//! `--core` keeps that core's MoonProto key, rebuilds its report database from this terminal's
//! replica, takes the settings snapshot straight from the running core unless `--zip` is given
//! (at the key's address, or `--from`), and carries its strategy set over MoonProto. The settings
//! carry the bot's exchange API keys, so they leave this machine only on a typed "yes".
//!
//! The new core is installed, then the old one is asked to stop, and only once it is seen to be
//! down does the new one start — two cores on one account trade against each other. A core with
//! unfinished deals refuses to stop; the command then asks again every minute until its user
//! stops it. Ctrl+C there leaves the new core installed and not started: `core-move-start
//! --old-stopped` starts it later, without the strategy carry, and `core-move-remove` takes it
//! out. When the old core's address is not known (`--new-key`, a packet without one),
//! `--old-stopped` says it is down.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use moon_core::core_move::old_core::StopAnswer;
use moon_remote::core_move::plan::{KeySource, Plan, Request};
use moon_remote::core_move::{self, MoveStep};
use moon_remote::hosts;
use moon_remote::script::Privilege;
use moon_remote::ssh::{Auth, Conn, Target};

use super::{Args, read_key};

/// How long the command waits between two stop requests to an old core that refuses.
const STOP_RETRY: Duration = Duration::from_secs(60);

/// Run one `core-move*` command.
pub fn run(command: &str, target: Target, args: &mut Args) -> anyhow::Result<()> {
    let mut say = |step: MoveStep| println!("{step:?}");
    match command {
        "core-move" => move_core(target, args, &mut say),
        "core-move-start" => {
            let dir = required(args, "--dir")?;
            // Nothing here can see the old core: starting next to it is the user's call to make.
            anyhow::ensure!(
                args.flag("--old-stopped"),
                "core-move-start starts the new core next to whatever runs: stop the old core \
                 first, then pass --old-stopped"
            );
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            core_move::start(&conn, &privilege, &dir, &mut say)?;
            core_move::finish(&conn, &privilege, &dir, &mut say)
        }
        "core-move-finish" => {
            let dir = required(args, "--dir")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            core_move::finish(&conn, &privilege, &dir, &mut say)
        }
        "core-move-remove" => {
            let dir = required(args, "--dir")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            core_move::remove(&conn, &privilege, &dir, &mut say)
        }
        other => anyhow::bail!("unknown command {other:?}"),
    }
}

/// `core-move`: gather the move, install the new core, stop the old one, start the new one.
fn move_core(target: Target, args: &mut Args, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<()> {
    let zip = args.value("--zip")?.map(PathBuf::from);
    let exchange = args.value("--exchange")?;
    let core = args.value("--core")?;
    let key_file = args.value("--key-file")?;
    let new_key = args.flag("--new-key");
    let no_db = args.flag("--no-report-db");
    let no_strategy_sync = args.flag("--no-strategy-sync");
    let strategies_off = args.flag("--strategies-off");
    let old_stopped = args.flag("--old-stopped");
    let from = args.value("--from")?;
    let dir = required(args, "--dir")?;
    let proto_port: u16 = required(args, "--proto-port")?
        .parse()
        .context("--proto-port")?;
    let package = args
        .value("--package")?
        .unwrap_or_else(|| core_move::TEST_PACKAGE.to_owned());
    let key_out = args.value("--key-out")?;
    let public_ip = match args.value("--public-ip")? {
        Some(ip) => ip.parse::<Ipv4Addr>().context("--public-ip")?,
        None => target.host.parse::<Ipv4Addr>().map_err(|_| {
            anyhow::anyhow!("--host is not an IPv4 address: give the server's --public-ip")
        })?,
    };
    let key = match (core, key_file, new_key) {
        (Some(pick), None, false) => KeySource::Core(pick),
        (None, Some(path), false) => KeySource::PacketFile(path.into()),
        (None, None, true) => {
            anyhow::ensure!(
                key_out.is_some(),
                "--new-key needs --key-out <file> to keep it"
            );
            println!(
                "A NEW MoonProto key: every terminal of this core will need the new packet. The \
                 core keeps the transport mode its settings name; the packet says V0."
            );
            KeySource::NewKey
        }
        _ => {
            anyhow::bail!("give exactly one of --core <name|uid>, --key-file <packet> or --new-key")
        }
    };
    let request = Request {
        key,
        zip,
        exchange,
        from,
        report_db: !no_db,
        strategy_sync: !no_strategy_sync,
        strategies_off,
        public_ip,
        proto_port,
    };
    let (conn, privilege) = connect(&target, args)?;
    args.done()?;

    let mut plan = Plan::build(&request, say)?;
    // `--old-stopped` stands in for the stop only where the move cannot stop the old core itself.
    anyhow::ensure!(
        plan.has_old_core() != old_stopped,
        "{}",
        if old_stopped {
            "--old-stopped is for an old core whose address is not known; this one is stopped by \
             the move itself"
        } else {
            "the old core's address is not known, so it cannot be stopped from here: stop it, then \
             pass --old-stopped"
        }
    );
    confirm_secrets(&target)?;

    // Written before the install: the core consumes its key file on the first start, so a
    // packet that could not be saved afterwards would leave a core nobody can reach.
    if let Some(path) = &key_out {
        std::fs::write(path, &plan.packet_text).with_context(|| format!("write {path}"))?;
        println!("connection packet written to {path} (valid once the move succeeds)");
    }
    core_move::check(&conn, &privilege, proto_port, say)?;
    let installed = core_move::install(
        &conn,
        &privilege,
        &core_move::Install {
            dir: &dir,
            proto_port,
            package_url: &package,
            archive: &plan.archive.bytes,
            key_packet: &plan.packet_text,
            ini_flag_added: plan.archive.ini_flag_added,
        },
        say,
    )?;
    println!(
        "MOVE_DIRECTORY={}\nMOVE_SERVICE={}",
        installed.dir, installed.unit
    );
    let dir = installed.dir.as_str();
    // `core-move-start` carries no strategies: where the carry is required, only a fresh move
    // brings them over.
    let later = if plan.carry_required && plan.strategies.is_some() {
        format!(
            "the strategies must be carried, which only the move does: core-move-remove --dir \
             {dir} and move again once the old core is down"
        )
    } else {
        format!(
            "core-move-start --dir {dir} --old-stopped once the old core is down (the snapshot's \
             strategy file stands in for the carry), or core-move-remove --dir {dir}"
        )
    };
    let pending = || format!("the new core is installed at {dir}, not started: {later}");
    if plan.has_old_core() {
        loop {
            // Read again before every request: the old core kept trading meanwhile.
            plan.refresh_strategies(say);
            let StopAnswer::Refused { open_deals } = plan.stop_old(say).with_context(pending)?
            else {
                break;
            };
            println!(
                "The old core keeps running ({} unfinished deals). Stop it in MoonBot; asking \
                 again in {} s. Ctrl+C leaves the new core installed, not started.",
                open_deals.map_or("?".to_owned(), |n| n.to_string()),
                STOP_RETRY.as_secs()
            );
            std::thread::sleep(STOP_RETRY);
        }
    }
    core_move::start(&conn, &privilege, dir, say).with_context(|| {
        format!(
            "the old core is down and the new one did not start: {later}; or undo with \
             core-move-remove --dir {dir} and start the old core again"
        )
    })?;
    plan.carry_strategies(say).with_context(|| {
        format!("the new core runs at {dir}; undo with core-move-remove --dir {dir}")
    })?;
    core_move::finish(&conn, &privilege, dir, say)?;
    if key_out.is_none() {
        println!("connection packet:\n{}", plan.packet_text);
    }
    println!(
        "Done: switch this core's terminals to the new packet. To undo: core-move-remove --dir \
         {dir}, then start the old core again"
    );
    Ok(())
}

/// A required `--name value`.
fn required(args: &mut Args, name: &str) -> anyhow::Result<String> {
    args.value(name)?
        .ok_or_else(|| anyhow::anyhow!("{name} is required"))
}

/// Log in with `--login`/`--login-key` against the pinned or `--host-key` fingerprint, and find
/// how that login reaches root.
fn connect(target: &Target, args: &mut Args) -> anyhow::Result<(Conn, Privilege)> {
    let login = required(args, "--login")?;
    let key = read_key(&required(args, "--login-key")?)?;
    let pin = match args.value("--host-key")? {
        Some(pin) => pin,
        None => match hosts::Hosts::load(&hosts::Hosts::path())?.get(&target.addr()) {
            Some(host) => host.fingerprint.clone(),
            None => {
                let seen = moon_remote::ssh::presented_key(target)?;
                anyhow::bail!(
                    "host key of {}: {seen}; re-run with --host-key {seen} after checking it",
                    target.addr()
                );
            }
        },
    };
    let conn = Conn::open(
        target,
        &Auth::Key {
            user: &login,
            key: &key,
        },
        &pin,
    )?;
    let password = std::env::var("MOON_REMOTE_LOGIN_PASSWORD")
        .ok()
        .map(zeroize::Zeroizing::new);
    let privilege = Privilege::detect(&conn, password.as_ref())?;
    Ok((conn, privilege))
}

/// The settings archive carries the bot's exchange API keys: they leave only on a typed "yes".
fn confirm_secrets(target: &Target) -> anyhow::Result<()> {
    print!(
        "The settings archive carries the bot's EXCHANGE API KEYS and goes to {}. Type yes: ",
        target.addr()
    );
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(answer.trim() == "yes", "not sent");
    Ok(())
}
