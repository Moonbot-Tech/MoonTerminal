//! Which cores the station connects to, and the bot it runs.
//!
//! `station.toml` lists them, without their keys (`STATION.md` §5.1). Each active core's key is
//! the systemd credential `core-<uid>`: the unit loads it with `LoadCredentialEncrypted=`, and
//! the station reads it from `$CREDENTIALS_DIRECTORY` — a per-service ramfs, so the key is never a
//! file on the server's disk in the clear. `moon-remote cores` writes both halves.
//!
//! ```toml
//! [[core]]
//! uid = 3
//! name = "BinF1"
//! transport = "v1"   # optional; the key's own mode when absent
//! ```
//!
//! `[tape]` carries the terminal's `[trade_replay]` window, so a trade's tape is recorded as far
//! around it as the terminal asks for:
//!
//! ```toml
//! [tape]
//! margin_s = 180
//! long_position_min = 10
//! ```
//!
//! `[telegram]` runs the bot, its token the systemd credential `telegram-token` (written by
//! `moon-remote telegram`). `mini_app` also opens the Mini App — and with it switches the station
//! to its account profile (`feed::station::Profile::Account`): orders, balances, strategies and
//! the core's health reach it, as the Mini App's tabs need. Without `[telegram]`, or with the Mini
//! App off, the station stays light. The zone and the language are the bot's; the chats paired
//! with it are the station's own state (`telegram.json` in the data root), not this file's.
//!
//! ```toml
//! [telegram]
//! mini_app = true
//! zone = "Europe/Moscow"   # the reports' time zone; UTC when absent
//! language = "ru"          # ru | en | es; en when absent
//! ```
//!
//! A `key =` line is refused rather than ignored: a key in a plain file is exactly what this
//! layout exists to prevent.
//!
//! Without the file, a build with the `terminal-config` feature reads the terminal's own
//! configuration from the same data root (a copy of its `cfg/`), which is how the probe runs on
//! the developer's machine. A COPY: loading may assign uids to entries that lack one and save
//! them, without the terminal's durable-store uid floor.

use std::path::Path;

use anyhow::Context;
use chrono_tz::Tz;
use moon_core::config::{AppConfig, FeedFlags, Language, Secret, ServerConfig, TransportVersion};
use moon_core::feed::station::Profile;
use serde::Deserialize;
use zeroize::Zeroizing;

/// The credential holding the bot token.
const TOKEN_CREDENTIAL: &str = "telegram-token";

/// `station.toml`.
#[derive(Deserialize)]
struct StationFile {
    #[serde(default, rename = "core")]
    cores: Vec<CoreEntry>,
    #[serde(default)]
    tape: Tape,
    telegram: Option<TelegramSection>,
}

/// `[tape]`: the terminal's window around a trade. Absent fields keep the station's own
/// `storage.toml` values.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tape {
    /// Seconds of prints on each side of a trade (`[trade_replay] margin_s`).
    pub margin_s: Option<u32>,
    /// From how many minutes a position is recorded as its two ends
    /// (`[trade_replay] long_position_min`).
    pub long_position_min: Option<u32>,
}

/// `[telegram]` as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TelegramSection {
    #[serde(default)]
    mini_app: bool,
    zone: Option<String>,
    language: Option<String>,
}

/// The bot the station runs.
pub struct Telegram {
    /// `None` when the credential is missing or empty: the bot stays off, the rest of the station
    /// runs — the cores do not wait on the bot.
    pub token: Option<Secret>,
    /// The Mini App is open, and the station runs the account it shows.
    pub mini_app: bool,
    /// The zone every report and Mini App time is shown in.
    pub zone: Tz,
    pub language: Language,
}

impl Telegram {
    /// Whether this configuration equals `other`, the token included: any difference takes a
    /// restart, which a reload cannot give.
    pub fn same_as(&self, other: &Self) -> bool {
        self.mini_app == other.mini_app
            && self.zone == other.zone
            && self.language == other.language
            && self.token.as_ref().map(Secret::expose) == other.token.as_ref().map(Secret::expose)
    }
}

/// What the station runs with: the cores, the tape window and the bot.
pub struct Station {
    pub config: AppConfig,
    pub tape: Tape,
    pub telegram: Option<Telegram>,
}

impl Station {
    /// The profile this configuration asks for: the account one with the Mini App open — and a
    /// bot to open it, since without its token nobody reads that account.
    pub fn profile(&self) -> Profile {
        match &self.telegram {
            Some(telegram) if telegram.mini_app && telegram.token.is_some() => Profile::Account,
            _ => Profile::Reports,
        }
    }
}

/// One `[[core]]`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CoreEntry {
    /// The terminal's uid for this core, so report rows and traces carry the same key in both.
    /// Also names its credential, `core-<uid>`.
    uid: u64,
    name: String,
    /// Skip a core without deleting its entry.
    #[serde(default = "default_true")]
    active: bool,
    /// A transport mode the terminal overrides the key's with.
    #[serde(default)]
    transport: Option<TransportVersion>,
}

