//! Mini App loopback server and tunnel worker.
//!
//! Acquisition and URL observation run on their own service worker. Authenticated requests are
//! relayed to Backend without being blocked behind Telegram long polling. Stop drops the tunnel
//! and loopback listener before the owning service joins this worker.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Secret;
use crate::telegram::TelegramStatus;
use crate::telegram::tunnel::TunnelProcess;
use crate::telegram::web::{MiniAppApiRequest, MiniAppServer, MiniAppServerConfig};

/// First wait after a Mini App acquisition, start, or tunnel-exit failure.
const MINI_APP_RETRY_INITIAL: Duration = Duration::from_secs(1);
/// Cap for Mini App restart backoff on unchanged desired configuration.
const MINI_APP_RETRY_CAP: Duration = Duration::from_secs(32);

/// Typed Mini App runtime status for Settings and the UI adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MiniAppStatus {
    /// Owner exists but neither listener nor tunnel is running.
    Stopped,
    /// Listener is coming up.
    Starting,
    /// Loopback server is bound; no public URL yet.
    Listening {
        /// OS-assigned loopback port.
        port: u16,
    },
    /// Quick tunnel published a trycloudflare URL.
    Tunneling {
        /// OS-assigned loopback port.
        port: u16,
        /// Canonical `https://<label>.trycloudflare.com` URL.
        url: String,
    },
    /// Start failed; the Mini App is unavailable. The bot itself is unaffected and still polling.
    Failed {
        /// Stable machine reason, never a secret.
        reason: MiniAppFailReason,
    },
}

/// Why Mini App start rolled back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiniAppFailReason {
    /// Loopback bind or listener thread failed.
    Server,
    /// `cloudflared` child did not start.
    Tunnel,
}

/// Status and URL events emitted by the owner after start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MiniAppEvent {
    /// Current owner status.
    Status(MiniAppStatus),
    /// Newly parsed public URL.
    PublicUrl(String),
}

/// Stopped-by-default owner of [`MiniAppServer`] and [`TunnelProcess`].
pub struct MiniAppOwner {
    server: Option<MiniAppServer>,
    tunnel: Option<TunnelProcess>,
    events_tx: SyncSender<MiniAppEvent>,
    events_rx: Receiver<MiniAppEvent>,
    api_rx: Option<Receiver<MiniAppApiRequest>>,
    status: MiniAppStatus,
}

impl MiniAppOwner {
    /// Construct a stopped owner. No listener, thread, child, or download is created.
    pub fn new() -> Self {
        let (events_tx, events_rx) = mpsc::sync_channel(16);
        Self {
            server: None,
            tunnel: None,
            events_tx,
            events_rx,
            api_rx: None,
            status: MiniAppStatus::Stopped,
        }
    }

    /// Current owner status. Never contains a token or Bot API URL.
    pub fn status(&self) -> MiniAppStatus {
        self.status.clone()
    }

    /// Drain typed status/URL events. The caller polls this on the owner loop.
    pub fn events(&mut self) -> &mut Receiver<MiniAppEvent> {
        &mut self.events_rx
    }

    /// Bind the loopback server and retain its request receiver before spawning the tunnel.
    ///
    /// Failure rolls back everything already started. The service worker drains the retained
    /// request receiver into Backend; it does not run a placeholder API responder.
    ///
    /// Args:
    ///     token: Bot token used only for Mini App HMAC verification.
    ///     authorized_chat_ids: Paired chat ids required before the session check.
    ///     cloudflared_bin: Already-verified `cloudflared` path. This method does not download.
    ///
    /// Returns:
    ///     `Ok` after the listener, request receiver, and child exist. `Err` after rollback.
    pub fn start(
        &mut self,
        token: Secret,
        authorized_chat_ids: Vec<i64>,
        cloudflared_bin: &Path,
    ) -> Result<(), MiniAppFailReason> {
        self.start_localized(
            token,
            authorized_chat_ids,
            cloudflared_bin,
            std::collections::BTreeMap::new(),
        )
    }

