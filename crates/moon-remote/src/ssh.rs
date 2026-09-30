//! A blocking SSH facade over `russh`: open a connection, run one command with its stdin, read
//! back its exit status and output.
//!
//! Every connection owns a one-worker runtime, so the session keeps answering the server while
//! the caller is busy with another connection — the setup holds its first login open as a
//! lifeline while it tests new ones, and that login must still be alive when a rollback needs it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use russh::client::{self, AuthResult};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind};
use zeroize::Zeroizing;

/// How long a TCP connect plus key exchange may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Keepalive cadence: a lifeline idle for minutes must not be dropped by a NAT on the way.
const KEEPALIVE_EVERY: Duration = Duration::from_secs(15);

/// Where to connect.
#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    /// `host:port`, the key the host's pin is stored under.
    pub fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// How to log in.
pub enum Auth<'a> {
    Password { user: &'a str, password: &'a str },
    Key { user: &'a str, key: &'a PrivateKey },
}

impl Auth<'_> {
    pub fn user(&self) -> &str {
        match self {
            Self::Password { user, .. } | Self::Key { user, .. } => user,
        }
    }
}

/// Why a connection did not open.
#[derive(Debug)]
pub enum OpenError {
    /// The server answered these credentials with a refusal and kept the connection open —
    /// never a connection that merely ended mid-login, which russh reports the same way.
    /// After the server is closed this is the answer a password login MUST get, so it is kept
    /// apart from every other failure.
    Refused {
        /// The refusal still lists `password` or `keyboard-interactive` as methods to try.
        password_offered: bool,
    },
    /// The host presented a different key than the one pinned at first contact.
    HostKeyChanged {
        pinned: String,
        presented: String,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { .. } => f.write_str("the server refused the login"),
            Self::HostKeyChanged { pinned, presented } => write!(
                f,
                "the host key changed: pinned {pinned}, presented {presented} — refusing to connect"
            ),
            Self::Other(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// What one command returned.
pub struct Output {
    /// `None` when the server closed the channel without an exit status (a signal, a drop).
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Credential-bearing SSH output, protected during reception and on timeout or failure.
pub struct SecretOutput {
    pub status: Option<u32>,
    pub stdout: Zeroizing<Vec<u8>>,
    pub stderr: Zeroizing<Vec<u8>>,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// Checks the host key against the pin; remembers what was presented.
struct PinCheck {
    pinned: Option<String>,
    presented: Arc<Mutex<Option<String>>>,
}

impl client::Handler for PinCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // A certificate would need a CA to trust; nothing here has one.
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let accept = self.pinned.as_deref().is_none_or(|pin| pin == fingerprint);
        *self.presented.lock().unwrap_or_else(|e| e.into_inner()) = Some(fingerprint);
        Ok(accept)
    }
}

/// One logged-in SSH connection.
pub struct Conn {
    rt: tokio::runtime::Runtime,
    handle: client::Handle<PinCheck>,
    user: String,
    fingerprint: String,
}

impl Conn {
    /// Connect, check the host key against `pinned` (`None` = first contact, accept and report),
    /// and log in.
    pub fn open(target: &Target, auth: &Auth<'_>, pinned: Option<&str>) -> Result<Self, OpenError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .context("ssh runtime")
            .map_err(OpenError::Other)?;
        let presented = Arc::new(Mutex::new(None));
        let handler = PinCheck {
            pinned: pinned.map(str::to_owned),
            presented: presented.clone(),
        };
        let config = Arc::new(client::Config {
            keepalive_interval: Some(KEEPALIVE_EVERY),
            keepalive_max: 4,
            nodelay: true,
            ..Default::default()
        });
        let addr = (target.host.clone(), target.port);
        let connected = rt.block_on(async {
            tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, addr, handler)).await
        });
        let mut handle = match connected {
            Ok(Ok(handle)) => handle,
            Ok(Err(e)) => {
                let presented = presented.lock().unwrap_or_else(|e| e.into_inner()).clone();
                return Err(match (pinned, presented) {
                    (Some(pin), Some(seen)) if pin != seen => OpenError::HostKeyChanged {
                        pinned: pin.to_owned(),
                        presented: seen,
                    },
                    _ => OpenError::Other(
                        anyhow::Error::new(e).context(format!("connect to {}", target.addr())),
                    ),
                });
            }
            Err(_) => {
                return Err(OpenError::Other(anyhow::anyhow!(
                    "connect to {}: no answer within {}s",
                    target.addr(),
                    CONNECT_TIMEOUT.as_secs()
                )));
            }
        };
        let fingerprint = presented
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| OpenError::Other(anyhow::anyhow!("the host presented no key")))?;
        let result = rt.block_on(async {
            match auth {
                Auth::Password { user, password } => {
                    handle.authenticate_password(*user, *password).await
                }
                Auth::Key { user, key } => {
                    let key = PrivateKeyWithHashAlg::new(Arc::new((*key).clone()), None);
                    handle.authenticate_publickey(*user, key).await
                }
            }
        });
        match result {
            Ok(result) if result.success() => Ok(Self {
                rt,
                handle,
                user: auth.user().to_owned(),
                fingerprint,
            }),
            // russh answers a connection that ended mid-login with an empty `Failure` too; only a
            // server that is still there and names what it would accept has refused.
            Ok(AuthResult::Failure {
                remaining_methods, ..
            }) if !remaining_methods.is_empty() && !handle.is_closed() => Err(OpenError::Refused {
                password_offered: remaining_methods
                    .iter()
                    .any(|m| matches!(m, MethodKind::Password | MethodKind::KeyboardInteractive)),
            }),
            Ok(_) => Err(OpenError::Other(anyhow::anyhow!(
                "the connection to {} ended during the login",
                target.addr()
            ))),
            Err(e) => Err(OpenError::Other(anyhow::Error::new(e).context("ssh login"))),
        }
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    /// The SHA256 fingerprint of the key the host presented.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Run `command` through the login shell, feed it `stdin` and close it, wait for it to end.
    pub fn run(&self, command: &str, stdin: &[u8], timeout: Duration) -> anyhow::Result<Output> {
        let mut out = self.run_secret(command, stdin, timeout)?;
        Ok(Output {
            status: out.status,
            stdout: std::mem::take(&mut *out.stdout),
            stderr: std::mem::take(&mut *out.stderr),
        })
    }

    /// Run a credential read without converting its buffers to ordinary strings or vectors.
    pub fn run_secret(
        &self,
        command: &str,
        stdin: &[u8],
        timeout: Duration,
    ) -> anyhow::Result<SecretOutput> {
        self.rt.block_on(async {
            tokio::time::timeout(timeout, self.run_async(command, stdin))
                .await
                .map_err(|_| anyhow::anyhow!("no end within {}s", timeout.as_secs()))?
        })
    }

    /// Receive into zeroizing buffers so a cancelled read cannot leave a partial credential.
    async fn run_async(&self, command: &str, stdin: &[u8]) -> anyhow::Result<SecretOutput> {
        let channel = self.handle.channel_open_session().await?;
        channel.exec(true, command).await?;
        // Written and read at once: a command that answers while it still reads a large stdin
        // would otherwise fill the channel's inbound queue, and the connection's one read loop —
        // which also carries the window adjusts the writer waits for — would stall on it.
        let (mut read, write) = channel.split();
        let send = async {
            if !stdin.is_empty() {
                write.data(stdin).await?;
            }
            write.eof().await
        };
        let receive = async {
            let mut out = SecretOutput {
                status: None,
                stdout: Zeroizing::new(Vec::new()),
                stderr: Zeroizing::new(Vec::new()),
            };
            while let Some(msg) = read.wait().await {
                match msg {
                    ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => out.stderr.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                    _ => {}
                }
            }
            out
        };
        tokio::pin!(send, receive);
        // The output decides when the command is over. russh never wakes a writer waiting for
        // window space when the channel closes, so a command that exits without reading all of
        // its stdin would leave the writer — and a join on it — hanging until the timeout; the
        // unfinished write is dropped instead, and the exit status is the answer.
        let mut sent = None;
        let out = loop {
            tokio::select! {
                result = &mut send, if sent.is_none() => sent = Some(result),
                out = &mut receive => break out,
            }
        };
        if let (None, Some(Err(e))) = (out.status, sent) {
            return Err(e.into());
        }
        Ok(out)
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        let handle = &self.handle;
        self.rt.block_on(async {
            let bye = handle.disconnect(russh::Disconnect::ByApplication, "", "en");
            let _ = tokio::time::timeout(Duration::from_secs(3), bye).await;
        });
    }
}
