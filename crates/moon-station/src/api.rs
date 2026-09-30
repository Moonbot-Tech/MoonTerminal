//! The station's control API (`moon_core::station_api`, STATION.md §4.5): a Unix socket in the
//! runtime directory, and `moon-station ctl`, its client for the helper.
//!
//! The socket's thread moves frames: a request about the bot, the configuration or the cores goes
//! to the main loop over a channel and is answered there, between two drains — they have one
//! owner, and no lock is taken on them. A read of the station's own files (the terminal's pull,
//! [`Direct`]) is answered on this thread, so it never holds up the loop. One exchange per connection, one connection at a time, each read
//! and write bounded by a timeout, so a client that stalls holds the socket for seconds, never
//! the loop.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use anyhow::Context;
use moon_core::station_api::{CtlOutput, Hello, MAX_FRAME, PROTO_VERSION, Reply, Request};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// How long a connection may take to send or receive one frame.
#[cfg_attr(not(unix), allow(dead_code))]
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a request waits for the main loop, which drains every 100 ms.
#[cfg_attr(not(unix), allow(dead_code))]
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// How much longer than a call's deadline its connection waits: an answer made just before the
/// deadline still reaches the client.
const DELIVERY_GRACE: Duration = Duration::from_secs(1);

/// One request on its way to the main loop, with where its reply goes.
struct Call {
    request: Request,
    reply: SyncSender<Reply>,
    /// Past it the client has been told nothing was done, so nothing is: a stopping station, or a
    /// loop held up, never applies a change its caller has given up on.
    deadline: Instant,
}

/// The requests this thread answers itself, from files alone; `None` sends one to the main loop.
pub type Direct = fn(&Request) -> Option<Reply>;

/// The listening socket's side the main loop holds: the requests waiting for an answer.
pub struct Api {
    calls: Receiver<Call>,
    path: PathBuf,
}

impl Api {
    /// Listen on `path`, group-readable (`0660`, the service's group); a socket a previous run
    /// left behind is replaced.
    #[cfg(unix)]
    pub fn start(path: PathBuf, direct: Direct) -> anyhow::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("remove {}", path.display())),
        }
        let listener =
            UnixListener::bind(&path).with_context(|| format!("bind {}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
        let (tx, calls) = mpsc::channel();
        std::thread::Builder::new()
            .name("api".into())
            .spawn(move || serve(listener, tx, direct))?;
        Ok(Self { calls, path })
    }

    /// No Unix socket off Unix: the station's Windows build is for tests only.
    #[cfg(not(unix))]
    pub fn start(path: PathBuf, _direct: Direct) -> anyhow::Result<Self> {
        anyhow::bail!(
            "the control API needs a Unix socket ({} not created)",
            path.display()
        )
    }

    /// Answer every waiting request still within its deadline, on the caller's thread.
    pub fn drain(&self, mut answer: impl FnMut(Request) -> Reply) {
        for call in self.calls.try_iter() {
            if Instant::now() >= call.deadline {
                continue;
            }
            // A client that went away has dropped its receiver: nothing to tell it.
            let _ = call.reply.send(answer(call.request));
        }
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn serve(listener: std::os::unix::net::UnixListener, calls: Sender<Call>, direct: Direct) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                log::warn!("api: accept failed: {e}");
                // A lasting accept error must not spin this thread.
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        let bounded = stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)));
        if let Err(e) = bounded
            .map_err(anyhow::Error::from)
            .and_then(|()| exchange(&mut stream, &calls, ANSWER_WITHIN, direct))
        {
            log::warn!("api: {e:#}");
        }
    }
}

/// One connection's exchange: hello, the request, the main loop's reply — or why there is none.
#[cfg_attr(not(unix), allow(dead_code))]
fn exchange(
    stream: &mut (impl Read + Write),
    calls: &Sender<Call>,
    within: Duration,
    direct: Direct,
) -> anyhow::Result<()> {
    write_frame(
        stream,
        &Hello {
            proto_version: PROTO_VERSION,
            station_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )?;
    let request: Request = match read_frame(stream) {
        Ok(request) => request,
        Err(e) => {
            // A request from a newer terminal, or garbage: said so, rather than a bare hang-up.
            let _ = write_frame(stream, &Reply::Err(format!("unreadable request: {e:#}")));
            return Err(e.context("read the request"));
        }
    };
    if let Some(reply) = direct(&request) {
        return write_frame(stream, &reply);
    }
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let call = Call {
        request,
        reply: reply_tx,
        deadline: Instant::now() + within,
    };
    let reply = match calls.send(call) {
        Err(_) => Reply::Err("the station is stopping".into()),
        Ok(()) => reply_rx
            .recv_timeout(within + DELIVERY_GRACE)
            .unwrap_or_else(|_| {
                Reply::Err("the station did not answer in time; nothing was changed".into())
            }),
    };
    write_frame(stream, &reply)
}

/// One frame: a big-endian `u32` length, then that many bytes of JSON.
pub fn write_frame(w: &mut impl Write, value: &impl Serialize) -> anyhow::Result<()> {
    let body = serde_json::to_vec(value)?;
    anyhow::ensure!(body.len() <= MAX_FRAME, "a frame of {} bytes", body.len());
    w.write_all(&(body.len() as u32).to_be_bytes())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Read one frame written by [`write_frame`]; a length over [`MAX_FRAME`] is refused unread.
pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> anyhow::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "a frame of {len} bytes");
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

/// `moon-station ctl [--socket <path>]`: one request from stdin to the running station, its hello
/// and reply on stdout as one JSON line ([`CtlOutput`]). The exit code says whether a reply came,
/// not what it says: a refusal is a reply (`{"err": …}`). The version is the caller's to judge.
pub fn ctl(args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    const USAGE: &str = "usage: moon-station ctl [--socket <path>] < request.json";
    let mut socket = PathBuf::from(moon_core::station_api::SERVICE_SOCKET);
    let mut args = args;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                socket = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("--socket needs a path; {USAGE}"))?;
            }
            other => anyhow::bail!("unknown argument {other:?}; {USAGE}"),
        }
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(input.trim()).context("the request on stdin")?;
    let output = call(&socket, &request)?;
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

#[cfg(unix)]
fn call(socket: &std::path::Path, request: &Request) -> anyhow::Result<CtlOutput> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket).with_context(|| {
        format!(
            "connect {} (is the station running, and new enough to have its API?)",
            socket.display()
        )
    })?;
    stream.set_read_timeout(Some(ANSWER_WITHIN + IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let hello: Hello = read_frame(&mut stream).context("read the station's hello")?;
    write_frame(&mut stream, request)?;
    let reply = read_frame(&mut stream).context("read the station's reply")?;
    Ok(CtlOutput { hello, reply })
}

#[cfg(not(unix))]
fn call(_socket: &std::path::Path, _request: &Request) -> anyhow::Result<CtlOutput> {
    anyhow::bail!("the control API needs a Unix socket")
}

#[cfg(test)]
mod tests;