    /// Start with application-localized initial page labels and the same verified process path.
    pub fn start_localized(
        &mut self,
        token: Secret,
        authorized_chat_ids: Vec<i64>,
        cloudflared_bin: &Path,
        labels: std::collections::BTreeMap<String, String>,
    ) -> Result<(), MiniAppFailReason> {
        self.stop();
        self.set_status(MiniAppStatus::Starting);
        let mut server = MiniAppServer::bind_localized(
            MiniAppServerConfig {
                token,
                authorized_chat_ids,
            },
            labels,
        )
        .map_err(|_| {
            self.set_status(MiniAppStatus::Failed {
                reason: MiniAppFailReason::Server,
            });
            MiniAppFailReason::Server
        })?;
        let port = server.port();
        let Some(api_rx) = server.take_events() else {
            server.stop();
            self.set_status(MiniAppStatus::Failed {
                reason: MiniAppFailReason::Server,
            });
            return Err(MiniAppFailReason::Server);
        };
        let tunnel = match TunnelProcess::spawn_quick_tunnel(cloudflared_bin, port) {
            Ok(tunnel) => tunnel,
            Err(_) => {
                self.set_status(MiniAppStatus::Failed {
                    reason: MiniAppFailReason::Tunnel,
                });
                self.server = Some(server);
                self.api_rx = Some(api_rx);
                self.stop();
                return Err(MiniAppFailReason::Tunnel);
            }
        };
        self.server = Some(server);
        self.tunnel = Some(tunnel);
        self.api_rx = Some(api_rx);
        self.set_status(MiniAppStatus::Listening { port });
        Ok(())
    }

    /// Poll the tunnel for a newly parsed URL and emit it once.
    pub fn poll_url(&mut self) {
        if self
            .tunnel
            .as_mut()
            .is_some_and(|tunnel| !tunnel.is_running())
        {
            self.set_status(MiniAppStatus::Failed {
                reason: MiniAppFailReason::Tunnel,
            });
            self.stop();
            return;
        }
        let MiniAppStatus::Listening { port } = self.status else {
            return;
        };
        let Some(url) = self.tunnel.as_ref().and_then(TunnelProcess::public_url) else {
            return;
        };
        self.set_status(MiniAppStatus::Tunneling {
            port,
            url: url.clone(),
        });
        let _ = self.events_tx.try_send(MiniAppEvent::PublicUrl(url));
    }

    /// Tear down the tunnel first, then the listener. Safe to call twice.
    pub fn stop(&mut self) {
        if let Some(mut tunnel) = self.tunnel.take() {
            tunnel.stop();
        }
        if let Some(mut server) = self.server.take() {
            server.stop();
        }
        self.api_rx = None;
        if !matches!(
            self.status,
            MiniAppStatus::Failed { .. } | MiniAppStatus::Stopped
        ) {
            self.set_status(MiniAppStatus::Stopped);
        }
    }
}

