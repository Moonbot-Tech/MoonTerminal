//! Name the threads a library spawns for us, without touching the library.
//!
//! A Linux thread inherits its creator's name (`comm`) at creation. moonproto names none of the
//! threads a client runs — reader, writer, history worker — so they all read as the core's
//! `feed-N`, and neither `top` nor the station's heap accounting can tell the feed thread from
//! the client it drives (#1081). Renaming the calling thread for the duration of the call that
//! builds the client gives every thread spawned inside it, and every thread those spawn later,
//! the name passed here.

/// Run `f` with the calling thread renamed to `name`, then restore its own name.
///
/// Threads created inside `f` keep `name` for life. Linux only; elsewhere `f` simply runs.
///
/// Args:
///     name: At most 15 bytes are kept (the kernel's `comm` limit).
///     f: The call that spawns the threads to be named.
///
/// Returns:
///     Whatever `f` returns.
pub fn spawning_as<T>(name: &str, f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "linux")]
    {
        // Restored on drop, so a panic inside `f` does not leave the caller wearing `name`.
        struct Restore(Vec<u8>);
        impl Drop for Restore {
            fn drop(&mut self) {
                set(&self.0);
            }
        }
        let _restore = Restore(current());
        set(name.as_bytes());
        f()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = name;
        f()
    }
}

/// The calling thread's name, without its terminating NUL.
#[cfg(target_os = "linux")]
fn current() -> Vec<u8> {
    let mut buf = [0u8; 16];
    // SAFETY: PR_GET_NAME writes at most 16 bytes, NUL included, into a buffer of that size.
    unsafe {
        libc::prctl(
            libc::PR_GET_NAME,
            buf.as_mut_ptr() as libc::c_ulong,
            0,
            0,
            0,
        )
    };
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    buf[..len].to_vec()
}

/// Rename the calling thread; longer names are cut to the kernel's 15 bytes.
#[cfg(target_os = "linux")]
fn set(name: &[u8]) {
    let mut buf = [0u8; 16];
    let len = name.len().min(15);
    buf[..len].copy_from_slice(&name[..len]);
    // SAFETY: PR_SET_NAME reads a NUL-terminated string of at most 16 bytes from `buf`.
    unsafe { libc::prctl(libc::PR_SET_NAME, buf.as_ptr() as libc::c_ulong, 0, 0, 0) };
}
