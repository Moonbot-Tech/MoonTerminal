//! Native crash logging on macOS and Linux: a signal handler that writes the same report header
//! the Windows filter writes, then hands the signal back so the operating system's own crash
//! report is still produced.
//!
//! On a Mac the system already keeps the fuller record — `~/Library/Logs/DiagnosticReports/
//! MoonTerminal-*.ips`, every thread's stack with the binary's UUID — so this handler does not
//! try to replace it. What it adds is the piece that record lacks: a line in OUR `panic.log`,
//! next to the Rust panics, saying that the process died natively, in which build, after how long,
//! on which thread, at what address, and the faulting thread's stack as `moonterminal+0xOFFSET`
//! (offsets into the image; no symbols are built for them, by decision). Without it a native
//! crash on a Mac leaves `panic.log` untouched, and "the terminal just closed" is all the log
//! can say.
//!
//! Everything here runs inside a signal handler, so nothing allocates and nothing of ours locks:
//! the report is formatted into a static buffer and written with `write(2)`. It goes out in two
//! pieces, the header first: the frame walk (`backtrace::trace_unsynchronized`, named through
//! `dladdr`) is the one step that is not async-signal-safe — the loader's lock, if the fault
//! happened inside `dlopen`, could hang it — and a hang there must not cost the line that says
//! what died. The bypass of `applog::panic_log` is deliberate and safe: that sink allocates and
//! redacts network addresses, and this report is built from numbers, build constants and image
//! names alone. When the write is done the handler restores whatever handler was installed
//! before it — the standard library's stack-overflow detector, or the default action — and
//! returns. The faulting instruction then re-executes and faults again into that handler, which
//! is how the system report, and Rust's "thread has overflowed its stack" message, keep working.
//!
//! `SA_ONSTACK` runs the handler on the alternate stack the standard library gives every thread
//! it creates. A thread the system created — a dispatch queue, Metal's own — has none, and there
//! the handler runs on the thread's stack; a fault on such a thread still reports, a stack
//! OVERFLOW on one does not, and never did.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

/// Signals a native fault arrives as. `SIGTRAP` is what `core::intrinsics::abort` raises on
/// arm64 (`brk`), `SIGILL` its x86_64 form (`ud2`); `SIGABRT` is left alone — it is the panic
/// hook's exit, and that hook has already written its own report by then.
const SIGNALS: [libc::c_int; 5] = [
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGTRAP,
];

/// Frames the stack section prints; the fault and its callers up to the run loop sit well inside.
const STACK_FRAMES: usize = 40;

/// Size of the report buffer — static, not on the stack: on Linux the alternate signal stack is
/// eight kilobytes, and a buffer this size on it would leave the unwinder no room. A full report
/// is under two kilobytes; the rest is headroom for long image names.
const BUFFER: usize = 4096;

/// Entries the handler accepts: the first writes the report, the next three write one line
/// each, anything past that returns at once. Bounds a fault inside the handler itself.
const MAX_ENTRIES: u32 = 4;

/// Longest a second thread holds the process open for the first report, in 50 ms steps.
const FIRST_REPORT_WAIT_STEPS: u32 = 30 * 20;

/// The report file, relative to the working directory like `paths::panic_log`. A C string
/// because the handler must not allocate to build a path.
const PANIC_LOG: &std::ffi::CStr = c"panic.log";

/// A cell the handler may write to from a signal context. Soundness rests on the entry counter:
/// `PREVIOUS` is written only by `install`, before the handler exists; `REPORT` only by entry 0.
struct HandlerCell<T>(std::cell::UnsafeCell<T>);

// SAFETY: see the type's doc — every cell has exactly one writer at a time by construction.
unsafe impl<T> Sync for HandlerCell<T> {}

/// Handlers that were installed before ours, one per entry of [`SIGNALS`].
static PREVIOUS: HandlerCell<[Option<libc::sigaction>; SIGNALS.len()]> =
    HandlerCell(std::cell::UnsafeCell::new([None; SIGNALS.len()]));

/// The report buffer of entry 0.
static REPORT: HandlerCell<Buf<BUFFER>> = HandlerCell(std::cell::UnsafeCell::new(Buf::new()));

/// Bytes a later entry's single line needs; small enough to live on the alternate stack.
const LINE: usize = 512;

/// `pthread_self()` of the installing thread, so the report can say whether the fault was on it.
static MAIN_THREAD: AtomicUsize = AtomicUsize::new(0);

/// Handler entries so far; entry 0 owns the report.
static ENTRIES: AtomicU32 = AtomicU32::new(0);
/// Thread of entry 0, and whether its report is complete — what a later entry waits on.
static FIRST_THREAD: AtomicUsize = AtomicUsize::new(0);
static FIRST_DONE: AtomicBool = AtomicBool::new(false);

/// Registers the handler for every signal in [`SIGNALS`], remembering what was there before.
///
/// Call once, on the main thread, before any window exists. The previous handler is read and
/// stored BEFORE ours goes in, so a fault on another thread in between finds it already recorded.
pub(super) fn install() {
    MAIN_THREAD.store(unsafe { libc::pthread_self() } as usize, Ordering::Relaxed);
    for (i, sig) in SIGNALS.iter().enumerate() {
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(*sig, std::ptr::null(), &mut previous) } != 0 {
            continue;
        }
        // SAFETY: the single write to this cell, before the handler is registered.
        unsafe { (*PREVIOUS.0.get())[i] = Some(previous) };
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        unsafe { libc::sigaction(*sig, &action, std::ptr::null_mut()) };
    }
}

