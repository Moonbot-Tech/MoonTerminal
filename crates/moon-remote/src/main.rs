//! `moon-remote`: command-line server setup and station administration, alongside the terminal UI.
//!
//! ```text
//! moon-remote --data <terminal data dir> setup  --host <h> [--port 22] --login <user>
//!             [--login-key <file>] [--station-bin <file> | --station-release]
//!             [--host-key <SHA256:...>]
//! moon-remote --data <dir> station-bin --host <h> [--port 22] --bin <file>
//! moon-remote --data <dir> station-update --host <h> [--port 22]
//! Core picks only add; rename/key updates use the terminal's Cores on the station block.
//! moon-remote --data <dir> cores  --host <h> [--port 22] (--from-terminal --core <name|uid>… | --dummy <uid>:<name>…)
//! moon-remote --data <dir> cores  --host <h> [--port 22] --remove <uid>…
//! Core pushes only add; removal keeps report history and requires a remaining core.
//! --remove of an absent uid is an idempotent credential cleanup and can race an add from
//! another terminal; the terminal UI guards removal selections with removal_matches.
//! moon-remote --data <dir> status --host <h> [--port 22] [--logs <n>]
//! moon-remote --data <dir> telegram --host <h> [--port 22] (--off | --state | [--token]
//!             [--mini-app on|off] [--zone <IANA zone>] [--language ru|en|es|uk|tr|pt|vi])
//! ```
//!
//! A server not set up from this machine yet needs `--host-key`: without it `setup` only prints the
//! key the server presents, to be checked in the provider's console, and sends nothing to it.
//!
//! The administrator is always `moon`, by key only. Passwords are asked without echo, or taken
//! from `MOON_REMOTE_LOGIN_PASSWORD` (the provider's login, or sudo for `--login-key`) and, for a
//! server set up before key-only sudo, `MOON_REMOTE_OLD_ADMIN_PASSWORD`. None is stored anywhere.
//! The bot token (`telegram --token`) likewise: asked without echo, or `MOON_REMOTE_BOT_TOKEN`.

use std::path::PathBuf;

