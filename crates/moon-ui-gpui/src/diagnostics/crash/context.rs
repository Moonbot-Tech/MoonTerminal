//! What the crash filter can learn about the faulting process WITHOUT `dbghelp`: where the
//! instruction sits inside our own image, what the registers held, which thread it was, how much
//! the process had grown, which foreign DLLs share its address space, and the raw return addresses
//! of the stack.
//!
//! Every probe here is a plain kernel32/user32 call that is safe to make while an exception is
//! being handled. `dbghelp` is not — it takes a process-wide lock and has faulted inside this very
//! handler before — so the symbolised backtrace stays LAST in the report and nothing here depends
//! on it. A stripped release build prints `<unknown>` for every frame of that backtrace anyway;
//! the `module+rva` form printed here is an offset into our own image — the same offset in the
//! same build is the same code, which is what lets two reports be compared, and what a
//! disassembler of the shipped exe reads. No symbols are built or kept for it: the decision was
//! that the report must stand without them.
//!
//! `panic.log` masks network addresses before writing; hexadecimal values pass through untouched.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, RtlCaptureStackBackTrace};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::System::ProcessStatus::{
    K32EnumProcessModules, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
};
use windows::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_GDIOBJECTS_PEAK, GR_USEROBJECTS, GR_USEROBJECTS_PEAK, GetCurrentProcess,
    GetCurrentThreadId, GetGuiResources, GetProcessHandleCount,
};
use windows::core::PCWSTR;

/// Return addresses the stack line captures. Sixty-two is the count the oldest documented
/// `RtlCaptureStackBackTrace` accepted, and the frames of interest — filter, dispatch, the fault
/// and its callers up to the message loop — sit within the first thirty.
const STACK_FRAMES: usize = 62;

/// Modules the enumeration buffer holds; the terminal loads about eighty.
const MODULE_SLOTS: usize = 512;

/// Id of the thread that installed the handler — the UI thread. Zero until then.
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

/// Records the calling thread as the main one; the report then says whether the fault was on it.
pub(super) fn remember_main_thread() {
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
}

/// `build: … · uptime … · thread=…` — which binary, how long it ran, which thread died.
pub(super) fn header() -> String {
    let thread = unsafe { GetCurrentThreadId() };
    let main = MAIN_THREAD.load(Ordering::Relaxed);
    let mut out = String::new();
    let _ = super::write_header(&mut out, u64::from(thread), main != 0 && thread == main);
    out
}

/// The faulting instruction as `module+rva` — an offset into the image, stable across runs of
/// the same build — next to the load base that turns any other raw address in the same report
/// into an offset.
pub(super) fn location(rip: usize) -> String {
    match module_of(rip) {
        Some((name, base)) => format!("at {name}+0x{:X} (base 0x{base:016X})", rip - base),
        None => "at an address outside every loaded module".to_string(),
    }
}

/// General-purpose registers from the saved context. The one holding the inaccessible address
/// says whether the fault was `[reg]` or `[reg+disp]`, which the address alone cannot.
#[cfg(target_arch = "x86_64")]
pub(super) fn registers(ctx: &CONTEXT) -> String {
    format!(
        "regs: rip={:016X} rsp={:016X} rbp={:016X} rax={:016X} rbx={:016X} rcx={:016X} \
         rdx={:016X} rsi={:016X} rdi={:016X} r8={:016X} r9={:016X} r10={:016X} r11={:016X} \
         r12={:016X} r13={:016X} r14={:016X} r15={:016X}",
        ctx.Rip,
        ctx.Rsp,
        ctx.Rbp,
        ctx.Rax,
        ctx.Rbx,
        ctx.Rcx,
        ctx.Rdx,
        ctx.Rsi,
        ctx.Rdi,
        ctx.R8,
        ctx.R9,
        ctx.R10,
        ctx.R11,
        ctx.R12,
        ctx.R13,
        ctx.R14,
        ctx.R15
    )
}

#[cfg(not(target_arch = "x86_64"))]
pub(super) fn registers(_ctx: &CONTEXT) -> String {
    "regs: not printed on this architecture".to_string()
}

