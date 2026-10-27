//! What a move takes from this machine and the old core, gathered before the server is touched,
//! and the steps that talk to the two cores around the new one's start.
//!
//! [`Plan::build`] reads the core's key, takes the settings snapshot (from a ZIP or straight from
//! the running core), settles the exchange, rebuilds the report database from the replica,
//! composes the archive and reads the old core's strategy set. Nothing has left the machine then:
//! the caller confirms the secrets and installs. [`Plan::stop_old`] asks the old core to stop and
//! reports whether it did; [`Plan::carry_strategies`] writes the strategy set to the new core once
//! it runs.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use moon_core::config::TransportVersion;
use moon_core::core_move::archive::{self, MoveArchive};
use moon_core::core_move::key_packet::ConnectionPacket;
use moon_core::core_move::old_core::{self, StopAnswer};
use moon_core::core_move::report_db;
use moon_core::core_move::snapshot;
use moon_core::core_move::strategies::{self, StrategySet};

use super::{MoveStep, MoveWarning};

/// How long a call to a core waits for it: connect, log in to the exchange, answer.
const CORE_TIMEOUT: Duration = Duration::from_secs(180);

/// Which MoonProto key the moved core runs on.
pub enum KeySource {
    /// A core of this terminal, by uid or name: it keeps its key, its report history and its
    /// strategies.
    Core(String),
    /// A connection packet file: the key of a core this terminal does not hold.
    PacketFile(PathBuf),
    /// A new key: every terminal of the core will need the new packet.
    NewKey,
}

/// What a move is asked to do.
pub struct Request {
    pub key: KeySource,
    /// A settings snapshot saved in MoonBot; `None` takes it from the running core (needs
    /// [`KeySource::Core`]).
    pub zip: Option<PathBuf>,
    /// The core's exchange as the user names it; otherwise the core's own report, or the
    /// strategy files.
    pub exchange: Option<String>,
    /// Where the old core answers, `host:port`, when not at its packet's address.
    pub from: Option<String>,
    /// Rebuild the report database from the replica (with [`KeySource::Core`]).
    pub report_db: bool,
    /// Carry the old core's strategy set over MoonProto (with [`KeySource::Core`]).
    pub strategy_sync: bool,
    /// Carry the strategies switched off.
    pub strategies_off: bool,
    /// The server's public address and the new core's UDP port.
    pub public_ip: Ipv4Addr,
    pub proto_port: u16,
}

/// A move ready to install.
pub struct Plan {
    /// The terminal uid of the moved core, with [`KeySource::Core`].
    pub uid: Option<u64>,
    /// The connection packet at the new address — the one the new core starts from.
    pub packet: ConnectionPacket,
    /// [`Plan::packet`] exported, for the install and for whoever keeps it.
    pub packet_text: String,
    /// The settings archive.
    pub archive: MoveArchive,
    /// The old core's strategy set, to write to the new core.
    pub strategies: Option<StrategySet>,
    /// Whether the strategy carry must succeed: the snapshot has no strategy file of its own, or
    /// the strategies are to arrive switched off.
    pub carry_required: bool,
    /// Whether the strategies arrive switched off, for a set read again later.
    strategies_off: bool,
    /// The old core's key and address, when it is known.
    old: Option<(ConnectionPacket, String, u16)>,
    /// Whether the old core answered at that address during this move. Only then does silence
    /// there mean it stopped, rather than a wrong address.
    old_answered: bool,
}

