//! Short-lived administrator sessions, serialized per exchange and retired even without callers.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use russh::keys::{HashAlg, PrivateKey};

use crate::ssh::{Auth, Conn, Output, SecretOutput, Target};

/// Station use extends the login for two minutes, never for the terminal's whole lifetime.
const ADMIN_IDLE_WINDOW: Duration = Duration::from_secs(120);

/// Every input that selects the peer and authenticated administrator; no private key material.
#[derive(Clone, PartialEq, Eq)]
struct Identity {
    host: String,
    port: u16,
    user: String,
    key: String,
    pin: String,
}

/// A connection's expiry is measured from completion, so an active exchange cannot be retired.
struct Entry<K, C> {
    key: K,
    conn: C,
    used: Instant,
}

/// Network-independent policy, with opening, exchange and time supplied by the caller.
struct Cache<K, C> {
    entries: Vec<Entry<K, C>>,
}

impl<K: PartialEq + Clone, C> Cache<K, C> {
    /// Start empty; no network or background work is needed to test the policy.
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Drop every expired login, including peers that are never selected again.
    fn expire(&mut self, now: Instant) {
        self.entries
            .retain(|entry| now.saturating_duration_since(entry.used) < ADMIN_IDLE_WINDOW);
    }

    /// Locate a live slot or open it once, returning whether its connection was reused.
    fn connect(
        &mut self,
        key: &K,
        now: Instant,
        open: &mut impl FnMut() -> anyhow::Result<C>,
    ) -> anyhow::Result<(usize, bool)> {
        self.expire(now);
        if let Some(index) = self.entries.iter().position(|entry| &entry.key == key) {
            self.entries[index].used = now;
            return Ok((index, true));
        }
        let conn = open()?;
        self.entries.push(Entry {
            key: key.clone(),
            conn,
            used: now,
        });
        Ok((self.entries.len() - 1, false))
    }

    /// Retry only one failed exchange on a reused transport, never an entire station job.
    fn exchange<R>(
        &mut self,
        key: &K,
        mut now: impl FnMut() -> Instant,
        mut open: impl FnMut() -> anyhow::Result<C>,
        mut run: impl FnMut(&C) -> anyhow::Result<R>,
        transport: impl Fn(&anyhow::Error) -> bool,
    ) -> anyhow::Result<R> {
        let (index, reused) = self.connect(key, now(), &mut open)?;
        let result = run(&self.entries[index].conn);
        if result.as_ref().is_err_and(&transport) {
            self.entries.remove(index);
            if reused {
                let (index, _) = self.connect(key, now(), &mut open)?;
                let result = run(&self.entries[index].conn);
                self.entries[index].used = now();
                if result.as_ref().is_err_and(transport) {
                    self.entries.remove(index);
                }
                return result;
            }
        } else {
            self.entries[index].used = now();
        }
        result
    }
}

/// One mutex keeps concurrent station jobs from sharing an in-flight SSH channel exchange.
struct Shared<K = Identity, C = Conn> {
    cache: Mutex<Cache<K, C>>,
    wake: Condvar,
}

impl<K: PartialEq + Clone, C> Shared<K, C> {
    /// Serialize complete exchanges, including a reconnect, against use and idle retirement.
    fn exchange<R>(
        &self,
        key: &K,
        open: impl FnMut() -> anyhow::Result<C>,
        run: impl FnMut(&C) -> anyhow::Result<R>,
        transport: impl Fn(&anyhow::Error) -> bool,
    ) -> anyhow::Result<R> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let result = cache.exchange(key, Instant::now, open, run, transport);
        self.wake.notify_one();
        result
    }
}

/// Start one deadline-driven reaper; spawning failure refuses caching instead of leaking logins.
fn shared() -> anyhow::Result<&'static Arc<Shared>> {
    static SHARED: OnceLock<Result<Arc<Shared>, std::io::Error>> = OnceLock::new();
    SHARED
        .get_or_init(|| {
            let shared = Arc::new(Shared {
                cache: Mutex::new(Cache::new()),
                wake: Condvar::new(),
            });
            let worker = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("station-ssh-idle".into())
                .spawn(move || {
                    let mut cache = worker.cache.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        cache.expire(Instant::now());
                        cache = match cache
                            .entries
                            .iter()
                            .map(|entry| entry.used + ADMIN_IDLE_WINDOW)
                            .min()
                        {
                            Some(deadline) => {
                                worker
                                    .wake
                                    .wait_timeout(
                                        cache,
                                        deadline.saturating_duration_since(Instant::now()),
                                    )
                                    .unwrap_or_else(|e| e.into_inner())
                                    .0
                            }
                            None => worker.wake.wait(cache).unwrap_or_else(|e| e.into_inner()),
                        };
                    }
                })?;
            Ok(shared)
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("station SSH idle worker: {e}"))
}

/// A station login identity, rather than ownership of a connection that could stay open forever.
pub struct AdminConn {
    target: Target,
    identity: Identity,
    app: PrivateKey,
    shared: Arc<Shared>,
}

impl AdminConn {
    /// Establish or reuse the full pinned login; all future exchanges use the same cache policy.
    pub(super) fn open(
        target: &Target,
        user: &str,
        app: PrivateKey,
        pin: &str,
    ) -> anyhow::Result<Self> {
        let conn = Self {
            target: target.clone(),
            identity: Identity {
                host: target.host.clone(),
                port: target.port,
                user: user.to_owned(),
                key: app.public_key().fingerprint(HashAlg::Sha256).to_string(),
                pin: pin.to_owned(),
            },
            app,
            shared: Arc::clone(shared()?),
        };
        conn.exchange(|_| Ok(()))?;
        Ok(conn)
    }

    /// Open with the same host-key verification and authentication errors as the uncached path.
    fn fresh(&self) -> anyhow::Result<Conn> {
        Ok(Conn::open(
            &self.target,
            &Auth::Key {
                user: &self.identity.user,
                key: &self.app,
            },
            &self.identity.pin,
        )?)
    }

    /// Serialize an exchange and reset its idle deadline even when its remote command fails.
    fn exchange<R>(&self, run: impl FnMut(&Conn) -> anyhow::Result<R>) -> anyhow::Result<R> {
        self.shared.exchange(
            &self.identity,
            || self.fresh(),
            run,
            |e| e.downcast_ref::<crate::ssh::TransportLost>().is_some(),
        )
    }

    /// The administrator selected when this station operation began.
    pub fn user(&self) -> &str {
        &self.identity.user
    }

    /// The mandatory peer pin, already verified when the cache entry was opened.
    pub fn fingerprint(&self) -> &str {
        &self.identity.pin
    }

    /// Run one command; a completed reply is returned without any retry of its operation.
    pub fn run(&self, command: &str, stdin: &[u8], timeout: Duration) -> anyhow::Result<Output> {
        let mut out = self.run_secret(command, stdin, timeout)?;
        Ok(Output {
            status: out.status,
            stdout: std::mem::take(&mut *out.stdout),
            stderr: std::mem::take(&mut *out.stderr),
        })
    }

    /// Keep credential output zeroizing through both the first exchange and its possible retry.
    pub fn run_secret(
        &self,
        command: &str,
        stdin: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<SecretOutput> {
        self.exchange(|conn| conn.run_secret_reusable(command, stdin, timeout))
    }
}

#[cfg(test)]
mod tests;
