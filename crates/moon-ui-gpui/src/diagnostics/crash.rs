//! Native crash logging, complementing the Rust panic hook installed during startup. A panic hook
//! handles Rust panics but not native faults such as access violations in DirectX or GPUI code.
//! On Windows this module installs a process-wide top-level exception filter (Structured
//! Exception Handling): in a process without a debugger it runs when an exception remains
//! unhandled and reaches `UnhandledExceptionFilter`, appends a report to the same `panic.log` the
//! panic hook uses and, last, writes a minidump beside the logs. On macOS and Linux [`posix`]
//! installs a signal handler that writes the report's header and stack to the same file and then
//! lets the system's own crash reporter run.
//!
//! The report is ordered by how likely each part is to survive: the exception code and address
//! first (plain formatting), then everything [`context`] can read without `dbghelp`, then the
//! window messages that led here, then the symbolised backtrace and the minidump — the two that go
//! through `dbghelp` and can fault themselves. A report cut short still carries its top.
//!
//! What a stripped release build gets from it: the faulting instruction as `moonterminal+0xRVA`
//! (an offset into the image, no symbols by decision), the registers, the raw stack in the same
//! form, the thread, the process's memory and GUI-object counts, the foreign DLLs loaded into it,
//! the last [`super::msg_ring::REPORT_LINES`] window messages plus the older notable ones the ring
//! still holds, and the dump.

#[cfg(windows)]
mod context;
#[cfg(windows)]
pub(crate) mod minidump;
#[cfg(unix)]
mod posix;

/// The `build: … · uptime … · thread=…` line both platforms' reports carry, written without
/// allocating — a signal handler writes it too. `thread` is the platform's own id.
pub(super) fn write_header(
    out: &mut impl std::fmt::Write,
    thread: u64,
    is_main: bool,
) -> std::fmt::Result {
    let secs = super::now_ms() / 1000;
    write!(
        out,
        "build: moonterminal={} release_base={} moonui={} · uptime {}h{:02}m{:02}s · thread=0x{thread:X} ({})",
        option_env!("MOONTERMINAL_GIT_REV").unwrap_or("unknown"),
        option_env!("MOONTERMINAL_RELEASE_BASE").unwrap_or("unknown"),
        option_env!("MOONUI_GIT_REV").unwrap_or("unknown"),
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        if is_main { "main" } else { "NOT main" }
    )
}

/// Install the native crash handler: the top-level exception filter on Windows, the signal
/// handler on macOS and Linux.
///
/// Call this once during startup after establishing the working directory and before creating
/// windows, on the main thread: the report says whether the fault was on the installing thread.
/// `SetUnhandledExceptionFilter` replaces the filter currently registered for all existing and
/// future process threads; a later registration can replace this one.
///
/// Returns:
///     Nothing.
pub fn install_native_handler() {
    // Start the process clock here so every uptime is measured from the same moment.
    let _ = super::now_ms();
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        context::remember_main_thread();
        SetUnhandledExceptionFilter(Some(native_exception_filter));
    }
    #[cfg(unix)]
    posix::install();
}

/// Arms a deliberate access violation when `MOON_CRASH_PROBE=native[-thread][:seconds]`
/// is set in the environment — the only way to read the whole report without waiting for a real
/// crash.
///
/// `native` faults from a foreground task, which GPUI runs from inside its platform window's
/// procedure, so the report has the same shape as a fault under `CallWindowProcW`: the message
/// ring shows the dispatch message in flight and the stack passes through user32. `native-thread`
/// faults from a plain spawned thread instead — the report must then say `NOT main` and read the
/// ring from a thread that never wrote it. Default delay is five seconds; the environment variable
/// is the whole switch, nothing in any config file.
///
/// Args:
///     cx: The application, for the foreground executor.
///
/// Returns:
///     Nothing. No-op unless the variable is set.
pub(crate) fn arm_probe_if_requested(cx: &mut gpui::App) {
    let Ok(spec) = std::env::var("MOON_CRASH_PROBE") else {
        return;
    };
    let Some(rest) = spec.strip_prefix("native") else {
        log::warn!("MOON_CRASH_PROBE={spec}: only `native[-thread][:seconds]` is understood");
        return;
    };
    let (on_thread, rest) = match rest.strip_prefix("-thread") {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let secs: u64 = rest
        .strip_prefix(':')
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let delay = std::time::Duration::from_secs(secs);
    log::warn!(
        "crash probe armed: a native access violation in {secs} s on {}",
        if on_thread {
            "a spawned thread"
        } else {
            "the main thread"
        }
    );
    if on_thread {
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            fault();
        });
        return;
    }
    cx.spawn(async move |cx| {
        let executor = cx.update(|cx| cx.background_executor().clone());
        executor.timer(delay).await;
        fault();
    })
    .detach();
}

/// A read of the address the v0.42.1 report faulted on — small, unmapped, and not a constant the
/// compiler can fold away.
fn fault() {
    let bogus = std::hint::black_box(0xA8C40usize) as *const u8;
    let _ = unsafe { std::ptr::read_volatile(bogus) };
}

