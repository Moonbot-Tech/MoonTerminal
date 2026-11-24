//! A core's settings snapshot taken straight from the running core, over MoonProto.
//!
//! It is the same ZIP MoonBot's own VPS wizard saves ("Get settings"), so [`super::archive`]
//! takes either. A Windows core of a recent build serves it to any authorised client while it
//! keeps trading — the library fetches it in parts and checks its SHA-256; an older core ignores
//! the request. The snapshot carries the bot's exchange API keys and its MoonProto key, so it is
//! read into memory and the file the library writes is removed on every path: a failed or
//! abandoned transfer would otherwise leave a partial copy behind.
//!
//! A transfer that fails is marked as one ([`is_transfer_failure`]): a snapshot saved by hand in
//! MoonBot works around it on any build, but only a caller that can take such a ZIP may say so —
//! the terminal's window cannot, the command line can.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use moonproto::state::SettingsEvent;
use moonproto::{ClientConfig, Event, MoonClient};
use zeroize::Zeroizing;

/// How often the wait below looks at the client's events.
const POLL: Duration = Duration::from_millis(200);
/// How long the first part may take to arrive. A core that does not serve snapshots stays
/// silent, and the library itself gives up only after five minutes without progress; a core that
/// does serve one starts sending within seconds, so this leaves room for a slow first part.
const FIRST_PART: Duration = Duration::from_secs(60);
/// How long the transfer may go without a new part once it has started: the library's own limit
/// (it ends a download after five minutes without progress). A fixed clock over the whole
/// transfer would cut a slow link that is still receiving parts.
const NO_PROGRESS: Duration = Duration::from_secs(300);
/// The least time between two reports of the bytes received: often enough for a count that
/// moves, rare enough that a fast link does not wake whoever draws it on every poll.
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// Name prefix of the library's download target in the temp folder; the process id follows.
const TEMP_PREFIX: &str = "moon-core-move-settings-";
/// Age past which a target is a leftover of a killed run: a live transfer gets a new part at
/// least every [`NO_PROGRESS`], and a settings archive of a few MB arrives in minutes.
const STALE: Duration = Duration::from_secs(3600);

/// A settings snapshot as the core served it.
pub struct CoreSnapshot {
    /// The ZIP bytes.
    pub bytes: Zeroizing<Vec<u8>>,
    /// The exchange the core reports itself running on, the name it files its data under —
    /// `None` when it reports none. A snapshot carries the strategy files of every exchange the
    /// bot has ever run on, so the files alone cannot tell which one this core is.
    pub exchange: Option<String>,
}

/// Download the settings snapshot of the core at `cfg`. `timeout` bounds the connect; the
/// transfer runs for as long as parts keep arriving (see [`Progress`]).
///
/// `received` hears the bytes that have arrived so far, each time they grew but at most once per
/// [`REPORT_EVERY`]: the library tells nothing until the end, and a large snapshot over a slow
/// link takes minutes the user would otherwise watch without a sign of life. The total is not
/// known before the end — the library keeps it to itself.
///
/// Errors:
///     A failure of the transfer itself is marked for [`is_transfer_failure`]; a connect that
///     fails is not — the core is not reached, and a hand-saved ZIP would not reach it either.
pub fn download(
    cfg: ClientConfig,
    timeout: Duration,
    received: &mut dyn FnMut(u64),
) -> anyhow::Result<CoreSnapshot> {
    let temp = std::env::temp_dir();
    remove_stale(&temp);
    let client = super::strategies::connect(cfg, timeout)?;
    let exchange = reported_exchange(&client);
    // One name per process, so two moves on one machine never share a file. The library creates
    // the file itself and refuses one that exists.
    let file = TempFile(temp.join(format!("{TEMP_PREFIX}{}.zip", std::process::id())));
    let _ = std::fs::remove_file(&file.0);
    let result = fetch(&client, &file.0, received);
    // The library holds the file open until its runtime thread ends; a removal before that only
    // marks it, so `file` is dropped after the thread has exited and the copy is gone on return.
    let _ = client.disconnect();
    let _ = client.wait_finished();
    drop(file);
    Ok(CoreSnapshot {
        bytes: result?,
        exchange,
    })
}

/// The exchange the core at `cfg` reports itself running on, for a move whose snapshot came
/// from elsewhere (a ZIP); `None` when it reports none.
pub fn exchange_of(cfg: ClientConfig, timeout: Duration) -> anyhow::Result<Option<String>> {
    let client = super::strategies::connect(cfg, timeout)?;
    let exchange = reported_exchange(&client);
    let _ = client.disconnect();
    Ok(exchange)
}

/// Whether `error` is a failed transfer of the snapshot — one a snapshot saved by hand in MoonBot
/// ("VPS setup -> Get settings") works around on any build. Seen through any context a caller
/// added; a caller that can take such a ZIP says so, one that cannot leaves it unsaid.
pub fn is_transfer_failure(error: &anyhow::Error) -> bool {
    error.downcast_ref::<TransferFailed>().is_some()
}