/// Memory, handle and GUI-object counts of the process at the moment of the fault.
///
/// USER and GDI objects are the two with a hard ceiling — 10,000 each per process — after which
/// `CreateWindow` and `LoadCursor` return null. A crash "after an hour or three" with a count near
/// that ceiling is a leak, not a race.
pub(super) fn process_state() -> String {
    let process = unsafe { GetCurrentProcess() };
    let mut mem = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    let mem_ok = unsafe { K32GetProcessMemoryInfo(process, &mut mem, mem.cb) }.as_bool();
    let mut handles = 0u32;
    let handles_ok = unsafe { GetProcessHandleCount(process, &mut handles) }.is_ok();
    let gui = |flag| unsafe { GetGuiResources(process, flag) };
    let mut out = String::from("process:");
    if mem_ok {
        let mib = |b: usize| b / (1024 * 1024);
        let _ = write!(
            out,
            " working_set={}MiB (peak {}MiB) commit={}MiB",
            mib(mem.WorkingSetSize),
            mib(mem.PeakWorkingSetSize),
            mib(mem.PagefileUsage)
        );
    } else {
        out.push_str(" working_set=? ");
    }
    if handles_ok {
        let _ = write!(out, " handles={handles}");
    } else {
        out.push_str(" handles=?");
    }
    let _ = write!(
        out,
        " user_objects={} (peak {}) gdi_objects={} (peak {}) — limit 10000 each",
        gui(GR_USEROBJECTS),
        gui(GR_USEROBJECTS_PEAK),
        gui(GR_GDIOBJECTS),
        gui(GR_GDIOBJECTS_PEAK)
    );
    out
}

/// Every loaded module that is neither our executable nor under the Windows directory: injected
/// hooks (antivirus, input switchers, overlays, remote-desktop tools) are the classic source of a
/// fault inside a window procedure that the program's own code cannot explain.
pub(super) fn foreign_modules() -> String {
    let process = unsafe { GetCurrentProcess() };
    let mut handles = [HMODULE::default(); MODULE_SLOTS];
    let cb = (handles.len() * std::mem::size_of::<HMODULE>()) as u32;
    let mut needed = 0u32;
    let ok = unsafe { K32EnumProcessModules(process, handles.as_mut_ptr(), cb, &mut needed) };
    if !ok.as_bool() {
        return "modules: enumeration failed".to_string();
    }
    let count = (needed as usize / std::mem::size_of::<HMODULE>()).min(handles.len());
    let system_root = std::env::var_os("SystemRoot")
        .map(|r| r.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "c:\\windows".to_string());
    let mut foreign = Vec::new();
    let mut system = 0usize;
    // The first entry is the executable itself.
    for h in handles.iter().take(count).skip(1) {
        let Some(path) = module_path(*h) else {
            continue;
        };
        if path.to_lowercase().starts_with(&system_root) {
            system += 1;
        } else {
            foreign.push(path);
        }
    }
    let truncated = if needed as usize > cb as usize {
        " (list truncated)"
    } else {
        ""
    };
    if foreign.is_empty() {
        format!("modules: none outside the Windows directory; {system} system modules{truncated}")
    } else {
        format!(
            "modules outside the Windows directory ({system} system modules omitted{truncated}): {}",
            foreign.join(", ")
        )
    }
}

/// The stack of the faulting thread as it is while the filter runs: the filter's own frames and
/// the exception dispatch first, then the frames that faulted. Each address prints as
/// `module+rva` when a module claims it, otherwise raw — a raw address is itself a finding
/// (JIT-less process; only injected or unloaded code lives outside every module).
pub(super) fn stack() -> String {
    let mut frames = [std::ptr::null_mut(); STACK_FRAMES];
    let captured = unsafe { RtlCaptureStackBackTrace(0, &mut frames, None) } as usize;
    let mut out = String::new();
    for (i, addr) in frames.iter().take(captured).enumerate() {
        let addr = *addr as usize;
        match module_of(addr) {
            Some((name, base)) => {
                let _ = writeln!(out, "  {i:>2}: {name}+0x{:X}", addr - base);
            }
            None => {
                let _ = writeln!(out, "  {i:>2}: 0x{addr:016X} (no module)");
            }
        }
    }
    if captured == 0 {
        out.push_str("  (no frames captured)\n");
    }
    out
}

/// The module containing `addr` as `(file stem, load base)`.
fn module_of(addr: usize) -> Option<(String, usize)> {
    let mut handle = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(addr as *const u16),
            &mut handle,
        )
    }
    .ok()?;
    let path = module_path(handle)?;
    let stem = std::path::Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or(path);
    Some((stem, handle.0 as usize))
}

/// Full path of a loaded module, or `None` when Windows cannot name it.
fn module_path(handle: HMODULE) -> Option<String> {
    let mut buf = [0u16; 1024];
    let len = unsafe { GetModuleFileNameW(Some(handle), &mut buf) } as usize;
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len.min(buf.len())]))
}