impl Plan {
    /// Gather everything the move takes; see the module docs. `say` hears the progress.
    pub fn build(request: &Request, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<Plan> {
        let core = matches!(request.key, KeySource::Core(_));
        anyhow::ensure!(
            request.zip.is_some() || core,
            "give a settings snapshot ZIP: only a move of a terminal core can take the snapshot \
             from the running core instead"
        );
        anyhow::ensure!(
            !request.strategies_off || (core && request.strategy_sync),
            "carrying the strategies switched off needs a terminal core and the strategy carry"
        );
        anyhow::ensure!(
            request.from.is_none() || core,
            "the old core's address belongs to a move of a terminal core"
        );
        let (old_packet, uid) = match &request.key {
            KeySource::Core(pick) => {
                let (uid, packet) = core_packet(pick)?;
                (packet, Some(uid))
            }
            KeySource::PacketFile(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("read {}", path.display()))?;
                let packet = ConnectionPacket::parse(&text).ok_or_else(|| {
                    anyhow::anyhow!("{} is not a MoonBot connection packet", path.display())
                })?;
                (packet, None)
            }
            KeySource::NewKey => (
                ConnectionPacket::generate(
                    request.public_ip,
                    request.proto_port,
                    TransportVersion::V0,
                )?,
                None,
            ),
        };
        // A generated key belongs to no running core; any other core is reached at the address
        // given for it, else the one its packet names — and is unknown only when there is neither.
        let old = match &request.key {
            KeySource::NewKey => None,
            _ if request.from.is_none() && old_packet.ipv4.is_none() => None,
            _ => {
                let (host, port) = old_core_endpoint(&old_packet, request.from.as_deref())?;
                Some((old_packet.clone(), host, port))
            }
        };
        let packet = old_packet.with_endpoint(request.public_ip, request.proto_port);
        let mut old_answered = false;

        // The exchange is the user's, else the one the old core reports for itself (a terminal
        // core); without either, `archive::compose` reads it off the strategy files.
        let (settings, reported) = match &request.zip {
            Some(path) => {
                let bytes = zeroize::Zeroizing::new(
                    std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
                );
                // Asked only when the strategy files cannot settle it: a ZIP of one exchange
                // never needs the old core.
                let ambiguous = archive::strategy_exchanges(&bytes)?.len() > 1;
                let reported = if request.exchange.is_none() && core && ambiguous {
                    let ask = || -> anyhow::Result<Option<String>> {
                        let (packet, host, port) = old.as_ref().ok_or_else(|| {
                            anyhow::anyhow!("the core's key names no address: give its address")
                        })?;
                        snapshot::exchange_of(config(packet, host, *port)?, CORE_TIMEOUT)
                    };
                    match ask() {
                        Ok(name) => {
                            old_answered = true;
                            name
                        }
                        Err(e) => {
                            say(MoveStep::Warning(
                                MoveWarning::ExchangeNotAsked,
                                format!("{e:#}"),
                            ));
                            None
                        }
                    }
                } else {
                    None
                };
                (bytes, reported)
            }
            None => {
                let (packet, host, port) = old.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("the core's key names no address: give its address")
                })?;
                let taken = snapshot::download(config(packet, host, *port)?, CORE_TIMEOUT)
                    .with_context(|| format!("take the settings snapshot from {host}:{port}"))?;
                say(MoveStep::SnapshotTaken {
                    bytes: taken.bytes.len(),
                    exchange: taken.exchange.clone(),
                });
                old_answered = true;
                (taken.bytes, taken.exchange)
            }
        };
        // A core-reported name decides where the rebuilt report database goes, so it must be one
        // the snapshot's strategy files carry too: a mismatch would file the database under a name
        // the new core never opens. The user's exchange is taken as given.
        let exchange = match (&request.exchange, reported) {
            (Some(chosen), _) => Some(chosen.clone()),
            (None, Some(reported)) => {
                let known = archive::strategy_exchanges(&settings)?;
                anyhow::ensure!(
                    known.is_empty() || known.contains(&reported),
                    "the old core reports exchange {reported:?}, but the snapshot's strategy files \
                     name {}: name the core's exchange",
                    known.join(", ")
                );
                Some(reported)
            }
            (None, None) => None,
        };
        let db_file = match (uid, request.report_db) {
            (Some(uid), true) => Some(rebuild_db(uid, say)?),
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
        let archive = composed?;
        say(MoveStep::Composed {
            exchange: archive.exchange.clone(),
            report_db: archive.report_db.clone(),
            ini_flag_added: archive.ini_flag_added,
        });
        // The carry may fall back to the snapshot's own strategy file only when there is one and
        // the strategies are not to arrive switched off — that file keeps them as they were.
        let carry_required = !archive.has_strategies || request.strategies_off;
        let strategies = match uid {
            Some(_) if request.strategy_sync => {
                let mut set = read_old_strategies(old.as_ref(), carry_required, say)?;
                old_answered |= set.is_some();
                // Switched off, the new core holds the strategies but trades none until someone
                // checks them — a move that wants the new core verified before it trades.
                if request.strategies_off {
                    for strategy in set.iter_mut().flat_map(|s| s.strategies.iter_mut()) {
                        strategy.checked = false;
                    }
                }
                set
            }
            _ => {
                anyhow::ensure!(
                    archive.has_strategies || !request.strategy_sync,
                    "the snapshot carries no strategy file: the new core would start with a \
                     default set, and terminals would read the real strategies as deleted. Move a \
                     terminal core, add the strategy file to the ZIP, or skip the strategy carry \
                     knowingly"
                );
                None
            }
        };
        Ok(Plan {
            uid,
            packet_text: packet.export()?,
            packet,
            archive,
            strategies,
            carry_required,
            strategies_off: request.strategies_off,
            old,
            old_answered,
        })
    }

    /// Read the old core's strategy set again, just before it is stopped: it kept trading — and
    /// its user editing — while the move was confirmed and installed. A failed read keeps the set
    /// [`Plan::build`] read, with a warning.
    pub fn refresh_strategies(&mut self, say: &mut dyn FnMut(MoveStep)) {
        if self.strategies.is_none() {
            return;
        }
        match read_old_strategies(self.old.as_ref(), true, &mut |_| {}) {
            Ok(Some(mut set)) => {
                if self.strategies_off {
                    for strategy in &mut set.strategies {
                        strategy.checked = false;
                    }
                }
                say(MoveStep::OldStrategies {
                    strategies: set.strategies.len(),
                    folders: set.folders.len(),
                });
                self.strategies = Some(set);
                self.old_answered = true;
            }
            Ok(None) => {}
            Err(e) => say(MoveStep::Warning(
                MoveWarning::StrategiesNotRefreshed,
                format!("{e:#}"),
            )),
        }
    }

    /// Whether the old core is known, and so can be stopped and watched.
    pub fn has_old_core(&self) -> bool {
        self.old.is_some()
    }

    /// Ask the old core to stop and see whether it does: [`StopAnswer::Stopped`] or
    /// [`StopAnswer::Refused`], never `NotReached` — silence counts as stopped only for a core
    /// that answered at this address earlier in the move, and is an error otherwise.
    pub fn stop_old(&mut self, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<StopAnswer> {
        let (packet, host, port) = self
            .old
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the old core's address is not known"))?;
        say(MoveStep::StopRequested);
        let answer = match old_core::request_stop(config(packet, host, *port)?, CORE_TIMEOUT)
            .with_context(|| format!("stop the old core at {host}:{port}"))?
        {
            StopAnswer::NotReached if self.old_answered => StopAnswer::Stopped,
            StopAnswer::NotReached => anyhow::bail!(
                "nothing answers at {host}:{port}, where the old core is expected: if the address \
                 is wrong, the old core may still run elsewhere. Check it; once the old core is \
                 down, start the new one knowingly"
            ),
            answer => answer,
        };
        match answer {
            StopAnswer::Refused { open_deals } => {
                self.old_answered = true;
                say(MoveStep::OldRefused { open_deals });
            }
            _ => say(MoveStep::OldStopped),
        }
        Ok(answer)
    }

    /// Whether the old core answers no connection — stopped by its user after a refusal.
    pub fn old_is_silent(&self) -> anyhow::Result<bool> {
        let (packet, host, port) = self
            .old
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the old core's address is not known"))?;
        Ok(old_core::is_silent(config(packet, host, *port)?))
    }

    /// Write the old core's strategy set to the new core, which must run. Fails only when the
    /// carry is required; otherwise a failure is a warning and the snapshot's file stands in.
    pub fn carry_strategies(&self, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<()> {
        let Some(set) = &self.strategies else {
            return Ok(());
        };
        let host = self
            .packet
            .ipv4
            .map(|ip| ip.to_string())
            .ok_or_else(|| anyhow::anyhow!("the new packet names no address"))?;
        let cfg = config(&self.packet, &host, self.packet.port)?;
        match strategies::write_set(cfg, set, CORE_TIMEOUT) {
            Ok(carried) => {
                say(MoveStep::StrategiesWritten {
                    written: carried.written,
                    deleted: carried.deleted,
                });
                Ok(())
            }
            Err(e) if !self.carry_required => {
                say(MoveStep::Warning(
                    MoveWarning::StrategiesNotConfirmed,
                    format!("{e:#}"),
                ));
                Ok(())
            }
            Err(e) => Err(e.context(
                "the new core's strategies are NOT confirmed as the old core's (they may be \
                 partly written): do not switch terminals to it",
            )),
        }
    }
}

/// A MoonProto client configuration for the core holding `packet`'s key at `host:port`.
fn config(
    packet: &ConnectionPacket,
    host: &str,
    port: u16,
) -> anyhow::Result<moon_core::core_move::strategies::ClientConfig> {
    strategies::client_config(&packet.export()?, host, port, Some(packet.mode.into()))
}

/// Where the old core answers: `from` when given, else the address its packet names.
pub fn old_core_endpoint(
    packet: &ConnectionPacket,
    from: Option<&str>,
) -> anyhow::Result<(String, u16)> {
    match from {
        Some(endpoint) => {
            let (host, port) = endpoint
                .rsplit_once(':')
                .ok_or_else(|| anyhow::anyhow!("the old core's address wants <host:port>"))?;
            Ok((
                host.to_owned(),
                port.parse::<u16>().context("the old core's port")?,
            ))
        }
        None => {
            let ip = packet.ipv4.ok_or_else(|| {
                anyhow::anyhow!("the core's key names no address: give the old core's address")
            })?;
            Ok((ip.to_string(), packet.port))
        }
    }
}

/// The old core's strategy set. When it cannot be read: an error if the carry is `required`,
/// else `None` with a warning — the snapshot's own file then stands in.
fn read_old_strategies(
    old: Option<&(ConnectionPacket, String, u16)>,
    required: bool,
    say: &mut dyn FnMut(MoveStep),
) -> anyhow::Result<Option<StrategySet>> {
    let read = || -> anyhow::Result<StrategySet> {
        let (packet, host, port) =
            old.ok_or_else(|| anyhow::anyhow!("the old core's address is not known"))?;
        strategies::read_set(config(packet, host, *port)?, CORE_TIMEOUT)
            .with_context(|| format!("read the old core's strategies at {host}:{port}"))
    };
    match read() {
        Ok(set) => {
            say(MoveStep::OldStrategies {
                strategies: set.strategies.len(),
                folders: set.folders.len(),
            });
            Ok(Some(set))
        }
        Err(e) if !required => {
            say(MoveStep::Warning(
                MoveWarning::StrategiesNotRead,
                format!("{e:#}"),
            ));
            Ok(None)
        }
        Err(e) => Err(e.context(
            "the old core's strategy set could not be read, and the move needs it (no strategy \
             file in the snapshot, or the strategies are to arrive switched off)",
        )),
    }
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

/// Rebuild core `uid`'s report database from the replica of the data root.
fn rebuild_db(uid: u64, say: &mut dyn FnMut(MoveStep)) -> anyhow::Result<TempDb> {
    let replica = moon_core::config::paths::reports_db_path_no_create();
    anyhow::ensure!(
        replica.exists(),
        "no report replica at {}: move without the report database",
        replica.display()
    );
    let path = std::env::temp_dir().join(format!("moon-core-move-{}-{uid}.db", std::process::id()));
    let file = TempDb { path };
    let summary = report_db::build(&replica, uid, &file.path)?;
    say(MoveStep::ReportDb {
        rows: summary.rows,
        open_rows: summary.open_rows,
        epoch: summary.epoch,
    });
    Ok(file)
}
