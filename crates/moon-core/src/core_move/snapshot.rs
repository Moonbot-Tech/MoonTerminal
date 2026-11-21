//! A core's settings snapshot taken straight from the running core, over MoonProto.
//!
//! It is the same ZIP MoonBot's own VPS wizard saves ("Get settings"), so [`super::archive`]
//! takes either. A Windows core of a recent build serves it to any authorised client while it
//! keeps trading — the library fetches it in parts and checks its SHA-256; an older core ignores
//! the request. The snapshot carries the bot's exchange API keys and its MoonProto key, so it is
//! read into memory and the file the library writes is removed on every path: a failed or
//! abandoned transfer would otherwise leave a partial copy behind.

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
/// The pointer every failure ends with: the snapshot MoonBot saves by hand works on any build.
const USE_ZIP: &str = "save the snapshot in Moonbot (VPS setup -> Get settings) and pass the ZIP";
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
pub fn download(cfg: ClientConfig, timeout: Duration) -> anyhow::Result<CoreSnapshot> {
    let temp = std::env::temp_dir();
    remove_stale(&temp);
    let client = super::strategies::connect(cfg, timeout)?;
    let exchange = reported_exchange(&client);
    // One name per process, so two moves on one machine never share a file. The library creates
    // the file itself and refuses one that exists.
    let file = TempFile(temp.join(format!("{TEMP_PREFIX}{}.zip", std::process::id())));
    let _ = std::fs::remove_file(&file.0);
    let result = fetch(&client, &file.0);
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
/// writes; [`Progress`] decides when to give up.
fn fetch(client: &MoonClient, path: &Path) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    client
        .settings()
        .download_settings_snapshot(path.to_path_buf())
        .map_err(|e| anyhow::anyhow!("ask for the settings snapshot: {e}; or {USE_ZIP}"))?;
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
            let size = result.map_err(|e| {
                anyhow::anyhow!("the core's settings snapshot failed: {e}; or {USE_ZIP}")
            })?;
            let bytes = Zeroizing::new(
                std::fs::read(path)
                    .map_err(|e| anyhow::anyhow!("read the snapshot: {e}; or {USE_ZIP}"))?,
            );
            anyhow::ensure!(
                bytes.len() as u64 == size,
                "the snapshot file holds {} bytes, the core sent {size}; or {USE_ZIP}",
                bytes.len()
            );
            return Ok(bytes);
        }
        let received = std::fs::metadata(path).map_or(0, |m| m.len());
        progress.check(received, Instant::now())?;
        std::thread::sleep(POLL);
    }
}

/// When a transfer still running is given up, judged by the bytes on disk.
struct Progress {
    /// When the snapshot was asked for.
    start: Instant,
    /// The most bytes seen on disk so far.
    received: u64,
    /// When `received` last grew (`start` until the first part).
    moved: Instant,
}

impl Progress {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            received: 0,
            moved: now,
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
        anyhow::ensure!(
            self.received > 0 || now.duration_since(self.start) < FIRST_PART,
            "the core sent no part of its settings snapshot within {}s: most likely its build \
             does not serve one; {USE_ZIP}",
            FIRST_PART.as_secs()
        );
        anyhow::ensure!(
            now.duration_since(self.moved) < NO_PROGRESS,
            "the core sent no new part of its settings snapshot for {}s ({} bytes received); \
             {USE_ZIP}",
            NO_PROGRESS.as_secs(),
            self.received
        );
        Ok(())
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
