//! The last window messages the main thread dispatched, kept in memory and written out only by
//! the crash handler.
//!
//! A native crash inside a window procedure names the faulting instruction but not the message
//! that led there, and a stripped release build cannot name the handler either. The one report this
//! was written for (v0.42.1, an access violation six frames under `CallWindowProcW`, once every
//! one to three hours) could not be told apart from three different causes for exactly that
//! reason. Whether the message in flight was `WM_GETOBJECT` (UI Automation), `WM_USER+7` (GPUI's
//! device-lost recovery) or `WM_DISPLAYCHANGE` is the first question, and this ring answers it.
//!
//! Two thread-local hooks on the main thread see every message: `WH_CALLWNDPROC` runs before the
//! window procedure for a SENT message, `WH_GETMESSAGE` when a POSTED one is retrieved, just
//! before it is dispatched. Both fire in-process on the thread that owns the window, so there is no
//! context switch; the whole cost per message is one store into a fixed ring. Nothing is written
//! to disk while the process is healthy — the ring lives in [`CAPACITY`] slots of atomics and
//! the crash filter reads it from whatever thread faulted.
//!
//! What it cannot promise: the reader takes no lock (the faulting thread may be the writer), so
//! the newest slot can be half-written when another thread crashes mid-store. That is a torn
//! diagnostic line, not a fault, and it is the price of a handler that never waits.

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::now_ms;

/// Messages the ring holds. GPUI's frame clock and task dispatch alone post about two hundred a
/// second, a mouse storm many more, so this is a second or two of history at rest and a fraction
/// of one under load. The crash report prints the newest [`REPORT_LINES`] verbatim and then every
/// older message in the ring that is not [routine](is_routine), so a rare trigger that the frame
/// clock has already pushed past the first lines still appears.
pub(crate) const CAPACITY: usize = 256;

/// Lines the crash report takes from the ring verbatim — enough to see the message in flight and
/// the sequence that preceded it without turning the report into a message trace.
pub(crate) const REPORT_LINES: usize = 32;

/// One recorded message. `hwnd`, `wparam` and `lparam` are kept as raw integers: the report prints
/// them, it never dereferences them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    /// Milliseconds since [`now_ms`]'s origin.
    pub(crate) at_ms: u64,
    pub(crate) hwnd: usize,
    pub(crate) msg: u32,
    pub(crate) wparam: usize,
    pub(crate) lparam: isize,
}

struct Slot {
    at_ms: AtomicU64,
    hwnd: AtomicUsize,
    msg: AtomicU32,
    wparam: AtomicUsize,
    lparam: AtomicIsize,
}

impl Slot {
    const fn new() -> Self {
        Self {
            at_ms: AtomicU64::new(0),
            hwnd: AtomicUsize::new(0),
            msg: AtomicU32::new(0),
            wparam: AtomicUsize::new(0),
            lparam: AtomicIsize::new(0),
        }
    }
}

/// A fixed ring of the last `N` records, written by one thread and readable from any.
pub(crate) struct Ring<const N: usize> {
    slots: [Slot; N],
    /// Records ever written; the slot of record `i` is `i % N`.
    written: AtomicUsize,
}

impl<const N: usize> Ring<N> {
    pub(crate) const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; N],
            written: AtomicUsize::new(0),
        }
    }

    /// Stores one record. Single writer: the fields go in first and the count is published after
    /// them, so a reader that observes the count sees the slot it points past.
    pub(crate) fn record(&self, r: Record) {
        let i = self.written.load(Ordering::Relaxed);
        let slot = &self.slots[i % N];
        slot.at_ms.store(r.at_ms, Ordering::Relaxed);
        slot.hwnd.store(r.hwnd, Ordering::Relaxed);
        slot.msg.store(r.msg, Ordering::Relaxed);
        slot.wparam.store(r.wparam, Ordering::Relaxed);
        slot.lparam.store(r.lparam, Ordering::Relaxed);
        self.written.store(i + 1, Ordering::Release);
    }

    /// The newest `limit` records, newest first. Fewer when the ring holds fewer.
    pub(crate) fn newest(&self, limit: usize) -> Vec<Record> {
        let written = self.written.load(Ordering::Acquire);
        let available = written.min(N).min(limit);
        (0..available)
            .map(|back| {
                let slot = &self.slots[(written - 1 - back) % N];
                Record {
                    at_ms: slot.at_ms.load(Ordering::Relaxed),
                    hwnd: slot.hwnd.load(Ordering::Relaxed),
                    msg: slot.msg.load(Ordering::Relaxed),
                    wparam: slot.wparam.load(Ordering::Relaxed),
                    lparam: slot.lparam.load(Ordering::Relaxed),
                }
            })
            .collect()
    }
}

