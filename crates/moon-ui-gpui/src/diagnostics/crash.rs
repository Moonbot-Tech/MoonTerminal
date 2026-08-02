//! Native crash logging for Windows Structured Exception Handling (SEH), complementing the Rust
//! panic hook installed during startup. A panic hook handles Rust panics but not native faults such
//! as access violations in DirectX or GPUI code. On Windows, this module installs a process-wide
//! top-level exception filter. In a process without a debugger, the filter runs when an exception
//! remains unhandled and reaches `UnhandledExceptionFilter`; it appends the exception code and
//! address plus a handler-time backtrace to the same `panic.log` used by the panic hook.
//!
//! The backtrace is captured while the filter executes on the faulting thread. It is not rebuilt
//! from the saved processor context in `EXCEPTION_POINTERS`, so the exception record is the precise
//! source of the fault code and instruction address.

/// Install the process-wide top-level exception filter on Windows.
///
/// Call this once during startup after establishing the working directory and before creating
/// windows. `SetUnhandledExceptionFilter` replaces the filter currently registered for all existing
/// and future process threads; a later registration can replace this one. This is a no-op on other
/// platforms.
///
/// Returns:
///     Nothing.
pub fn install_native_handler() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::SetUnhandledExceptionFilter;
        SetUnhandledExceptionFilter(Some(native_exception_filter));
    }
}

/// Log the first unhandled native exception observed by this filter and return control to normal
/// Windows exception processing.
///
/// A permanent one-shot guard makes nested and later invocations return without logging.
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
    // `EXCEPTION_CONTINUE_SEARCH` resumes normal `UnhandledExceptionFilter` processing after the
    // log instead of attempting to continue execution at the faulting instruction.
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_ACCESS_VIOLATION: u32 = 0xC0000005;

    // This one-shot reentrancy guard remains set after the first entry. Nested or repeated filter
    // invocations therefore skip logging instead of recursing if logging itself faults.
    static IN_HANDLER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if IN_HANDLER.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    let mut body = String::new();
    if let Some(info) = unsafe { info.as_ref() } {
        if let Some(rec) = unsafe { info.ExceptionRecord.as_ref() } {
            let code = rec.ExceptionCode.0 as u32;
            let addr = rec.ExceptionAddress as usize;
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
    } else {
        body.push_str("<no exception pointers>");
    }

    // The filter runs on the thread that faulted, and `force_capture` walks its current stack while
    // the handler is executing. It does not unwind from the saved `ContextRecord`; available PDBs
    // symbolize the captured frames in the same way as the panic hook.
    let bt = std::backtrace::Backtrace::force_capture();

    // Writes the file directly, without the global logger: the faulting thread may already hold its
    // lock. Same sink as the Rust panic hook, which is why it lives in `applog`.
    moon_core::applog::panic_log(&format!(
        "NATIVE CRASH: {body}\n--- backtrace ---\n{bt}\n--- end ---"
    ));

    EXCEPTION_CONTINUE_SEARCH
}