/// A failed transfer, as [`is_transfer_failure`] tells it apart. The text is the reason alone:
/// what the user can do about it is the caller's to say. Public so a caller's own tests can
/// stand one up without a core.
#[derive(Debug)]
pub struct TransferFailed(pub String);

impl std::fmt::Display for TransferFailed {
    /// The reason, as it was given.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransferFailed {}

/// A failed transfer for `reason`, marked as one.
fn failed(reason: String) -> anyhow::Error {
    anyhow::Error::new(TransferFailed(reason))
}

/// The exchange name a connected core reports (`ServerInfo::exchange_name`), trimmed; `None`
/// when empty. It is the name the core files its data under, the strategy files' stem.
fn reported_exchange(client: &MoonClient) -> Option<String> {
    client
        .snapshot()
        .and_then(|state| state.server_info().exchange_name.clone())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// Remove targets a killed run (Ctrl+C, a crash) left behind: its cleanup never ran, and the
/// file holds the bot's exchange API keys.
fn remove_stale(temp: &Path) {
    let Ok(entries) = std::fs::read_dir(temp) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let stale = name.to_str().is_some_and(|n| n.starts_with(TEMP_PREFIX))
            && entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > STALE));
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Ask for the snapshot into `path` and wait for the library's verdict on it.
///
/// The library reports nothing until the transfer ends, so progress is read off the file it
/// writes; [`Progress`] decides when to give up and when `received` hears of it.
fn fetch(
    client: &MoonClient,
    path: &Path,
    received: &mut dyn FnMut(u64),
) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    client
        .settings()
        .download_settings_snapshot(path.to_path_buf())
        .map_err(|e| failed(format!("ask for the settings snapshot: {e}")))?;
    let mut progress = Progress::new(Instant::now());
    let mut events = Vec::new();
    loop {
        client.drain_events_into(&mut events);
        for event in events.drain(..) {
            let Event::Settings(SettingsEvent::SettingsSnapshotDownloaded {
                path: done,
                result,
                ..
            }) = event
            else {
                continue;
            };
            if done != path {
                continue;
            }
            let size =
                result.map_err(|e| failed(format!("the core's settings snapshot failed: {e}")))?;
            let bytes = Zeroizing::new(
                std::fs::read(path).map_err(|e| failed(format!("read the snapshot: {e}")))?,
            );
            if bytes.len() as u64 != size {
                return Err(failed(format!(
                    "the snapshot file holds {} bytes, the core sent {size}",
                    bytes.len()
                )));
            }
            return Ok(bytes);
        }
        let now = Instant::now();
        progress.check(std::fs::metadata(path).map_or(0, |m| m.len()), now)?;
        if let Some(bytes) = progress.report(now) {
            received(bytes);
        }
        std::thread::sleep(POLL);
    }
}

/// When a transfer still running is given up, and when its bytes are reported, judged by the
/// bytes on disk.
struct Progress {
    /// When the snapshot was asked for.
    start: Instant,
    /// The most bytes seen on disk so far.
    received: u64,
    /// When `received` last grew (`start` until the first part).
    moved: Instant,
    /// The bytes last reported and when; `None` before the first report.
    reported: Option<(u64, Instant)>,
}

impl Progress {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            received: 0,
            moved: now,
            reported: None,
        }
    }

    /// Record `received` bytes on disk at `now`; an error when the transfer is to be given up.
    ///
    /// Still empty after [`FIRST_PART`] means no part arrived — an old build, as a rule, though
    /// a core that could not start the export in that time reads the same. Once parts arrive,
    /// only [`NO_PROGRESS`] without a new one ends the wait.
    fn check(&mut self, received: u64, now: Instant) -> anyhow::Result<()> {
        if received > self.received {
            self.received = received;
            self.moved = now;
        }
        if self.received == 0 && now.duration_since(self.start) >= FIRST_PART {
            return Err(failed(format!(
                "the core sent no part of its settings snapshot within {}s: most likely its \
                 build does not serve one",
                FIRST_PART.as_secs()
            )));
        }
        if now.duration_since(self.moved) >= NO_PROGRESS {
            return Err(failed(format!(
                "the core sent no new part of its settings snapshot for {}s ({} bytes received)",
                NO_PROGRESS.as_secs(),
                self.received
            )));
        }
        Ok(())
    }

    /// The bytes received, when they are to be reported at `now`: they grew since the last
    /// report, and that report is at least [`REPORT_EVERY`] old. The first part is reported at
    /// once, so the count appears as soon as there is one.
    fn report(&mut self, now: Instant) -> Option<u64> {
        let due = match self.reported {
            None => self.received > 0,
            Some((bytes, at)) => self.received > bytes && now.duration_since(at) >= REPORT_EVERY,
        };
        due.then(|| {
            self.reported = Some((self.received, now));
            self.received
        })
    }
}

/// The library's download target, removed when dropped.
struct TempFile(PathBuf);

impl Drop for TempFile {
    /// The file holds the bot's exchange API keys: it never outlives the download.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests;
