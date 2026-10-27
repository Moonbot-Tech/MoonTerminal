//! The `core-move*` commands: move a MoonBot core onto a Linux server, by hand, before the
//! terminal has a window for it.
//!
//! ```text
//! moon-remote --data <dir> core-move --host <h> [--port 22] --login <user> --login-key <file>
//!             [--host-key <SHA256:...>] [--zip <settings snapshot>] [--exchange <name>]
//!             (--core <name|uid> [--no-report-db] [--no-strategy-sync | --strategies-off]
//!              [--from <host:port>]
//!              | --key-file <packet> | --new-key)
//!             --dir <instance dir> --proto-port <udp> [--public-ip <ipv4>]
//!             [--package <url>] [--key-out <file>]
//! moon-remote --data <dir> core-move-finish --host <h> ... --dir <instance dir>
//! moon-remote --data <dir> core-move-remove --host <h> ... --dir <instance dir>
//! ```
//!
//! `--core` keeps that core's MoonProto key and rebuilds its report database from this
//! terminal's replica; the packet printed at the end is the same key at the new address. Without
//! `--zip` it takes the settings snapshot straight from the running old core (at the key's
//! address, or `--from`); a core whose build does not serve one needs the ZIP MoonBot's VPS
//! wizard saves. It also
//! reads the old core's strategy set over MoonProto before the install (at the same address)
//! and writes it to the new core once it runs — required when the snapshot carries no
//! strategy file or with `--strategies-off` (carried switched off), a warning on failure
//! otherwise. The
//! settings snapshot carries the bot's exchange API keys, so they leave this machine only on a
//! typed "yes". The old core keeps running: stop it once the new one answers.

use std::net::Ipv4Addr;
use std::path::PathBuf;

use anyhow::Context;
use moon_core::config::TransportVersion;
use moon_core::core_move::archive;
use moon_core::core_move::key_packet::ConnectionPacket;
use moon_core::core_move::report_db;
use moon_core::core_move::snapshot;
use moon_core::core_move::strategies::{self, StrategySet};
use moon_remote::core_move::{self, MoveStep};
use moon_remote::hosts;
use moon_remote::script::Privilege;
use moon_remote::ssh::{Auth, Conn, Target};

use super::{Args, read_key};