impl Default for MiniAppOwner {
    /// Create the same stopped owner used by explicit construction.
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MiniAppOwner {
    /// Stop the web server and tunnel before releasing the owner.
    fn drop(&mut self) {
        self.stop();
    }
}

impl MiniAppOwner {
    /// Publish the latest non-secret Mini App lifecycle state.
    fn set_status(&mut self, status: MiniAppStatus) {
        self.status = status.clone();
        let _ = self.events_tx.try_send(MiniAppEvent::Status(status));
    }
}

/// Path helper so callers can pass a verified binary without importing `config::paths` here.
pub fn default_cloudflared_path() -> PathBuf {
    crate::config::paths::cloudflared_executable_path()
}

/// Project a failed start into Telegram health for existing consumers.
pub fn failed_telegram_status() -> TelegramStatus {
    TelegramStatus::Unavailable
}

/// Own acquisition, tunnel URL observation, and bounded API relay independently of long polling.
pub(super) fn run_service(
    config: Arc<Mutex<crate::config::TelegramConfig>>,
    labels: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    alive: Weak<()>,
    tx: SyncSender<super::Work>,
) {
    let mut owner = MiniAppOwner::new();
    let mut previous: Option<(bool, Vec<i64>)> = None;
    let mut previous_labels = std::collections::BTreeMap::new();
    let mut last_status = MiniAppStatus::Stopped;
    let mut retry_at: Option<Instant> = None;
    let mut retry_failures: u32 = 0;
    while alive.upgrade().is_some() {
        let Ok(saved) = config.lock().map(|cfg| cfg.clone()) else {
            break;
        };
        let desired = (saved.mini_app_enabled, saved.authorized_chat_ids.clone());
        let Ok(current_labels) = labels.lock().map(|labels| labels.clone()) else {
            break;
        };
        if previous.as_ref() != Some(&desired) || previous_labels != current_labels {
            owner.stop();
            retry_at = None;
            retry_failures = 0;
            if desired.0 && !saved.token.is_empty() {
                match start_desired(
                    &mut owner,
                    &saved,
                    &desired.1,
                    &current_labels,
                    &config,
                    &alive,
                    &tx,
                ) {
                    StartOutcome::Shutdown => break,
                    StartOutcome::ConfigChanged => continue,
                    StartOutcome::Started => {}
                    StartOutcome::Failed => {
                        schedule_mini_app_retry(&mut retry_at, &mut retry_failures);
                    }
                }
            }
            previous = Some(desired.clone());
            previous_labels = current_labels;
        }
        owner.poll_url();
        if desired.0
            && !saved.token.is_empty()
            && matches!(owner.status(), MiniAppStatus::Failed { .. })
        {
            if retry_at.is_none() {
                schedule_mini_app_retry(&mut retry_at, &mut retry_failures);
            } else if retry_at.is_some_and(|at| Instant::now() >= at) {
                match start_desired(
                    &mut owner,
                    &saved,
                    &desired.1,
                    &previous_labels,
                    &config,
                    &alive,
                    &tx,
                ) {
                    StartOutcome::Shutdown => break,
                    StartOutcome::ConfigChanged => {
                        retry_at = None;
                        retry_failures = 0;
                        continue;
                    }
                    StartOutcome::Started => {
                        retry_at = None;
                        retry_failures = 0;
                    }
                    StartOutcome::Failed => {
                        // Force the later status publish: last_status may already be Failed
                        // from the previous attempt, and Starting was just pushed.
                        last_status = MiniAppStatus::Starting;
                        schedule_mini_app_retry(&mut retry_at, &mut retry_failures);
                    }
                }
            }
        }
        if owner.status() != last_status {
            let status = owner.status();
            if tx.try_send(super::Work::MiniStatus(status.clone())).is_ok() {
                last_status = status;
            }
        }
        if let Some(rx) = owner.api_rx.as_ref() {
            for request in rx.try_iter().take(64) {
                let _ = tx.try_send(super::Work::MiniApp(request));
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    owner.stop();
}

/// Outcome of one Mini App start attempt against the current desired configuration.
enum StartOutcome {
    Started,
    Failed,
    ConfigChanged,
    Shutdown,
}

/// Acquire the verified binary and start the owner, or classify why that did not happen.
fn start_desired(
    owner: &mut MiniAppOwner,
    saved: &crate::config::TelegramConfig,
    authorized_chat_ids: &[i64],
    current_labels: &std::collections::BTreeMap<String, String>,
    config: &Arc<Mutex<crate::config::TelegramConfig>>,
    alive: &Weak<()>,
    tx: &SyncSender<super::Work>,
) -> StartOutcome {
    let _ = tx.try_send(super::Work::MiniStatus(MiniAppStatus::Starting));
    match crate::telegram::cloudflared::ensure_verified_cloudflared() {
        Ok(binary) if alive.upgrade().is_some() => {
            let still_wanted = config.lock().is_ok_and(|current| {
                current.mini_app_enabled && current.authorized_chat_ids == authorized_chat_ids
            });
            if !still_wanted {
                return StartOutcome::ConfigChanged;
            }
            match owner.start_localized(
                saved.token.clone(),
                authorized_chat_ids.to_vec(),
                &binary,
                current_labels.clone(),
            ) {
                Ok(()) => StartOutcome::Started,
                Err(_) => StartOutcome::Failed,
            }
        }
        Ok(_) => StartOutcome::Shutdown,
        Err(_) => {
            owner.set_status(MiniAppStatus::Failed {
                reason: MiniAppFailReason::Tunnel,
            });
            StartOutcome::Failed
        }
    }
}

/// Exponential wait for the next Mini App restart of unchanged desired configuration.
fn mini_app_retry_wait(failures: u32) -> Duration {
    MINI_APP_RETRY_INITIAL
        .saturating_mul(1u32 << failures.min(5))
        .min(MINI_APP_RETRY_CAP)
}

/// Arm the next bounded retry without blocking the owner loop.
fn schedule_mini_app_retry(retry_at: &mut Option<Instant>, retry_failures: &mut u32) {
    let wait = mini_app_retry_wait(*retry_failures);
    *retry_failures = retry_failures.saturating_add(1);
    *retry_at = Some(Instant::now() + wait);
}