/// Log the first unhandled native exception observed by this filter and return control to normal
/// Windows exception processing.
///
/// The first entry writes the full report. A later entry writes one line; when it comes from
/// another thread it then waits for the first report to finish, since returning would let Windows
/// end the process mid-report. A nested entry on the SAME thread (a fault inside the handler)
/// cannot wait — the report it would wait for is the one it interrupted — and returns at once.
/// Entries past the fourth write nothing.
///
/// Args:
///     info: Exception record and saved processor context supplied as a non-null pointer by the
///         Windows callback contract. The implementation accepts null defensively.
///
/// Returns:
///     `EXCEPTION_CONTINUE_SEARCH`, allowing `UnhandledExceptionFilter` to continue its normal path.
///
/// Safety:
///     Any non-null pointer must reference a valid `EXCEPTION_POINTERS` layout for this call.
#[cfg(windows)]
unsafe extern "system" fn native_exception_filter(
    info: *const windows::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS,
) -> i32 {
    use moon_core::applog::panic_log;

    // `EXCEPTION_CONTINUE_SEARCH` resumes normal `UnhandledExceptionFilter` processing after the
    // log instead of attempting to continue execution at the faulting instruction.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_ACCESS_VIOLATION: u32 = 0xC0000005;

    // Entries so far. The first writes the whole report; a later one — another thread faulting
    // while this report is still being written, which the module enumeration and the dump make a
    // window of real length — writes only its own code and address, so it is not lost. The cap
    // stops a fault inside the handler itself from turning into a loop of entries.
    static ENTRIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    // Thread of entry 0, and whether its report is complete — what a later entry waits on.
    static FIRST_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static FIRST_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    const MAX_ENTRIES: u32 = 4;
    // Longest a second thread holds the process open for the first report; the dump is the slow
    // part and takes seconds, not minutes.
    const FIRST_REPORT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    let entry = ENTRIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if entry >= MAX_ENTRIES {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let this_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    if entry == 0 {
        FIRST_THREAD.store(this_thread, std::sync::atomic::Ordering::SeqCst);
    }

    // Read once so a bad pointer costs nothing below this line.
    let info_ref = unsafe { info.as_ref() };
    let record = info_ref.and_then(|i| unsafe { i.ExceptionRecord.as_ref() });
    let ctx = info_ref.and_then(|i| unsafe { i.ContextRecord.as_ref() });

    let mut body = String::new();
    let mut rip = None;
    if info_ref.is_none() {
        body.push_str("<no exception pointers>");
    } else if let Some(rec) = record {
        let code = rec.ExceptionCode.0 as u32;
        let addr = rec.ExceptionAddress as usize;
        rip = Some(addr);
        body.push_str(&format!("code=0x{code:08X} at instruction 0x{addr:016X}"));
        // For an access violation, the first two parameters are the operation kind (zero for
        // read, one for write, or eight for DEP execution) and the inaccessible virtual address.
        if code == STATUS_ACCESS_VIOLATION && rec.NumberParameters >= 2 {
            let kind = match rec.ExceptionInformation[0] {
                0 => "read",
                1 => "write",
                8 => "execute(DEP)",
                _ => "?",
            };
            let fault = rec.ExceptionInformation[1];
            body.push_str(&format!(" — access violation ({kind}) at 0x{fault:016X}"));
        }
    } else {
        body.push_str("<no exception record>");
    }

    // WHAT faulted, written before anything else is attempted. Symbolizing goes through
    // `dbghelp`, which is not thread-safe and can fault while handling a fault; a nested entry
    // records one line and returns, and the process dies with the rest of this report unwritten.
    // Written first, the crash's own code and address survive that.
    //
    // Writes the file directly, without the global logger: the faulting thread may already hold its
    // lock. Same sink as the Rust panic hook, which is why it lives in `applog`.
    if entry > 0 {
        panic_log(&format!(
            "NATIVE CRASH (entry {entry}, while the first report was being written): {body} · {}",
            context::header()
        ));
        if this_thread != FIRST_THREAD.load(std::sync::atomic::Ordering::SeqCst) {
            let started = std::time::Instant::now();
            while !FIRST_DONE.load(std::sync::atomic::Ordering::SeqCst)
                && started.elapsed() < FIRST_REPORT_WAIT
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        return EXCEPTION_CONTINUE_SEARCH;
    }
    panic_log(&format!("NATIVE CRASH: {body}"));

    // Everything `context` reads is a kernel32/user32 call with no lock of ours involved;
    // cheapest and most telling first.
    panic_log(&format!("  {}", context::header()));
    if let Some(rip) = rip {
        panic_log(&format!("  {}", context::location(rip)));
    }
    if let Some(ctx) = ctx {
        panic_log(&format!("  {}", context::registers(ctx)));
    }
    panic_log(&format!("  {}", context::process_state()));
    panic_log(&format!("  {}", context::foreign_modules()));
    panic_log(&format!(
        "--- stack (module+rva, handler frames first) ---\n{}--- end ---",
        context::stack()
    ));
    panic_log(&format!(
        "--- last window messages (newest first) ---\n{}--- end ---",
        super::msg_ring::report(super::now_ms())
    ));

    // The filter runs on the thread that faulted, and `force_capture` walks its current stack while
    // the handler is executing. It does not unwind from the saved `ContextRecord`; in a dev build
    // the captured frames symbolize the same way as the panic hook's, in release they print as
    // `<unknown>`.
    let bt = std::backtrace::Backtrace::force_capture();
    panic_log(&format!("--- backtrace ---\n{bt}\n--- end ---"));

    // Last: the one step that writes a second file and goes through `dbghelp` for real.
    match minidump::write(info) {
        Ok((path, kib)) => panic_log(&format!("minidump: {} ({kib} KiB)", path.display())),
        Err(e) => panic_log(&format!("minidump: not written — {e}")),
    }

    FIRST_DONE.store(true, std::sync::atomic::Ordering::SeqCst);
    EXCEPTION_CONTINUE_SEARCH
}