/// Run one `core-move*` command.
pub fn run(command: &str, target: Target, args: &mut Args) -> anyhow::Result<()> {
    let mut say = |step: MoveStep| println!("{step:?}");
    match command {
        "core-move" => move_core(target, args, &mut say),
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

/// `core-move`: compose the archive, check the server, install and start the core.
fn move_core(target: Target, args: &mut Args, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<()> {
    let zip = args.value("--zip")?.map(PathBuf::from);
    let exchange = args.value("--exchange")?;
    let core = args.value("--core")?;
    let key_file = args.value("--key-file")?;
    let new_key = args.flag("--new-key");
    let no_db = args.flag("--no-report-db");
    let no_strategy_sync = args.flag("--no-strategy-sync");
    let strategies_off = args.flag("--strategies-off");
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
    anyhow::ensure!(
        [core.is_some(), key_file.is_some(), new_key]
            .iter()
            .filter(|b| **b)
            .count()
            == 1,
        "give exactly one of --core <name|uid>, --key-file <packet> or --new-key"
    );
    anyhow::ensure!(
        zip.is_some() || core.is_some(),
        "give --zip <settings snapshot>: only a move with --core can take the snapshot from the \
         running core instead"
    );
    anyhow::ensure!(
        !strategies_off || (core.is_some() && !no_strategy_sync),
        "--strategies-off belongs to the strategy carry: it needs --core and no \
         --no-strategy-sync"
    );
    anyhow::ensure!(
        from.is_none() || core.is_some(),
        "--from names the old core: it needs --core"
    );
    let (conn, privilege) = connect(&target, args)?;
    args.done()?;

    let (packet, uid) = match (&core, &key_file) {
        (Some(pick), _) => {
            let (uid, packet) = core_packet(pick)?;
            (packet, Some(uid))
        }
        (None, Some(path)) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
            let packet = ConnectionPacket::parse(&text)
                .ok_or_else(|| anyhow::anyhow!("{path} is not a MoonBot connection packet"))?;
            (packet, None)
        }
        (None, None) => {
            anyhow::ensure!(
                key_out.is_some(),
                "--new-key needs --key-out <file> to keep it"
            );
            println!(
                "A NEW MoonProto key: every terminal of this core will need the new packet. The \
                 core keeps the transport mode its settings name; the packet says V0."
            );
            let packet = ConnectionPacket::generate(public_ip, proto_port, TransportVersion::V0)?;
            (packet, None)
        }
    };
    let old_packet = packet.clone();
    let packet = packet.with_endpoint(public_ip, proto_port);

    // The exchange is the user's `--exchange`, else the one the old core reports for itself
    // (with `--core`); without either, `archive::compose` reads it off the strategy files.
    let (settings, reported) = match &zip {
        Some(path) => {
            let bytes = zeroize::Zeroizing::new(
                std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
            );
            // Asked only when the strategy files cannot settle it — a ZIP of one exchange never
            // needs the old core, which may already be stopped.
            let ambiguous = archive::strategy_exchanges(&bytes)?.len() > 1;
            let reported = if exchange.is_none() && core.is_some() && ambiguous {
                let ask = || -> anyhow::Result<Option<String>> {
                    let (host, port) = old_core_endpoint(&old_packet, from.as_deref())?;
                    let cfg = strategies::client_config(
                        &old_packet.export()?,
                        &host,
                        port,
                        Some(old_packet.mode.into()),
                    )?;
                    snapshot::exchange_of(cfg, STRATEGY_TIMEOUT)
                };
                match ask() {
                    Ok(name) => name,
                    Err(e) => {
                        println!(
                            "WARNING: the old core's exchange could not be asked ({e:#}); \
                             reading it off the snapshot's strategy files"
                        );
                        None
                    }
                }
            } else {
                None
            };
            (bytes, reported)
        }
        None => {
            let (host, port) = old_core_endpoint(&old_packet, from.as_deref())?;
            let cfg = strategies::client_config(
                &old_packet.export()?,
                &host,
                port,
                Some(old_packet.mode.into()),
            )?;
            let taken = snapshot::download(cfg, STRATEGY_TIMEOUT)
                .with_context(|| format!("take the settings snapshot from {host}:{port}"))?;
            println!(
                "settings snapshot: {} bytes from the core at {host}:{port}, exchange {}",
                taken.bytes.len(),
                taken.exchange.as_deref().unwrap_or("not reported")
            );
            (taken.bytes, taken.exchange)
        }
    };
    // A core-reported name decides where the rebuilt report database goes, so it must be one the
    // snapshot's strategy files carry too: a mismatch would file the database under a name the
    // new core never opens. The user's `--exchange` is taken as given.
    let exchange = match (exchange, reported) {
        (Some(chosen), _) => Some(chosen),
        (None, Some(reported)) => {
            let known = archive::strategy_exchanges(&settings)?;
            anyhow::ensure!(
                known.is_empty() || known.contains(&reported),
                "the old core reports exchange {reported:?}, but the snapshot's strategy files \
                 name {}: pass the core's exchange with --exchange",
                known.join(", ")
            );
            println!("exchange: {reported} (reported by the old core)");
            Some(reported)
        }
        (None, None) => None,
    };
    let db_file = match (uid, no_db) {
        (Some(uid), false) => Some(rebuild_db(uid)?),
        _ => None,
    };
    let composed = archive::compose(
        &settings,
        exchange.as_deref(),
        db_file.as_ref().map(|f| f.path.as_path()),
    );
    // Both carry secrets — the API keys, the trade history — and nothing below reads them.
    drop(settings);
    drop(db_file);
    let composed = composed?;
    println!(
        "exchange: {} · report database: {} · first-start INI switch added: {}",
        composed.exchange,
        composed.report_db.as_deref().unwrap_or("none"),
        composed.ini_flag_added
    );
    // The carry may fall back to the snapshot's own strategy file only when there is one and the
    // user did not ask for the strategies switched off — that file keeps them as they were.
    let carry_required = !composed.has_strategies || strategies_off;
    let strategy_set = match uid {
        Some(_) if !no_strategy_sync => {
            let mut set = read_old_strategies(&old_packet, from.as_deref(), carry_required)?;
            // Switched off, the new core holds the strategies but trades none until someone
            // checks them — a move that wants the new core verified before it trades.
            if strategies_off {
                for strategy in set.iter_mut().flat_map(|s| s.strategies.iter_mut()) {
                    strategy.checked = false;
                }
            }
            set
        }
        _ => {
            anyhow::ensure!(
                composed.has_strategies || no_strategy_sync,
                "the snapshot carries no strategy file: the new core would start with a default \
                 set, and terminals would read the real strategies as deleted. Move with --core, \
                 add the strategy file to the ZIP, or pass --no-strategy-sync knowingly"
            );
            None
        }
    };
    confirm_secrets(&target)?;

    // Written before the install: the core consumes its key file on the first start, so a
    // packet that could not be saved afterwards would leave a core nobody can reach.
    let text = packet.export()?;
    if let Some(path) = &key_out {
        std::fs::write(path, &text).with_context(|| format!("write {path}"))?;
        println!("connection packet written to {path} (valid once the install succeeds)");
    }
    core_move::check(&conn, &privilege, proto_port, say)?;
    let installed = core_move::install(
        &conn,
        &privilege,
        &core_move::Install {
            dir: &dir,
            proto_port,
            package_url: &package,
            archive: &composed.bytes,
            key_packet: &text,
            ini_flag_added: composed.ini_flag_added,
        },
        say,
    )?;
    println!(
        "MOVE_DIRECTORY={}\nMOVE_SERVICE={}",
        installed.dir, installed.unit
    );
    if key_out.is_none() {
        println!("connection packet:\n{text}");
    }
    if let Some(set) = &strategy_set {
        let cfg = strategies::client_config(
            &text,
            &public_ip.to_string(),
            proto_port,
            Some(packet.mode.into()),
        )?;
        match strategies::write_set(cfg, set, STRATEGY_TIMEOUT) {
            Ok(carried) => println!(
                "strategies: {} written, {} of the new core's own deleted",
                carried.written, carried.deleted
            ),
            Err(e) if !carry_required => println!(
                "WARNING: strategies not confirmed as the old core's ({e:#}); the new core may \
                 hold the snapshot's file partly overwritten — check them in a terminal before \
                 switching terminals to it"
            ),
            Err(e) => {
                return Err(e.context(format!(
                    "the new core's strategies are NOT confirmed as the old core's (they may be \
                     partly written): do not switch terminals to it. Retry, or undo with \
                     core-move-remove --dir {}",
                    installed.dir
                )));
            }
        }
    }
    println!(
        "Next: check the core from a terminal, then run core-move-finish --dir {} and stop the \
         old core. To undo: core-move-remove --dir {}",
        installed.dir, installed.dir
    );
    Ok(())
}

/// How long a strategy read or write waits for a core: connect, log in to the exchange, answer.
const STRATEGY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// The old core's strategy set, read over MoonProto at its packet's address or `from`. When it
/// cannot be read: an error if the carry is `required` (no strategy file in the snapshot, or the
/// strategies are to arrive switched off), else `None` with a warning — the snapshot's own file
/// then stands in.
fn read_old_strategies(
    packet: &ConnectionPacket,
    from: Option<&str>,
    required: bool,
) -> anyhow::Result<Option<StrategySet>> {
    let read = || -> anyhow::Result<StrategySet> {
        let (host, port) = old_core_endpoint(packet, from)?;
        let text = packet.export()?;
        let cfg = strategies::client_config(&text, &host, port, Some(packet.mode.into()))?;
        let set = strategies::read_set(cfg, STRATEGY_TIMEOUT)
            .with_context(|| format!("read the old core's strategies at {host}:{port}"))?;
        println!(
            "old core: {} strategies, {} folders",
            set.strategies.len(),
            set.folders.len()
        );
        Ok(set)
    };
    match read() {
        Ok(set) => Ok(Some(set)),
        Err(e) if !required => {
            println!("WARNING: {e:#}; the new core keeps the snapshot's strategy file");
            Ok(None)
        }
        Err(e) => Err(e.context(
            "the old core's strategy set could not be read, and the move needs it (no strategy \
             file in the snapshot, or --strategies-off)",
        )),
    }
}

/// Where the old core answers: `from` when given, else the address its packet names.
fn old_core_endpoint(
    packet: &ConnectionPacket,
    from: Option<&str>,
) -> anyhow::Result<(String, u16)> {
    match from {
        Some(endpoint) => {
            let (host, port) = endpoint
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("--from wants <host:port>"))?;
            Ok((host.to_owned(), port.parse::<u16>().context("--from port")?))
        }
        None => {
            let ip = packet.ipv4.ok_or_else(|| {
                anyhow::anyhow!("the core's key names no address: give --from <host:port>")
            })?;
            Ok((ip.to_string(), packet.port))
        }
    }
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

/// The uid and connection packet of a core of this terminal, by uid or name.
fn core_packet(pick: &str) -> anyhow::Result<(u64, ConnectionPacket)> {
    let all = moon_core::config::read_core_keys()?;
    let mut found = all
        .iter()
        .filter(|e| e.uid.to_string() == pick || e.name.eq_ignore_ascii_case(pick));
    let entry = found
        .next()
        .ok_or_else(|| anyhow::anyhow!("no core {pick:?} in servers.enc"))?;
    anyhow::ensure!(
        found.next().is_none(),
        "{pick:?} names more than one core: give its uid"
    );
    anyhow::ensure!(
        entry.uid != 0,
        "core {:?} has no uid yet: start the terminal once",
        entry.name
    );
    let mut packet = ConnectionPacket::parse(entry.key.expose())
        .ok_or_else(|| anyhow::anyhow!("core {:?}: its key is not a MoonBot packet", entry.name))?;
    if let Some(mode) = entry.transport {
        packet.mode = mode;
    }
    Ok((entry.uid, packet))
}

/// A rebuilt report database in a temporary file, deleted when dropped.
struct TempDb {
    path: PathBuf,
}

impl Drop for TempDb {
    /// The file holds the core's whole trade history: it never outlives the move.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Rebuild core `uid`'s report database from the replica of the `--data` root.
fn rebuild_db(uid: u64) -> anyhow::Result<TempDb> {
    let replica = moon_core::config::paths::reports_db_path_no_create();
    anyhow::ensure!(
        replica.exists(),
        "no report replica at {}: pass --no-report-db",
        replica.display()
    );
    let path = std::env::temp_dir().join(format!("moon-core-move-{}-{uid}.db", std::process::id()));
    let file = TempDb { path };
    let summary = report_db::build(&replica, uid, &file.path)?;
    println!(
        "report database: {} rows ({} open), epoch {}, high-water {}",
        summary.rows,
        summary.open_rows,
        summary.epoch.map_or(
            "none — terminals will download the history again".to_owned(),
            |e| e.to_string()
        ),
        summary.high_water
    );
    Ok(file)
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
