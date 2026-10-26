//! The settings archive a moved core starts from.
//!
//! The input is the settings snapshot MoonBot's own VPS wizard saves ("Get settings"): the bot
//! configuration, its INI and strategy files at their paths inside the bot folder. This module
//! checks it, adds the report database rebuilt from the replica under the name the core opens,
//! and sets one INI switch for the first start.
//!
//! That switch is `KeepOrdersAfterCrash`. As the MoonBot Linux package's README warns, a copy of
//! a running bot's configuration carries an unfinished session, and the bot's usual handling of
//! one may cancel open buys — on the exchange account the old bot is still trading on. The switch
//! keeps them. It is meant for that one start only: the move's finish step removes it once the
//! core is confirmed working, but only when this archive added it
//! ([`MoveArchive::ini_flag_added`]).

use std::io::{Cursor, Read, Write};
use std::path::Path;

use anyhow::Context;
use zip::write::SimpleFileOptions;

/// The bot configuration every snapshot must carry, at the archive root.
pub const BOT_CONFIG: &str = "BotConfig.bcfg";
/// The bot's INI, at the archive root.
pub const BOT_INI: &str = "MoonBot.ini";
/// The INI section of the first-start switch.
pub const KEEP_ORDERS_SECTION: &str = "Special";
/// The INI key of the first-start switch.
pub const KEEP_ORDERS_KEY: &str = "KeepOrdersAfterCrash";
/// Suffix of the strategy files in `data/`: `<Exchange>-<Base>-strat.txt`.
const STRAT_SUFFIX: &str = "-strat.txt";
/// Files of the MoonBot Linux package itself: settings may never replace them.
const PACKAGE_FILES: [&str; 9] = [
    "MoonBot",
    "MoonTelegramService",
    "libmoonbot_sqlite.so",
    "moonbot-update.py",
    "moonbot@.service",
    "README.txt",
    "SHA256SUMS",
    "manifest.json",
    "lib",
];
/// Upper bound on one entry, against a snapshot that inflates without end.
const MAX_ENTRY: u64 = 2 << 30;

/// A checked archive, ready to upload.
pub struct MoveArchive {
    /// The ZIP bytes.
    pub bytes: Vec<u8>,
    /// The exchange name the core files its data under (`data/<exchange>.db`).
    pub exchange: String,
    /// Whether the first-start switch was added here (and so is to be removed after the start).
    pub ini_flag_added: bool,
    /// The report database entry, when the archive carries one.
    pub report_db: Option<String>,
    /// Whether the snapshot carries a strategy file; without one the new core starts with a
    /// default set of its own (`core_move::strategies` carries the real one over).
    pub has_strategies: bool,
}

/// Build the archive from a MoonBot settings snapshot.
///
/// `exchange` overrides the name read from the strategy files, needed when a snapshot holds
/// several; `report_db` is a rebuilt database file to put in as `data/<exchange>.db`.
pub fn compose(
    snapshot: &[u8],
    exchange: Option<&str>,
    report_db: Option<&Path>,
) -> anyhow::Result<MoveArchive> {
    let mut zip = zip::ZipArchive::new(Cursor::new(snapshot)).context("read the settings ZIP")?;
    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        if entry.is_dir() {
            continue;
        }
        names.push(checked_name(&entry.name()?)?);
    }
    anyhow::ensure!(
        names.iter().any(|n| n == BOT_CONFIG),
        "the ZIP has no {BOT_CONFIG}: it is not a MoonBot settings snapshot"
    );
    let exchange = match exchange {
        Some(name) => name.trim().to_owned(),
        None => exchange_from(&names)?,
    };
    anyhow::ensure!(
        !exchange.is_empty() && !exchange.contains(['/', '\\']),
        "invalid exchange name {exchange:?}"
    );
    let db_name = format!("data/{exchange}.db");
    let carries_db = names.iter().any(|n| n.eq_ignore_ascii_case(&db_name));
    let has_strategies = names
        .iter()
        .any(|n| n.starts_with("data/") && n.ends_with(STRAT_SUFFIX));
    anyhow::ensure!(
        !(carries_db && report_db.is_some()),
        "the ZIP already carries {db_name}: drop it or do not add a rebuilt one"
    );

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut ini: Option<Vec<u8>> = None;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = checked_name(&entry.name()?)?;
        anyhow::ensure!(entry.size() <= MAX_ENTRY, "{name} is too large");
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data)?;
        if name == BOT_INI {
            ini = Some(data);
            continue;
        }
        out.start_file(name.as_str(), options())?;
        out.write_all(&data)?;
    }
    let (ini, ini_flag_added) = with_keep_orders(ini.as_deref());
    out.start_file(BOT_INI, options())?;
    out.write_all(&ini)?;
    if let Some(path) = report_db {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        out.start_file(db_name.as_str(), options())?;
        out.write_all(&data)?;
    }
    let bytes = out.finish()?.into_inner();
    Ok(MoveArchive {
        bytes,
        exchange,
        ini_flag_added,
        report_db: (carries_db || report_db.is_some()).then_some(db_name),
        has_strategies,
    })
}

