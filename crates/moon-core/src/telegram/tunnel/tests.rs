//! Windows-only regression tests for tunnel child shutdown.

#[cfg(windows)]
use std::{env, thread, time::Duration};

#[cfg(windows)]
use std::process::{Command, Stdio};

#[cfg(windows)]
use super::*;

/// `telegram/tunnel.rs:TunnelProcess::stop` must reap its exact child; removing the unique
/// `reap_running_child` call leaks the helper process and leaves a Mini App tunnel running after
/// the user turns it off.
#[cfg(windows)]
#[test]
fn stop_reaps_the_exact_redirected_fixture_pid() {
    let current_test_exe = env::current_exe().expect("the test executable path must resolve");
    let mut command = Command::new(current_test_exe);
    command
        .args([
            "--exact",
            "telegram::tunnel::tests::fixture_child_closes_streams_and_waits",
            "--nocapture",
        ])
        .env("MOON_CORE_TUNNEL_TEST_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tunnel = TunnelProcess::spawn_command(command)
        .expect("the harmless redirected fixture command must spawn");
    let pid = tunnel
        .child
        .as_ref()
        .expect("the tunnel owner must retain the fixture child")
        .id();
    let cleanup = ExactPidCleanup { pid };

    assert!(
        fixture_pid_is_running(pid),
        "the fixture must still be alive before stop"
    );
    tunnel.stop();
    let still_running_after_stop = fixture_pid_is_running(pid);
    drop(tunnel);
    drop(cleanup);

    assert!(
        !still_running_after_stop,
        "TunnelProcess::stop must kill and reap the exact fixture PID"
    );
}

/// Close inherited readers in the owned fixture child before its harmless wait.
#[cfg(windows)]
#[test]
fn fixture_child_closes_streams_and_waits() {
    if env::var_os("MOON_CORE_TUNNEL_TEST_CHILD").is_none() {
        return;
    }
    unsafe {
        close_inherited_standard_handle(STD_OUTPUT_HANDLE);
        close_inherited_standard_handle(STD_ERROR_HANDLE);
    }
    thread::sleep(Duration::from_secs(60));
}

#[cfg(windows)]
const STD_OUTPUT_HANDLE: u32 = -11_i32 as u32;
#[cfg(windows)]
const STD_ERROR_HANDLE: u32 = -12_i32 as u32;

#[cfg(windows)]
#[link(name = "Kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> isize;
    fn CloseHandle(handle: isize) -> i32;
}

/// Close one raw inherited output handle so a parent reader reaches EOF before the long wait.
#[cfg(windows)]
unsafe fn close_inherited_standard_handle(which: u32) {
    let handle = unsafe { GetStdHandle(which) };
    if handle != 0 && handle != -1 {
        let _ = unsafe { CloseHandle(handle) };
    }
}

#[cfg(windows)]
struct ExactPidCleanup {
    pid: u32,
}

#[cfg(windows)]
impl Drop for ExactPidCleanup {
    /// Terminate only the fixture PID if a mutation deliberately leaked it.
    fn drop(&mut self) {
        if fixture_pid_is_running(self.pid) {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/F"])
                .status();
        }
    }
}

#[cfg(windows)]
fn fixture_pid_is_running(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("Windows tasklist must inspect the owned fixture PID");
    String::from_utf8_lossy(&output.stdout).contains(&format!(",\"{pid}\","))
}
