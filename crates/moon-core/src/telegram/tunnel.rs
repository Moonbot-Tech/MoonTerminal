//! Exact `cloudflared` child ownership, quick-tunnel URL parsing, and stop/wait.
//!
//! The bot token is never an argument or environment value. No shell is used. Stop and Drop
//! signal reader threads, kill the exact child if it is still running, wait/reap it, join
//! readers, and clear the public URL.

use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Parsed quick-tunnel URL host suffix required by Cloudflare's trycloudflare service.
const TRYCLOUDFLARE_SUFFIX: &str = ".trycloudflare.com";

/// Exact `cloudflared` argv used for a loopback quick tunnel. Constructed without a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelCommand {
    /// Path of the verified `cloudflared` executable.
    pub program: PathBuf,
    /// Arguments beginning with `tunnel --config <isolated> --url http://127.0.0.1:<port>
    /// --http-host-header 127.0.0.1 --no-autoupdate`.
    pub args: Vec<String>,
}

impl TunnelCommand {
    /// Build `cloudflared tunnel --config <isolated> --url http://127.0.0.1:<port>
    /// --http-host-header 127.0.0.1 --no-autoupdate`.
    ///
    /// `--http-host-header` rewrites the forwarded Host to the loopback origin so the Mini App
    /// allow-list can keep rejecting the public trycloudflare hostname. `--config` points at an
    /// app-owned empty YAML so a user-level `~/.cloudflared/config.yml` is not discovered.
    ///
    /// Args:
    ///     binary: Verified `cloudflared` path from [`crate::config::paths`].
    ///     loopback_port: Port of the Mini App listener bound to `127.0.0.1`.
    ///
    /// Returns:
    ///     Command parts ready for [`Command::new`] without a shell.
    pub fn quick_tunnel(binary: &Path, loopback_port: u16) -> Self {
        Self {
            program: binary.to_path_buf(),
            args: vec![
                "tunnel".into(),
                "--config".into(),
                isolated_quick_tunnel_config_path()
                    .to_string_lossy()
                    .into_owned(),
                "--url".into(),
                format!("http://127.0.0.1:{loopback_port}"),
                "--http-host-header".into(),
                "127.0.0.1".into(),
                "--no-autoupdate".into(),
            ],
        }
    }

    /// Convert into a `std::process::Command` with piped stdout/stderr and no extra env secrets.
    pub fn into_std_command(self) -> Command {
        let mut command = Command::new(self.program);
        command
            .args(self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
    }
}

/// Owner of one `cloudflared` child and its stdout/stderr reader threads.
pub struct TunnelProcess {
    child: Option<Child>,
    stop: Arc<AtomicBool>,
    url: Arc<Mutex<Option<String>>>,
    stdout_join: Option<JoinHandle<()>>,
    stderr_join: Option<JoinHandle<()>>,
}

impl TunnelProcess {
    /// Spawn a quick tunnel from a verified binary and loopback port.
    ///
    /// Args:
    ///     binary: Verified `cloudflared` executable.
    ///     loopback_port: Port of the Mini App listener.
    ///
    /// Returns:
    ///     An owner whose [`TunnelProcess::public_url`] fills in after a valid URL is parsed.
    pub fn spawn_quick_tunnel(binary: &Path, loopback_port: u16) -> io::Result<Self> {
        ensure_isolated_quick_tunnel_config()?;
        Self::spawn_command(TunnelCommand::quick_tunnel(binary, loopback_port).into_std_command())
    }

    /// Spawn an arbitrary command. The injection seam for a harmless fixture child.
    ///
    /// Args:
    ///     command: Must have piped stdout and stderr. Must not be a shell.
    ///
    /// Returns:
    ///     Owner that kills and waits this exact child on stop/drop.
    pub fn spawn_command(mut command: Command) -> io::Result<Self> {
        let child = command.spawn()?;
        Self::from_child(child)
    }