/// What the light station reads from a core: its reports alone (`STATION.md` §3.2). Above all no
/// `log`: with it the feed writes every core's log to the data root, and the station keeps no
/// logs. The clock offset still samples `ServerLog` — that pass ignores this flag — and stores
/// nothing but the offset.
const STATION_FEED: FeedFlags = FeedFlags {
    orders: false,
    detects: false,
    reports: true,
    balance: false,
    strategies: false,
    log: false,
    alerts: false,
    arb: false,
};

/// What the Mini App's station reads besides: the account its tabs show — orders, balances,
/// strategies. Still no log, detects, chart alerts or `arb`.
const ACCOUNT_FEED: FeedFlags = FeedFlags {
    orders: true,
    balance: true,
    strategies: true,
    ..STATION_FEED
};

/// Give every core the feed of `profile`.
pub fn set_feed(config: &mut AppConfig, profile: Profile) {
    let feed = match profile {
        Profile::Reports => STATION_FEED,
        Profile::Account => ACCOUNT_FEED,
    };
    for server in &mut config.servers {
        server.feed = feed;
    }
}

fn default_true() -> bool {
    true
}

/// The station's configuration: every core of `station.toml` at `path`, or of the terminal's own
/// files when there is none.
pub fn load(path: &Path) -> anyhow::Result<Station> {
    if path.exists() {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let creds = std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from);
        return from_station_file(&text, creds.as_deref())
            .with_context(|| format!("parse {}", path.display()));
    }
    terminal_config(path)
}

/// `creds` is the credentials directory; `None` when the process was not given one.
fn from_station_file(text: &str, creds: Option<&Path>) -> anyhow::Result<Station> {
    let file: StationFile = toml::from_str(text)?;
    anyhow::ensure!(!file.cores.is_empty(), "no [[core]] entries");
    // A uid keys a core's reports, traces and tape: two entries sharing one would merge two
    // cores' histories, and 0 means "unassigned" everywhere in the terminal.
    let mut seen = std::collections::HashSet::new();
    for entry in &file.cores {
        anyhow::ensure!(entry.uid != 0, "core {:?}: uid 0 is not a uid", entry.name);
        anyhow::ensure!(
            seen.insert(entry.uid),
            "core {:?}: uid {} is used twice",
            entry.name,
            entry.uid
        );
    }
    let servers = file
        .cores
        .into_iter()
        .map(|entry| {
            // Every other field keeps the terminal's own default for a new server.
            // `id` alone has no serde default; it is set from the uid below.
            let mut server: ServerConfig = toml::from_str("id = 0")?;
            server.uid = entry.uid;
            if entry.active {
                server.key =
                    core_key(creds, entry.uid).with_context(|| format!("core {:?}", entry.name))?;
            }
            server.name = entry.name;
            server.active = entry.active;
            server.transport = entry.transport;
            Ok(server)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let telegram = file
        .telegram
        .map(|section| telegram(section, creds))
        .transpose()
        .context("[telegram]")?;
    let mut station = Station {
        config: AppConfig::headless(servers),
        tape: file.tape,
        telegram,
    };
    let profile = station.profile();
    set_feed(&mut station.config, profile);
    Ok(station)
}

/// `[telegram]` with its token.
fn telegram(section: TelegramSection, creds: Option<&Path>) -> anyhow::Result<Telegram> {
    let zone = match section.zone {
        Some(name) => name
            .parse::<Tz>()
            .map_err(|_| anyhow::anyhow!("zone {name:?} is not an IANA time zone"))?,
        None => Tz::UTC,
    };
    let language = match section.language {
        Some(code) => Language::from_code(&code)
            .ok_or_else(|| anyhow::anyhow!("language {code:?}: ru, en or es"))?,
        None => Language::En,
    };
    let token = credential(creds, TOKEN_CREDENTIAL)
        .inspect_err(|e| log::error!("[telegram]: {e:#}; the bot stays off"))
        .ok();
    Ok(Telegram {
        token,
        mini_app: section.mini_app,
        zone,
        language,
    })
}

/// The credential `core-<uid>`, as systemd placed it.
fn core_key(creds: Option<&Path>, uid: u64) -> anyhow::Result<Secret> {
    credential(creds, &format!("core-{uid}"))
}

/// The credential `name`, as systemd placed it.
fn credential(creds: Option<&Path>, name: &str) -> anyhow::Result<Secret> {
    let dir = creds.ok_or_else(|| {
        anyhow::anyhow!(
            "no $CREDENTIALS_DIRECTORY: run under systemd with LoadCredentialEncrypted={name}:…"
        )
    })?;
    let text = Zeroizing::new(
        std::fs::read_to_string(dir.join(name))
            .with_context(|| format!("read credential {name}"))?,
    );
    let value = text.trim();
    anyhow::ensure!(!value.is_empty(), "credential {name} is empty");
    Ok(Secret::new(value))
}

#[cfg(feature = "terminal-config")]
fn terminal_config(_missing: &Path) -> anyhow::Result<Station> {
    log::info!("no station.toml: reading the terminal's configuration from the data root");
    Ok(Station {
        config: AppConfig::load(None, false)?,
        tape: Tape::default(),
        telegram: None,
    })
}

#[cfg(not(feature = "terminal-config"))]
fn terminal_config(missing: &Path) -> anyhow::Result<Station> {
    anyhow::bail!(
        "{} not found: it lists the cores to connect to",
        missing.display()
    )
}

#[cfg(test)]
mod tests;
