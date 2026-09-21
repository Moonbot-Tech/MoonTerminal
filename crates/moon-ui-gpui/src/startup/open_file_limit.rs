//! Raise the process soft `RLIMIT_NOFILE` to the hard limit at startup on Unix.
//!
//! GUI processes on macOS inherit a soft limit of 256 open files while the hard
//! limit is far higher. Report readers spend that budget quickly (a WAL trio of
//! databases is nine descriptors each), so the raise has to happen once, early,
//! before any window or database work. A failed raise is logged and is not fatal:
//! a process that refuses to start because it could not widen a limit is worse
//! than the limit staying narrow.
//!
//! Windows has no `RLIMIT_NOFILE` and is left alone; this module still compiles
//! there so the pure decision can be tested without a Unix kernel.

/// Darwin `OPEN_MAX` from `<sys/syslimits.h>`.
///
/// macOS `setrlimit(RLIMIT_NOFILE)` returns `EINVAL` when the requested soft
/// limit exceeds this value, even if the hard limit is `RLIM_INFINITY`.
const MACOS_OPEN_MAX: u64 = 10_240;

/// The hard limit a `setrlimit` request may actually use as the new soft limit.
///
/// Args:
///     hard: Hard `RLIMIT_NOFILE` reported by `getrlimit`.
///     macos: Whether the kernel is Darwin, which rejects a soft limit above
///         [`MACOS_OPEN_MAX`].
///
/// Returns:
///     `hard`, capped to [`MACOS_OPEN_MAX`] on macOS.
pub(super) fn request_hard(hard: u64, macos: bool) -> u64 {
    if macos {
        hard.min(MACOS_OPEN_MAX)
    } else {
        hard
    }
}

/// The soft `RLIMIT_NOFILE` to install given the current pair.
///
/// A process may raise its soft limit up to the hard limit and must not lower
/// it. When `soft` already equals `hard`, or is already above `hard`, the
/// current soft limit is kept: shrinking the descriptor budget at startup is
/// worse than leaving an unusual pair alone, and the kernel would reject a
/// raise past the hard limit.
///
/// Args:
///     soft: Current soft limit.
///     hard: Hard limit the new soft may reach. The caller applies
///         [`request_hard`] first so a Darwin kernel ceiling is already folded
///         in.
///
/// Returns:
///     The soft limit to pass to `setrlimit`, or the current soft when no
///     raise is possible.
pub(super) fn desired_soft(soft: u64, hard: u64) -> u64 {
    if soft >= hard { soft } else { hard }
}

/// Raise the soft open-file limit to the hard limit. Failure is logged and does
/// not stop startup.
///
/// Call once after the logger is installed and before any database or window
/// work, so the before/hard/after line reaches the Log tab and the wider budget
/// is in force for every later open.
#[cfg(unix)]
pub(super) fn raise_to_hard_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a stack `rlimit` we exclusively own. `getrlimit` only
    // writes those two fields, and `RLIMIT_NOFILE` is a defined resource.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        log::warn!(
            "RLIMIT_NOFILE: getrlimit failed: {}",
            std::io::Error::last_os_error()
        );
        return;
    }

    let before = limit.rlim_cur as u64;
    let hard = limit.rlim_max as u64;
    let want = desired_soft(before, request_hard(hard, cfg!(target_os = "macos")));

    if want != before {
        limit.rlim_cur = want as libc::rlim_t;
        // SAFETY: `limit` is the `rlimit` just read from the kernel, with only
        // `rlim_cur` raised to a value no higher than the capped hard limit.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
            log::warn!(
                "RLIMIT_NOFILE: before {} hard {} after {} (setrlimit failed: {})",
                format_limit(before),
                format_limit(hard),
                format_limit(before),
                std::io::Error::last_os_error()
            );
            return;
        }
    }

    let after = read_soft_limit().unwrap_or(want);
    log::info!(
        "RLIMIT_NOFILE: before {} hard {} after {}",
        format_limit(before),
        format_limit(hard),
        format_limit(after)
    );
}

/// Soft `RLIMIT_NOFILE` as the kernel currently has it, when `getrlimit` works.
#[cfg(unix)]
fn read_soft_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a stack `rlimit` we exclusively own.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } == 0 {
        Some(limit.rlim_cur as u64)
    } else {
        None
    }
}

/// Render a resource limit for the startup log line. `u64::MAX` is how this
/// crate sees `RLIM_INFINITY` on 64-bit Unix.
#[cfg(unix)]
fn format_limit(n: u64) -> String {
    if n == u64::MAX {
        "unlimited".to_string()
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests;