/// Deflate every entry.
fn options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

/// An entry name as a relative `/`-separated path that cannot leave the instance folder or
/// replace a file of the package.
fn checked_name(raw: &str) -> anyhow::Result<String> {
    let name = raw.replace('\\', "/");
    anyhow::ensure!(
        !name.is_empty()
            && !name.starts_with('/')
            && !name.contains(':')
            && !name.split('/').any(|part| part == ".." || part == "."),
        "unsafe path in the settings ZIP: {raw:?}"
    );
    let top = name.split('/').next().unwrap_or_default();
    anyhow::ensure!(
        !PACKAGE_FILES.contains(&top),
        "the settings ZIP would replace the program file {top:?}"
    );
    Ok(name)
}

/// The exchange name from the snapshot's strategy files; an error when there is none or more
/// than one, since the database name must match the core's exchange exactly.
fn exchange_from(names: &[String]) -> anyhow::Result<String> {
    let mut found: Vec<String> = names
        .iter()
        .filter_map(|n| n.strip_prefix("data/")?.strip_suffix(STRAT_SUFFIX))
        .filter(|stem| !stem.contains('/'))
        .filter_map(|stem| {
            stem.rsplit_once('-')
                .map(|(exchange, _base)| exchange.to_owned())
        })
        .collect();
    found.sort();
    found.dedup();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => anyhow::bail!("no strategy file in the ZIP names the exchange: pass it explicitly"),
        many => anyhow::bail!(
            "the ZIP holds strategies of several exchanges ({}): pass the core's exchange explicitly",
            many.join(", ")
        ),
    }
}

/// `ini` with `KeepOrdersAfterCrash=1` in `[Special]`, and whether that line was added here.
///
/// The INI is edited as bytes, so its encoding and line endings survive. A key already present is
/// left exactly as the user set it: the switch is theirs then, and nothing removes it later.
fn with_keep_orders(ini: Option<&[u8]>) -> (Vec<u8>, bool) {
    let text = ini.unwrap_or_default();
    let eol: &[u8] = if text.windows(2).any(|w| w == b"\r\n") || text.is_empty() {
        b"\r\n"
    } else {
        b"\n"
    };
    let lines: Vec<&[u8]> = text.split(|b| *b == b'\n').collect();
    let trim = |l: &[u8]| -> String {
        String::from_utf8_lossy(l)
            .trim_matches(['\r', ' ', '\t'])
            .to_owned()
    };
    let header = format!("[{KEEP_ORDERS_SECTION}]");
    let mut in_section = false;
    let mut section_at = None;
    for (i, line) in lines.iter().enumerate() {
        let t = trim(line);
        if t.starts_with('[') {
            in_section = t.eq_ignore_ascii_case(&header);
            if in_section {
                section_at = Some(i);
            }
            continue;
        }
        if in_section {
            let key = t.split('=').next().unwrap_or_default().trim();
            if key.eq_ignore_ascii_case(KEEP_ORDERS_KEY) {
                return (text.to_vec(), false);
            }
        }
    }
    let line = [format!("{KEEP_ORDERS_KEY}=1").as_bytes(), eol].concat();
    let mut out = Vec::with_capacity(text.len() + line.len() + header.len() + 4);
    match section_at {
        Some(at) => {
            let insert_after = text
                .iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\n')
                .map(|(i, _)| i + 1)
                .nth(at)
                .unwrap_or(text.len());
            out.extend_from_slice(&text[..insert_after]);
            if insert_after == text.len() && !text.ends_with(b"\n") {
                out.extend_from_slice(eol);
            }
            out.extend_from_slice(&line);
            out.extend_from_slice(&text[insert_after..]);
        }
        None => {
            out.extend_from_slice(text);
            if !text.is_empty() && !text.ends_with(b"\n") {
                out.extend_from_slice(eol);
            }
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(eol);
            out.extend_from_slice(&line);
        }
    }
    (out, true)
}

#[cfg(test)]
mod tests;
