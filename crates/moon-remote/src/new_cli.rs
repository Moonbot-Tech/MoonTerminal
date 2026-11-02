//! The `core-new*` commands: put a fresh MoonBot core on a Linux server from the command line, the
//! same steps the terminal's connect wizard runs.
//!
//! ```text
//! moon-remote --data <dir> core-new --host <h> [--port 22] --login <user> --login-key <file>
//!             [--host-key <SHA256:...>] --dir <instance dir> --proto-port <udp>
//!             [--public-ip <ipv4>] [--package <url>] [--key-out <file>] [--exchange <id>]
//! moon-remote --data <dir> core-new-status    --host <h> ... --dir <instance dir>
//! moon-remote --data <dir> core-new-exchanges --host <h> ... --dir <instance dir>
//! moon-remote --data <dir> core-new-keys      --host <h> ... --dir <instance dir> --exchange <id>
//! moon-remote --data <dir> core-new-retry     --host <h> ... --dir <instance dir>
//! ```
//!
//! `core-new` installs the package and the service, starts the core on its own default settings,
//! turns on its MoonProto server and prints the connection key (or writes it to `--key-out`).
//! With `--exchange` it goes on as `core-new-keys`. The exchange keys are asked without echo, or
//! taken from `MOON_REMOTE_API_KEY` / `MOON_REMOTE_API_SECRET`, and reach the core on stdin only.
//! `core-move-remove --dir` takes the instance out again.

use std::net::Ipv4Addr;

use anyhow::Context;
use moon_remote::core_move;
use moon_remote::core_new::{self, ApiKeys, CoreStatus};
use moon_remote::script::Privilege;
use moon_remote::ssh::{Conn, Target};
use zeroize::Zeroizing;

use super::Args;
use super::move_cli::{connect, required};

/// Run one `core-new*` command.
pub fn run(command: &str, target: Target, args: &mut Args) -> anyhow::Result<()> {
    match command {
        "core-new" => new_core(target, args),
        "core-new-status" => {
            let dir = required(args, "--dir")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            print_status(&core_new::status(&conn, &privilege, &dir)?);
            Ok(())
        }
        "core-new-exchanges" => {
            let dir = required(args, "--dir")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            for ex in core_new::exchanges(&conn, &privilege, &dir)? {
                println!(
                    "{:<22} {:<20} passphrase={:?} rsa={}",
                    ex.id, ex.name, ex.required_passphrase, ex.rsa_supported
                );
            }
            Ok(())
        }
        "core-new-keys" => {
            let dir = required(args, "--dir")?;
            let exchange = required(args, "--exchange")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            give_keys(&conn, &privilege, &dir, &exchange)
        }
        "core-new-retry" => {
            let dir = required(args, "--dir")?;
            let (conn, privilege) = connect(&target, args)?;
            args.done()?;
            core_new::retry_login(&conn, &privilege, &dir)?;
            settle(&conn, &privilege, &dir)
        }
        other => anyhow::bail!("unknown command {other:?}"),
    }
}

/// `core-new`: install, start, turn MoonProto on, and with `--exchange` give the keys.
fn new_core(target: Target, args: &mut Args) -> anyhow::Result<()> {
    let dir = required(args, "--dir")?;
    let proto_port: u16 = required(args, "--proto-port")?
        .parse()
        .context("--proto-port")?;
    let package = args
        .value("--package")?
        .unwrap_or_else(|| core_move::TEST_PACKAGE.to_owned());
    let key_out = args.value("--key-out")?;
    let exchange = args.value("--exchange")?;
    let public_ip = match args.value("--public-ip")? {
        Some(ip) => ip.parse::<Ipv4Addr>().context("--public-ip")?,
        None => target.host.parse::<Ipv4Addr>().map_err(|_| {
            anyhow::anyhow!("--host is not an IPv4 address: give the server's --public-ip")
        })?,
    };
    let (conn, privilege) = connect(&target, args)?;
    args.done()?;

    let mut say = |step: core_move::MoveStep| println!("{step:?}");
    let check = core_move::check(&conn, &privilege, proto_port, &mut say)?;
    println!("{check:?}");
    let installed = core_new::install(&conn, &privilege, &dir, proto_port, &package)?;
    println!(
        "MOVE_DIRECTORY={}\nMOVE_SERVICE={}",
        installed.dir, installed.unit
    );
    // Everything past the install leaves a running core behind on failure: say how to go on.
    let installed_dir = installed.dir.clone();
    let after = || -> anyhow::Result<()> {
        let key =
            core_new::setup_moonproto(&conn, &privilege, &installed.dir, public_ip, proto_port)?;
        match &key_out {
            Some(path) => {
                std::fs::write(path, key.as_bytes()).with_context(|| format!("write {path}"))?;
                println!("connection key written to {path}");
            }
            None => println!("connection key:\n{}", key.as_str()),
        }
        match &exchange {
            Some(exchange) => give_keys(&conn, &privilege, &installed.dir, exchange),
            None => {
                print_status(&core_new::status(&conn, &privilege, &installed.dir)?);
                Ok(())
            }
        }
    };
    after().with_context(|| {
        format!(
            "the core runs at {installed_dir}: core-new-keys or core-new-retry --dir \
             {installed_dir} once fixed, or remove it with core-move-remove --dir {installed_dir}"
        )
    })
}

/// Select `exchange`, give it the keys the user types, and wait for the login to settle.
fn give_keys(conn: &Conn, privilege: &Privilege, dir: &str, exchange: &str) -> anyhow::Result<()> {
    let known = core_new::exchanges(conn, privilege, dir)?;
    anyhow::ensure!(
        known.iter().any(|ex| ex.id == exchange),
        "the core knows no exchange {exchange:?}: core-new-exchanges lists them"
    );
    let keys = ApiKeys::Pair {
        api_key: secret("MOON_REMOTE_API_KEY", "API key: ")?,
        api_secret: secret("MOON_REMOTE_API_SECRET", "API secret: ")?,
    };
    core_new::select_exchange(conn, privilege, dir, exchange)?;
    core_new::set_api_keys(conn, privilege, dir, &keys)?;
    settle(conn, privilege, dir)
}

/// Wait for the login to settle, printing each status that differs from the one before.
fn settle(conn: &Conn, privilege: &Privilege, dir: &str) -> anyhow::Result<()> {
    let mut last: Option<CoreStatus> = None;
    let end = core_new::wait_settled(conn, privilege, dir, core_new::SETTLE_TIMEOUT, &mut |s| {
        if last.as_ref() != Some(s) {
            print_status(s);
            last = Some(s.clone());
        }
    })?;
    anyhow::ensure!(
        end.connectable(),
        "the core is not ready: {:?} {}",
        end.state,
        end.login_error
    );
    println!("ready: a terminal can connect with the key");
    Ok(())
}

fn print_status(s: &CoreStatus) {
    println!(
        "state={:?} exchange={} keys={} login_error={:?} moonproto: enabled={} listening={} \
         port={} error={:?} emulator={:?}",
        s.state,
        s.exchange,
        s.has_api_keys,
        s.login_error,
        s.moonproto_enabled,
        s.moonproto_listening,
        s.moonproto_port,
        s.moonproto_error,
        s.emulator
    );
}

/// A secret from `var`, or asked without echo.
fn secret(var: &str, prompt: &str) -> anyhow::Result<Zeroizing<String>> {
    match std::env::var(var) {
        Ok(value) if !value.is_empty() => Ok(Zeroizing::new(value)),
        _ => Ok(Zeroizing::new(rpassword::prompt_password(prompt)?)),
    }
}