static RING: Ring<CAPACITY> = Ring::new();

/// Records one message into the process-wide ring.
pub(crate) fn record(hwnd: usize, msg: u32, wparam: usize, lparam: isize) {
    RING.record(Record {
        at_ms: now_ms(),
        hwnd,
        msg,
        wparam,
        lparam,
    });
}

/// The crash report's view of the ring: the newest [`REPORT_LINES`] messages verbatim, then the
/// older non-routine ones, each with its age relative to `now_ms`.
pub(crate) fn report(now_ms: u64) -> String {
    report_from(&RING.newest(CAPACITY), now_ms)
}

/// [`report`] over an explicit newest-first list; the split it prints is what the tests check.
pub(crate) fn report_from(newest_first: &[Record], now_ms: u64) -> String {
    let (recent, older) = newest_first.split_at(newest_first.len().min(REPORT_LINES));
    let mut out = format(recent, now_ms);
    let notable: Vec<Record> = older
        .iter()
        .copied()
        .filter(|r| !is_routine(r.msg))
        .collect();
    if !notable.is_empty() {
        out.push_str("  … older, routine messages omitted:\n");
        out.push_str(&format(&notable, now_ms));
    }
    out
}

/// Messages the terminal receives continuously — GPUI's per-frame and per-task posts, mouse
/// motion and its hit-testing, paint and timers. They are what fills the ring between two
/// events worth reading, and the only ones the report leaves out of its older section.
pub(crate) fn is_routine(msg: u32) -> bool {
    matches!(
        msg,
        0x000F | 0x0020 | 0x0084 | 0x00A0 | 0x0113 | 0x0200 | 0x0403 | 0x0405 | 0x0409
    )
}

/// Formats records newest first as `-<age>ms hwnd=0x… msg=0x… NAME w=0x… l=0x…`.
///
/// The age is relative to `now_ms` so the line reads as "this long before the crash"; a message
/// recorded after `now_ms` (a torn read racing the writer) prints as `-0ms` rather than as a
/// negative age.
pub(crate) fn format(records: &[Record], now_ms: u64) -> String {
    let mut out = String::new();
    if records.is_empty() {
        out.push_str("  (no messages recorded)\n");
        return out;
    }
    for r in records {
        let age = now_ms.saturating_sub(r.at_ms);
        let name = message_name(r.msg).unwrap_or("");
        let _ = writeln!(
            out,
            "  -{age}ms hwnd=0x{:X} msg=0x{:04X} {name:<18} w=0x{:X} l=0x{:X}",
            r.hwnd, r.msg, r.wparam, r.lparam
        );
    }
    out
}