use anyhow::Context;
use moon_core::config::Secret;
use moon_remote::error::StationError;
use moon_remote::setup::{FirstAccess, Setup};
use moon_remote::ssh::Target;
use moon_remote::station::CoreKey;
use moon_remote::{app_key, hosts, keys, script, setup, station};
use zeroize::Zeroizing;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// Execute the selected setup or station operation, rejecting incompatible CLI options.
fn run() -> anyhow::Result<()> {
    let mut args = Args(std::env::args().skip(1).collect());
    let data = args
        .value("--data")?
        .ok_or_else(|| anyhow::anyhow!("--data <terminal data dir> is required"))?;
    anyhow::ensure!(
        moon_core::config::paths::set_data_dir_override(PathBuf::from(data)),
        "data root already set"
    );
    anyhow::ensure!(
        !args.0.is_empty(),
        "no command: setup | station-bin | station-update | cores | status | telegram"
    );
    let command = args.0.remove(0);
    let target = Target {
        host: args
            .value("--host")?
            .ok_or_else(|| anyhow::anyhow!("--host is required"))?,
        port: args
            .value("--port")?
            .map(|p| p.parse())
            .transpose()?
            .unwrap_or(22),
    };
    let mut say = |line: moon_remote::progress::Progress| println!("{line}");
    match command.as_str() {
        "setup" => {
            let host_key = args.value("--host-key")?;
            let login = args
                .value("--login")?
                .ok_or_else(|| anyhow::anyhow!("--login <user> is required"))?;
            let first = match args.value("--login-key")? {
                Some(path) => FirstAccess::Key {
                    user: login.clone(),
                    key: Box::new(read_key(&path)?),
                    sudo_password: (login != "root")
                        .then(|| {
                            secret(
                                "MOON_REMOTE_LOGIN_PASSWORD",
                                &format!("sudo password of {login}: "),
                            )
                        })
                        .transpose()?,
                },
                None => FirstAccess::Password {
                    password: secret(
                        "MOON_REMOTE_LOGIN_PASSWORD",
                        &format!("password of {login}: "),
                    )?,
                    user: login,
                },
            };
            let station = match (args.value("--station-bin")?, args.flag("--station-release")) {
                (Some(path), false) => setup::StationBinary::File(PathBuf::from(path)),
                (None, true) => setup::StationBinary::Release,
                (None, false) => setup::StationBinary::Keep,
                (Some(_), true) => {
                    anyhow::bail!("give --station-bin or --station-release, not both")
                }
            };
            args.done()?;
            let mut setup = Setup {
                target,
                first,
                legacy_admin_password: std::env::var("MOON_REMOTE_OLD_ADMIN_PASSWORD")
                    .ok()
                    .map(Zeroizing::new),
                station,
                host_key,
            };
            if let Err(e) = setup::run(&setup, &mut say) {
                if matches!(
                    e.downcast_ref::<StationError>(),
                    Some(StationError::HostKeyUnconfirmed)
                ) {
                    let seen = setup::probe_host_key(setup.target.clone())?;
                    anyhow::bail!(
                        "host key of {}: {}; re-run with --host-key {} after checking it in the \
                         provider console",
                        seen.target.addr(),
                        seen.fingerprint,
                        seen.fingerprint
                    );
                }
                let Some(old) = e.downcast_ref::<setup::NeedsAdminPassword>() else {
                    return Err(e);
                };
                if setup.legacy_admin_password.is_some() {
                    return Err(e);
                }
                println!("{old}");
                setup.legacy_admin_password = Some(secret(
                    "MOON_REMOTE_OLD_ADMIN_PASSWORD",
                    &format!("old password of {}: ", old.admin),
                )?);
                setup::run(&setup, &mut say)?;
            }
            println!("done: the server is closed; the station has no cores yet");
        }
        "station-bin" => {
            let bin = args
                .value("--bin")?
                .ok_or_else(|| anyhow::anyhow!("--bin <file> is required"))?;
            args.done()?;
            let known = hosts::Hosts::load(&hosts::Hosts::path())?;
            let host = known
                .get(&target.addr())
                .ok_or_else(|| anyhow::anyhow!("{} was never set up here", target.addr()))?;
            let admin = host
                .admin
                .clone()
                .ok_or_else(|| anyhow::anyhow!("{}: the setup did not finish", target.addr()))?;
            let app = app_key::load_or_create()?;
            setup::install_station(
                &target,
                &admin,
                &app,
                &host.fingerprint,
                std::path::Path::new(&bin),
                &mut say,
            )?;
        }
        "station-update" => {
            args.done()?;
            station::update_from_release(&target, &mut say)?;
        }
        "cores" => {
            let from_terminal = args.flag("--from-terminal");
            let picks = args.values("--core")?;
            let dummies = args.values("--dummy")?;
            let remove = args.values("--remove")?;
            args.done()?;
            if !remove.is_empty() {
                anyhow::ensure!(
                    !from_terminal && picks.is_empty() && dummies.is_empty(),
                    "--remove takes no push options"
                );
                let uids = remove
                    .iter()
                    .map(|uid| uid.parse::<u64>())
                    .collect::<Result<Vec<_>, _>>()?;
                station::remove_cores(&target, &uids, &mut say)?;
                return Ok(());
            }
            let cores = match (from_terminal, dummies.is_empty()) {
                (true, true) => terminal_cores(&picks)?,
                (false, false) => dummies
                    .iter()
                    .map(|d| dummy_core(d))
                    .collect::<anyhow::Result<_>>()?,
                _ => anyhow::bail!("give --from-terminal, --dummy <uid>:<name>, or --remove <uid>"),
            };
            let tape = if from_terminal {
                confirm_keys(&target, &cores)?;
                station::terminal_tape()
            } else {
                None
            };
            station::push_cores(&target, &cores, tape, &mut say)?;
        }
        "status" => {
            let logs = args.value("--logs")?;
            args.done()?;
            let conn = station::admin_conn(&target)?;
            let out = script::checked(conn.run(
                &script::helper("status", &[]),
                &[],
                script::STEP_TIMEOUT,
            )?)?;
            print!("{}", out.stdout_text());
            if let Some(lines) = logs {
                let out = script::checked(conn.run(
                    &script::helper("logs", &[&lines]),
                    &[],
                    script::STEP_TIMEOUT,
                )?)?;
                print!("{}", out.stdout_text());
            }
        }
        "telegram" => {
            if args.flag("--state") {
                args.done()?;
                let state = station::bot::bot_state(&target)?;
                println!("status={}", state.summary());
                println!("pairing_code={}", state.pairing_code().unwrap_or("-"));
                if let Some(access) = &state.access {
                    println!("chats={:?}", access.authorized_chat_ids);
                }
                return Ok(());
            }
            let off = args.flag("--off");
            let token = args.flag("--token");
            let change = station::BotChange {
                mini_app: args
                    .value("--mini-app")?
                    .map(|v| match v.as_str() {
                        "on" => Ok(true),
                        "off" => Ok(false),
                        other => Err(anyhow::anyhow!("--mini-app on|off, got {other:?}")),
                    })
                    .transpose()?,
                zone: args.value("--zone")?,
                language: args.value("--language")?,
            };
            args.done()?;
            anyhow::ensure!(
                !(off
                    && (token
                        || change.mini_app.is_some()
                        || change.zone.is_some()
                        || change.language.is_some())),
                "--off takes no other setting"
            );
            let token = token
                .then(|| secret("MOON_REMOTE_BOT_TOKEN", "bot token: "))
                .transpose()?
                .map(|t| Secret::new(t.trim()));
            station::push_telegram(&target, token.as_ref(), &change, off, &mut say)?;
        }
        other => anyhow::bail!(
            "unknown command {other:?}: setup | station-bin | station-update | cores | status | telegram"
        ),
    }
    Ok(())
}