    /// Adopt an already-spawned child. Stdout and stderr must be piped.
    ///
    /// Any failure after this function receives the child kills and waits that exact process.
    /// A reader thread that was already created is joined after the child is reaped.
    ///
    /// Args:
    ///     child: Exact process handle to own.
    ///
    /// Returns:
    ///     Owner that reaps this child on stop/drop.
    pub fn from_child(mut child: Child) -> io::Result<Self> {
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                reap_running_child(&mut child);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "tunnel child stdout is not piped",
                ));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                reap_running_child(&mut child);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "tunnel child stderr is not piped",
                ));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let url = Arc::new(Mutex::new(None));
        let stdout_join = match spawn_reader(
            "telegram-tunnel-out",
            stdout,
            Arc::clone(&stop),
            Arc::clone(&url),
        ) {
            Ok(join) => join,
            Err(error) => {
                reap_running_child(&mut child);
                return Err(error);
            }
        };
        let stderr_join = match spawn_reader(
            "telegram-tunnel-err",
            stderr,
            Arc::clone(&stop),
            Arc::clone(&url),
        ) {
            Ok(join) => join,
            Err(error) => {
                stop.store(true, Ordering::SeqCst);
                reap_running_child(&mut child);
                let _ = stdout_join.join();
                return Err(error);
            }
        };
        Ok(Self {
            child: Some(child),
            stop,
            url,
            stdout_join: Some(stdout_join),
            stderr_join: Some(stderr_join),
        })
    }

    /// Currently parsed `https://<label>.trycloudflare.com` URL, if any.
    pub fn public_url(&self) -> Option<String> {
        self.url.lock().ok().and_then(|guard| guard.clone())
    }

    /// Observe an exited tunnel so callers never keep advertising its obsolete URL.
    pub fn is_running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// Signal readers, kill the exact child when still running, wait/reap it, join readers, clear URL.
    ///
    /// Safe to call twice. An early return that skipped kill+wait would leak a public URL.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.take() {
            reap_running_child(&mut child);
        }
        if let Some(join) = self.stdout_join.take() {
            let _ = join.join();
        }
        if let Some(join) = self.stderr_join.take() {
            let _ = join.join();
        }
        if let Ok(mut url) = self.url.lock() {
            *url = None;
        }
    }
}

impl Drop for TunnelProcess {
    /// Stop the child process and its output readers before releasing the tunnel owner.
    fn drop(&mut self) {
        self.stop();
    }
}

/// App-owned empty config used only while a Mini App quick tunnel is spawned.
///
/// Lives beside the verified binary so it cannot be the user's `~/.cloudflared/config.yml`.
fn isolated_quick_tunnel_config_path() -> PathBuf {
    crate::config::paths::cloudflared_cache_dir().join("quick-tunnel.yml")
}

/// Write an empty isolated config. Never reads, renames, or writes the user-level file.
fn ensure_isolated_quick_tunnel_config() -> io::Result<PathBuf> {
    let path = isolated_quick_tunnel_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        "# MoonTerminal Mini App quick-tunnel isolation. Intentionally empty.\n",
    )?;
    Ok(path)
}

/// Parse the first syntactically valid `https://<label>.trycloudflare.com` URL in `text`.
///
/// Args:
///     text: One stdout/stderr line or a larger buffer.
///
/// Returns:
///     The canonical URL without a trailing path, userinfo, port, or query.
pub fn parse_trycloudflare_url(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(rel) = text[search..].find("https://") {
        let start = search + rel;
        let rest = &text[start + "https://".len()..];
        let host_end = rest
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-')))
            .unwrap_or(rest.len());
        let host = &rest[..host_end];
        if let Some(label) = trycloudflare_label(host) {
            let after = &rest[host_end..];
            if after.starts_with('/') {
                let path = after.as_bytes();
                if path.len() > 1
                    && path[1] != b' '
                    && path[1] != b'\r'
                    && path[1] != b'\n'
                    && path[1] != b'\t'
                {
                    search = start + 8;
                    continue;
                }
            } else if !after.is_empty()
                && !after
                    .starts_with(|ch: char| ch.is_ascii_whitespace() || ch == '"' || ch == '\'')
            {
                search = start + 8;
                continue;
            }
            return Some(format!("https://{label}{TRYCLOUDFLARE_SUFFIX}"));
        }
        search = start + 8;
        let _ = bytes;
    }
    None
}

/// Accept `label.trycloudflare.com` with a single DNS label of `[A-Za-z0-9-]` that does not
/// start or end with a hyphen.
fn trycloudflare_label(host: &str) -> Option<&str> {
    let label = host.strip_suffix(TRYCLOUDFLARE_SUFFIX)?;
    if label.is_empty() || label.len() > 63 || label.contains('.') {
        return None;
    }
    let bytes = label.as_bytes();
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return None;
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        return None;
    }
    Some(label)
}

/// Kill a still-running child if needed, then wait/reap it.
///
/// `Child` drop does neither, so every constructor error after spawn must call this before
/// returning `Err`.
fn reap_running_child(child: &mut Child) {
    let still_running = match child.try_wait() {
        Ok(None) => true,
        Ok(Some(_)) => false,
        Err(_) => true,
    };
    if still_running {
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Spawn a named reader that records the first valid trycloudflare URL.
fn spawn_reader(
    name: &str,
    stream: impl io::Read + Send + 'static,
    stop: Arc<AtomicBool>,
    url: Arc<Mutex<Option<String>>>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.into()).spawn(move || {
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(line) = line else {
                break;
            };
            if let Some(found) = parse_trycloudflare_url(&line) {
                if let Ok(mut slot) = url.lock() {
                    if slot.is_none() {
                        *slot = Some(found);
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests;