/// Puts back the handler that was installed before ours for `sig`, or the default action.
fn restore_previous(sig: libc::c_int) {
    let index = SIGNALS.iter().position(|s| *s == sig);
    // SAFETY: read-only after install.
    let previous = index.and_then(|i| unsafe { (*PREVIOUS.0.get())[i] });
    match previous {
        Some(action) => unsafe {
            libc::sigaction(sig, &action, std::ptr::null_mut());
        },
        None => unsafe {
            let mut default: libc::sigaction = std::mem::zeroed();
            default.sa_sigaction = libc::SIG_DFL;
            libc::sigaction(sig, &default, std::ptr::null_mut());
        },
    }
}

/// A fixed buffer that `write!` fills without allocating; text past the end is dropped.
struct Buf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Buf<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> std::fmt::Write for Buf<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = N - self.len;
        let take = s.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// Appends `bytes` to `panic.log` with raw syscalls, retrying an interrupted write. Failure is
/// silent: there is nothing left to report it to.
fn append(bytes: &[u8]) {
    let fd = unsafe {
        libc::open(
            PANIC_LOG.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
            0o644,
        )
    };
    if fd < 0 {
        return;
    }
    let mut written = 0;
    while written < bytes.len() {
        let rest = &bytes[written..];
        let n = unsafe { libc::write(fd, rest.as_ptr().cast(), rest.len()) };
        if n < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
    unsafe { libc::close(fd) };
}

fn signal_name(sig: libc::c_int) -> &'static str {
    match sig {
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGBUS => "SIGBUS",
        libc::SIGILL => "SIGILL",
        libc::SIGFPE => "SIGFPE",
        libc::SIGTRAP => "SIGTRAP",
        _ => "signal",
    }
}

/// Writes `name+0xOFFSET` for `ip` when a loaded image claims it, else the raw address. The name
/// is the image's file name without its directory.
fn write_frame(buf: &mut impl std::fmt::Write, index: usize, ip: usize) {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let known = unsafe { libc::dladdr(ip as *const libc::c_void, &mut info) } != 0
        && !info.dli_fname.is_null()
        && !info.dli_fbase.is_null();
    if !known {
        let _ = writeln!(buf, "  {index:>2}: 0x{ip:016X} (no image)");
        return;
    }
    let path = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_bytes();
    let start = path.iter().rposition(|b| *b == b'/').map_or(0, |p| p + 1);
    let name = std::str::from_utf8(&path[start..]).unwrap_or("?");
    let base = info.dli_fbase as usize;
    let _ = writeln!(buf, "  {index:>2}: {name}+0x{:X}", ip.wrapping_sub(base));
}

/// The `NATIVE CRASH: …` line and the build/uptime/thread line, into `buf`.
fn write_head(
    buf: &mut impl std::fmt::Write,
    sig: libc::c_int,
    info: *mut libc::siginfo_t,
    entry: u32,
) {
    let (fault, code) = unsafe { info.as_ref() }
        .map(|i| (unsafe { i.si_addr() } as usize, i.si_code))
        .unwrap_or((0, 0));
    let thread = unsafe { libc::pthread_self() } as usize;
    let main = MAIN_THREAD.load(Ordering::Relaxed);
    let note = if entry == 0 {
        ""
    } else {
        " (a later entry, while the first report was being written)"
    };
    let _ = writeln!(
        buf,
        "NATIVE CRASH{note}: {} ({sig}) code={code} at address 0x{fault:016X}",
        signal_name(sig)
    );
    let _ = buf.write_str("  ");
    let _ = super::write_header(buf, thread as u64, main != 0 && thread == main);
    let _ = buf.write_str("\n");
}

/// The handler. Entry 0 writes the report; a later entry on another thread writes one line and
/// waits for the report, a nested entry on the same thread writes its line and returns. Each
/// then restores the previous handler for its signal and returns, so the fault re-executes into
/// that handler.
extern "C" fn handler(sig: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    let entry = ENTRIES.fetch_add(1, Ordering::SeqCst);
    if entry >= MAX_ENTRIES {
        restore_previous(sig);
        return;
    }
    let this_thread = unsafe { libc::pthread_self() } as usize;

    if entry > 0 {
        // Its own small buffer: the static one belongs to entry 0, which may be mid-write.
        let mut line = Buf::<LINE>::new();
        write_head(&mut line, sig, info, entry);
        append(line.as_bytes());
        if this_thread != FIRST_THREAD.load(Ordering::SeqCst) {
            let nap = libc::timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            };
            let mut steps = 0;
            while !FIRST_DONE.load(Ordering::SeqCst) && steps < FIRST_REPORT_WAIT_STEPS {
                unsafe { libc::nanosleep(&nap, std::ptr::null_mut()) };
                steps += 1;
            }
        }
        restore_previous(sig);
        return;
    }

    FIRST_THREAD.store(this_thread, Ordering::SeqCst);
    // SAFETY: entry 0 is the only writer of `REPORT`, and there is exactly one entry 0.
    let buf = unsafe { &mut *REPORT.0.get() };

    // The head goes out on its own before the frame walk, which is the step that may not return.
    write_head(buf, sig, info, 0);
    append(buf.as_bytes());

    buf.clear();
    let _ = writeln!(buf, "--- stack (image+offset, handler frames first) ---");
    let mut index = 0usize;
    unsafe {
        backtrace::trace_unsynchronized(|frame| {
            write_frame(buf, index, frame.ip() as usize);
            index += 1;
            index < STACK_FRAMES
        });
    }
    let _ = writeln!(
        buf,
        "--- end ---\n  the OS crash report carries every thread: Console → Crash Reports, or ~/Library/Logs/DiagnosticReports"
    );
    append(buf.as_bytes());

    // Restore BEFORE releasing the waiters: the fault this report describes should be the one
    // the system's own reporter sees, and a released waiter re-faults on its own as soon as it
    // returns.
    restore_previous(sig);
    FIRST_DONE.store(true, Ordering::SeqCst);
}