/// The picked cores' keys from the terminal's `servers.enc`, read without writing anything
/// back. Every core must be named: full-access keys leave this machine only by choice.
fn terminal_cores(picks: &[String]) -> anyhow::Result<Vec<CoreKey>> {
    anyhow::ensure!(
        !picks.is_empty(),
        "--from-terminal needs --core <name|uid> for each core to send"
    );
    let all = moon_core::config::read_core_keys()?;
    let mut cores = Vec::new();
    for pick in picks {
        // A uid, or a name in any case — refused when two names differ in case only.
        let mut found = all
            .iter()
            .filter(|e| e.uid.to_string() == *pick || e.name.eq_ignore_ascii_case(pick));
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
        anyhow::ensure!(!entry.key.is_empty(), "core {:?} has no key", entry.name);
        anyhow::ensure!(
            entry.active,
            "core {:?} is switched off in the terminal: switch it on or leave it out",
            entry.name
        );
        cores.push(CoreKey {
            uid: entry.uid,
            name: entry.name.clone(),
            transport: entry.transport,
            key: entry.key.clone(),
            endpoint_override: entry.endpoint_override.clone(),
        });
    }
    Ok(cores)
}

/// A stand-in key: tests the path end to end without any real key leaving this machine.
fn dummy_core(spec: &str) -> anyhow::Result<CoreKey> {
    let (uid, name) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("--dummy wants <uid>:<name>, got {spec:?}"))?;
    let uid: u64 = uid.parse().with_context(|| format!("uid in {spec:?}"))?;
    Ok(CoreKey {
        uid,
        name: name.to_owned(),
        transport: None,
        key: Secret::new(format!("dummy-not-a-key-{uid}")),
        endpoint_override: String::new(),
    })
}

/// Real keys leave this machine only on a typed "yes".
fn confirm_keys(target: &Target, cores: &[CoreKey]) -> anyhow::Result<()> {
    println!("These cores' FULL-ACCESS keys go to {}:", target.addr());
    for core in cores {
        println!("  {} (uid {})", core.name, core.uid);
    }
    print!("Type yes to send them: ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    anyhow::ensure!(answer.trim() == "yes", "not sent");
    Ok(())
}

fn read_key(path: &str) -> anyhow::Result<russh::keys::PrivateKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    match keys::parse(&text, None) {
        Err(keys::KeyError::NeedsPassphrase) => {
            let phrase = secret(
                "MOON_REMOTE_KEY_PASSPHRASE",
                &format!("passphrase of {path}: "),
            )?;
            Ok(keys::parse(&text, Some(&phrase))?)
        }
        other => Ok(other?),
    }
}

/// A secret from the environment, or asked without echo.
fn secret(env: &str, prompt: &str) -> anyhow::Result<Zeroizing<String>> {
    if let Ok(value) = std::env::var(env) {
        return Ok(Zeroizing::new(value));
    }
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

/// Hand-rolled flags: a handful of `--name value` pairs.
struct Args(Vec<String>);

impl Args {
    fn value(&mut self, name: &str) -> anyhow::Result<Option<String>> {
        let Some(at) = self.0.iter().position(|a| a == name) else {
            return Ok(None);
        };
        anyhow::ensure!(at + 1 < self.0.len(), "{name} needs a value");
        let value = self.0.remove(at + 1);
        self.0.remove(at);
        Ok(Some(value))
    }

    fn values(&mut self, name: &str) -> anyhow::Result<Vec<String>> {
        let mut all = Vec::new();
        while let Some(value) = self.value(name)? {
            all.push(value);
        }
        Ok(all)
    }

    fn flag(&mut self, name: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|a| a != name);
        self.0.len() != before
    }

    fn done(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.0.is_empty(), "unexpected arguments: {:?}", self.0);
        Ok(())
    }
}