/// Names for the messages a crash report is likely to turn on. Everything else prints as its
/// number: the list is a reading aid, not a registry, and a full table would be a thousand lines
/// nobody maintains. `WM_USER+n` covers GPUI's own messages (`WM_USER+7` is device-lost recovery).
pub(crate) fn message_name(msg: u32) -> Option<&'static str> {
    const WM_USER: u32 = 0x0400;
    const WM_APP: u32 = 0x8000;
    Some(match msg {
        0x0001 => "WM_CREATE",
        0x0002 => "WM_DESTROY",
        0x0005 => "WM_SIZE",
        0x0006 => "WM_ACTIVATE",
        0x0007 => "WM_SETFOCUS",
        0x0008 => "WM_KILLFOCUS",
        0x000F => "WM_PAINT",
        0x0010 => "WM_CLOSE",
        0x0012 => "WM_QUIT",
        0x001A => "WM_SETTINGCHANGE",
        0x001C => "WM_ACTIVATEAPP",
        0x0020 => "WM_SETCURSOR",
        0x0024 => "WM_GETMINMAXINFO",
        0x003D => "WM_GETOBJECT",
        0x0046 => "WM_WINDOWPOSCHANGING",
        0x0047 => "WM_WINDOWPOSCHANGED",
        0x0051 => "WM_INPUTLANGCHANGE",
        0x007E => "WM_DISPLAYCHANGE",
        0x0082 => "WM_NCDESTROY",
        0x0083 => "WM_NCCALCSIZE",
        0x0084 => "WM_NCHITTEST",
        0x0086 => "WM_NCACTIVATE",
        0x00A0 => "WM_NCMOUSEMOVE",
        0x00FF => "WM_INPUT",
        0x0100 => "WM_KEYDOWN",
        0x0101 => "WM_KEYUP",
        0x0102 => "WM_CHAR",
        0x0104 => "WM_SYSKEYDOWN",
        0x0113 => "WM_TIMER",
        0x0200 => "WM_MOUSEMOVE",
        0x0201 => "WM_LBUTTONDOWN",
        0x0202 => "WM_LBUTTONUP",
        0x0204 => "WM_RBUTTONDOWN",
        0x0205 => "WM_RBUTTONUP",
        0x020A => "WM_MOUSEWHEEL",
        0x0218 => "WM_POWERBROADCAST",
        0x0219 => "WM_DEVICECHANGE",
        0x02A3 => "WM_MOUSELEAVE",
        0x02E0 => "WM_DPICHANGED",
        0x031E => "WM_DWMCOMPOSITIONCHANGED",
        0x0401 => "WM_USER+1",
        0x0402 => "WM_USER+2",
        0x0403 => "WM_USER+3",
        0x0404 => "WM_USER+4",
        0x0405 => "WM_USER+5",
        0x0406 => "WM_USER+6",
        0x0407 => "WM_USER+7",
        0x0408 => "WM_USER+8",
        0x0409 => "WM_USER+9",
        m if (WM_USER..WM_APP).contains(&m) => "WM_USER+n",
        m if m >= WM_APP => "WM_APP+n",
        _ => return None,
    })
}

/// Installs the two hooks on the calling thread. Call once, on the main thread, before any window
/// exists; the hooks live as long as the process, so their handles are deliberately kept nowhere.
///
/// Returns:
///     The Windows error when a hook could not be set. A hook that did go in stays: a ring of
///     sent messages alone is worth more than none, and the report shows whatever was recorded.
#[cfg(windows)]
pub(crate) fn install() -> windows::core::Result<()> {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowsHookExW, WH_CALLWNDPROC, WH_GETMESSAGE,
    };

    // Start the clock now so the first recorded message already has a meaningful age.
    let _ = now_ms();
    let thread = unsafe { GetCurrentThreadId() };
    unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(call_wnd_proc_hook), None, thread) }?;
    unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(get_message_hook), None, thread) }?;
    Ok(())
}

/// `WH_CALLWNDPROC`: a sent message, about to reach its window procedure.
#[cfg(windows)]
unsafe extern "system" fn call_wnd_proc_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CWPSTRUCT, CallNextHookEx, HC_ACTION};

    // Below `HC_ACTION` the hook must pass the call on without looking at it.
    if code == HC_ACTION as i32
        && let Some(m) = unsafe { (lparam.0 as *const CWPSTRUCT).as_ref() }
    {
        record(m.hwnd.0 as usize, m.message, m.wParam.0, m.lParam.0);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// `WH_GETMESSAGE`: a posted message, just retrieved by the message loop. Only the `PM_REMOVE`
/// retrieval counts — a `PM_NOREMOVE` peek would record the same message twice.
#[cfg(windows)]
unsafe extern "system" fn get_message_hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, HC_ACTION, MSG, PM_REMOVE};

    if code == HC_ACTION as i32
        && wparam.0 as u32 == PM_REMOVE.0
        && let Some(m) = unsafe { (lparam.0 as *const MSG).as_ref() }
    {
        record(m.hwnd.0 as usize, m.message, m.wParam.0, m.lParam.0);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
