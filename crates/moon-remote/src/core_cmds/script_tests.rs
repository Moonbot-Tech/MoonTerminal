use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core_move::SCRIPT;

static NEXT_SANDBOX: AtomicU64 = AtomicU64::new(0);

const SYSTEMCTL_STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$MB_TEST_ROOT/systemctl_calls"
case ${1:-} in
list-units)
    cat "$MB_TEST_ROOT/systemctl_list_units"
    ;;
list-unit-files)
    case "$*" in
    *moonbot@*) cat "$MB_TEST_ROOT/systemctl_moonbot_units" ;;
    *) cat "$MB_TEST_ROOT/systemctl_all_units" ;;
    esac
    ;;
show)
    case "$*" in
    *Id,ExecStart,WorkingDirectory,ActiveState*)
        while test $# -gt 0; do
            if test "$1" = --; then
                shift
                break
            fi
            shift
        done
        if test $# = 0; then
            printf 'Version=stub\n'
            exit 0
        fi
        for name in "$@"; do
            printf 'ActiveState=%s\n' "$(cat "$MB_TEST_ROOT/active_state")"
            printf 'ExecStart=%s\n' "$(cat "$MB_TEST_ROOT/exec_start")"
            printf 'WorkingDirectory=%s\n' "$(cat "$MB_TEST_ROOT/working_directory")"
            printf 'Id=%s\n\n' "$name"
        done
        ;;
    *MainPID*) cat "$MB_TEST_ROOT/main_pid" ;;
    *ActiveEnterTimestampMonotonic*) cat "$MB_TEST_ROOT/active_entered" ;;
    *MemoryCurrent*) cat "$MB_TEST_ROOT/memory_current" ;;
    *ActiveState*) cat "$MB_TEST_ROOT/active_state" ;;
    *WorkingDirectory*) cat "$MB_TEST_ROOT/working_directory" ;;
    esac
    ;;
is-active)
    test "$(cat "$MB_TEST_ROOT/active_state")" = active
    ;;
start|restart)
    if test -s "$MB_TEST_ROOT/service_action_fail"; then
        printf 'inactive\n' > "$MB_TEST_ROOT/active_state"
        : > "$MB_TEST_ROOT/ss_uan"
        exit 1
    fi
    printf 'active\n' > "$MB_TEST_ROOT/active_state"
    printf 'UNCONN 0 0 0.0.0.0:3302 0.0.0.0:*\n' > "$MB_TEST_ROOT/ss_uan"
    ;;
stop)
    if test -s "$MB_TEST_ROOT/service_action_fail"; then
        exit 1
    fi
    printf 'inactive\n' > "$MB_TEST_ROOT/active_state"
    : > "$MB_TEST_ROOT/ss_uan"
    ;;
enable|disable) exit 0 ;;
esac
"#;

const SS_STUB: &str = r#"#!/bin/sh
case "$*" in
*-ulnp*) cat "$MB_TEST_ROOT/ss_ulnp" ;;
*) cat "$MB_TEST_ROOT/ss_uan" ;;
esac
"#;

const JOURNALCTL_STUB: &str = r#"#!/bin/sh
cat "$MB_TEST_ROOT/journal"
"#;

const SYSTEMD_ESCAPE_STUB: &str = r#"#!/bin/sh
case "$*" in
*--unescape*) cat "$MB_TEST_ROOT/unescape" ;;
*) printf 'moonbot@root-moonbot-a.service\n' ;;
esac
"#;

const UFW_STUB: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$MB_TEST_ROOT/ufw_calls"
record_order() {
    if test -s "$MB_TEST_ROOT/retarget_marker"; then
        printf '%s|marker=%s\n' "$*" "$(head -n 1 "$(cat "$MB_TEST_ROOT/retarget_marker")")" >> "$MB_TEST_ROOT/retarget_order"
    fi
}
add_allow() {
    position=''
    if test "$1" = insert; then
        position=$2
        shift 2
    fi
    if test "${2:-}" = from; then
        source=$3
        port=$7
    else
        source=any
        port=${2%/udp}
    fi
    comment=$(printf '%s\n' "$*" | sed -n 's/.* comment //p')
    if test "$source" = ::/0; then
        rule="$port/udp (v6) ALLOW IN Anywhere (v6) # $comment"
        match="$port/udp (v6) ALLOW "
    else
        rule="$port/udp ALLOW IN Anywhere # $comment"
        match="$port/udp ALLOW "
    fi
    if grep -q -F "$match" "$MB_TEST_ROOT/ufw_rules"; then
        awk -v match="$match" -v rule="$rule" 'index($0, match) == 1 {$0 = rule} {print}' \
            "$MB_TEST_ROOT/ufw_rules" > "$MB_TEST_ROOT/ufw_rules.kept"
    elif test "$position" = 1; then
        printf '%s\n' "$rule" > "$MB_TEST_ROOT/ufw_rules.kept"
        cat "$MB_TEST_ROOT/ufw_rules" >> "$MB_TEST_ROOT/ufw_rules.kept"
    else
        cat "$MB_TEST_ROOT/ufw_rules" > "$MB_TEST_ROOT/ufw_rules.kept"
        printf '%s\n' "$rule" >> "$MB_TEST_ROOT/ufw_rules.kept"
    fi
    cat "$MB_TEST_ROOT/ufw_rules.kept" > "$MB_TEST_ROOT/ufw_rules"
    rm -f "$MB_TEST_ROOT/ufw_rules.kept"
    record_order "$@"
}
case ${1:-} in
allow|insert)
    add_allow "$@"
    ;;
delete)
    port=$3
    grep -v "^$port " "$MB_TEST_ROOT/ufw_rules" > "$MB_TEST_ROOT/ufw_rules.kept" || true
    cat "$MB_TEST_ROOT/ufw_rules.kept" > "$MB_TEST_ROOT/ufw_rules"
    rm -f "$MB_TEST_ROOT/ufw_rules.kept"
    record_order "$@"
    ;;
--force)
    if test "${2:-}" = delete; then
        awk -v number="$3" 'NR != number' "$MB_TEST_ROOT/ufw_rules" > "$MB_TEST_ROOT/ufw_rules.kept"
        cat "$MB_TEST_ROOT/ufw_rules.kept" > "$MB_TEST_ROOT/ufw_rules"
        rm -f "$MB_TEST_ROOT/ufw_rules.kept"
        record_order "$@"
    fi
    ;;
status)
    if test "$(cat "$MB_TEST_ROOT/ufw_active")" = 1; then
        printf 'Status: active\n'
        if test "${2:-}" = numbered; then
            awk '{printf "[%3d] %s\n", NR, $0}' "$MB_TEST_ROOT/ufw_rules"
        else
            cat "$MB_TEST_ROOT/ufw_rules"
        fi
    else
        printf 'Status: inactive\n'
    fi
    ;;
esac
"#;

const SUDO_STUB: &str = r#"#!/bin/sh
exit 0
"#;

const BASH_ENV: &str = r#"readlink() {
    case ${1:-} in
    */cwd) cat "$MB_TEST_ROOT/readlink_cwd" ;;
    */exe) cat "$MB_TEST_ROOT/readlink_exe" ;;
    *) return 1 ;;
    esac
}
"#;

/// Find Git Bash on Windows before the WSL `bash`, which does not understand `C:/` fixture paths.
fn bash_command() -> Command {
    #[cfg(windows)]
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        let git_bash = PathBuf::from(program_files)
            .join("Git")
            .join("bin")
            .join("bash.exe");
        if git_bash.is_file() {
            return Command::new(git_bash);
        }
    }
    Command::new("bash")
}

/// A self-contained server fixture whose fake tools and proc tree live beside the embedded script.
struct Sandbox {
    root: PathBuf,
    script: PathBuf,
    bin: PathBuf,
    proc: PathBuf,
}

impl Sandbox {
    /// Create an isolated script fixture, or report a skipped test when Bash is unavailable.
    fn new() -> Option<Self> {
        if bash_command().arg("--version").output().is_err() {
            eprintln!("[skip] bash is not on PATH");
            return None;
        }
        let serial = NEXT_SANDBOX.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("moon-core-cmds-{}-{serial}", std::process::id()));
        let bin = root.join("fakebin");
        let proc = root.join("proc");
        fs::create_dir_all(&bin).expect("create fake tool directory");
        fs::create_dir_all(&proc).expect("create fake proc directory");
        let script = root.join("moonbot-move.sh");
        fs::write(&script, SCRIPT).expect("write embedded script");
        let sandbox = Self {
            root,
            script,
            bin,
            proc,
        };
        for (name, body) in [
            ("systemctl", SYSTEMCTL_STUB),
            ("ss", SS_STUB),
            ("journalctl", JOURNALCTL_STUB),
            ("systemd-escape", SYSTEMD_ESCAPE_STUB),
            ("ufw", UFW_STUB),
            ("sudo", SUDO_STUB),
        ] {
            sandbox.executable(&sandbox.bin.join(name), body);
        }
        sandbox.answer("bash_env", BASH_ENV);
        for name in [
            "systemctl_calls",
            "systemctl_list_units",
            "systemctl_moonbot_units",
            "systemctl_all_units",
            "ss_ulnp",
            "ss_uan",
            "journal",
            "unescape",
            "ufw_calls",
            "ufw_rules",
            "service_action_fail",
            "retarget_marker",
            "retarget_order",
            "working_directory",
            "exec_start",
            "active_state",
            "main_pid",
            "active_entered",
            "memory_current",
            "readlink_cwd",
            "readlink_exe",
        ] {
            sandbox.answer(name, "");
        }
        sandbox.answer("ufw_active", "0\n");
        sandbox.answer_at(&sandbox.proc.join("uptime"), "100.00 0.00\n");
        Some(sandbox)
    }

    /// Write one canned tool answer relative to the fixture root.
    fn answer(&self, name: &str, body: &str) {
        self.answer_at(&self.root.join(name), body);
    }

    /// Write a fixture file, creating its parent directories.
    fn answer_at(&self, path: &Path, body: &str) {
        fs::create_dir_all(path.parent().expect("fixture file parent"))
            .expect("create fixture parent");
        fs::write(path, body).expect("write fixture file");
    }

    /// Create an executable shell file on Unix and a shebang executable for Git Bash on Windows.
    fn executable(&self, path: &Path, body: &str) {
        self.answer_at(path, body);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .expect("make fixture executable");
        }
    }

    /// Create a neutral instance directory below the fixture's fake `/root`.
    fn instance(&self, name: &str) -> PathBuf {
        let dir = self.root.join("root").join(name);
        fs::create_dir_all(&dir).expect("create instance directory");
        dir
    }

    /// Render a fixture path as an absolute POSIX path for Bash on every host.
    fn posix(&self, path: &Path) -> String {
        let path = path.to_string_lossy().replace('\\', "/");
        #[cfg(windows)]
        if path.as_bytes().get(1) == Some(&b':') {
            if let Some(rest) = path.get(2..) {
                return format!("/{}{rest}", path[..1].to_ascii_lowercase());
            }
        }
        path
    }

    /// Install a live UDP-owner answer for PID 123 and its fake cgroup.
    fn live_owner(&self, dir: &Path, unit: &str, executable: &str) {
        let dir = self.posix(dir);
        self.answer(
            "ss_ulnp",
            "UNCONN 0 0 0.0.0.0:3302 0.0.0.0:* users:((\"MoonBot\",pid=123,fd=7))\n",
        );
        self.answer("readlink_cwd", &format!("{dir}\n"));
        self.answer("readlink_exe", &format!("{dir}/{executable}\n"));
        self.answer("main_pid", "123\n");
        self.answer_at(
            &self.proc.join("123").join("cgroup"),
            &format!("0::/system.slice/{unit}\n"),
        );
    }

    /// Run the embedded script with only the fake system tools ahead of the host's basic tools.
    fn run(&self, args: &[&str]) -> Output {
        bash_command()
            .arg(self.posix(&self.script))
            .args(args)
            .current_dir(&self.root)
            .env("PATH", format!("{}:/usr/bin:/bin", self.posix(&self.bin)))
            .env("BASH_ENV", self.posix(&self.root.join("bash_env")))
            .env("MB_TEST_ROOT", self.posix(&self.root))
            .env("MB_PROC_ROOT", self.posix(&self.proc))
            .output()
            .expect("run embedded core command script")
    }

    /// Read a fixture file as UTF-8.
    fn read(&self, path: &Path) -> String {
        fs::read_to_string(path).expect("read fixture file")
    }
}

impl Drop for Sandbox {
    /// Remove every script, tool, answer, proc entry, and instance created by this fixture.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Decode a script command's standard output for wire assertions.
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Assert that a script command succeeded, including its output when it did not.
fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        stdout(output),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Removing the marker match must leave a MoonTerminal-installed instance undiscovered.
#[test]
fn locate_finds_ours_by_marker() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.answer_at(&dir.join(".moonterminal-move"), "3302\n");
    sandbox.executable(&dir.join("mb"), "#!/bin/sh\nexit 0\n");
    sandbox.answer(
        "systemctl_list_units",
        "moonbot@root-moonbot-a.service loaded active running MoonBot\n",
    );
    let dir_text = sandbox.posix(&dir);
    sandbox.answer("unescape", &format!("{dir_text}\n"));

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    let text = stdout(&output);
    assert!(text.contains(&format!("LOCATE_DIRECTORY={dir_text}\n")));
    assert!(text.contains("LOCATE_SERVICE=moonbot@root-moonbot-a.service\n"));
    assert!(text.contains("LOCATE_OURS=1\nLOCATE_MB=1\n"));
}

/// Accepting a UDP owner without a MoonBot executable could send commands to a foreign process.
#[test]
fn locate_accepts_live_moonbot_and_rejects_foreign_owner() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.executable(&dir.join("mb"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, "UNIT_01.service", "MoonBot");

    let hand = sandbox.run(&["locate", "3302"]);

    assert_ok(&hand);
    let text = stdout(&hand);
    assert!(text.contains("LOCATE_OURS=0\nLOCATE_MB=1\n"), "{text}");

    let foreign_dir = sandbox.instance("moonbot-b");
    sandbox.live_owner(&foreign_dir, "UNIT_02.service", "other-tool");
    let foreign = sandbox.run(&["locate", "3302"]);
    assert_ok(&foreign);
    assert_eq!(stdout(&foreign), "LOCATE_NONE=1\n");
}

/// Accepting a UDP owner without a service unit would return an empty unit to service commands.
#[test]
fn locate_rejects_udp_owner_without_service_unit() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, "UNIT_01.service", "MoonBot");
    sandbox.answer_at(
        &sandbox.proc.join("123").join("cgroup"),
        "0::/user.slice/session-1.scope\n",
    );

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "LOCATE_NONE=1\n");
}

/// A no-name systemctl show returns manager properties, so stopped discovery must name installed units.
#[test]
fn locate_finds_stopped_hand_installed_core() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.executable(&dir.join("mb"), "#!/bin/sh\nexit 0\n");
    let dir_text = sandbox.posix(&dir);
    sandbox.answer(
        "systemctl_all_units",
        "moonbot@.service enabled enabled\nUNIT_01.service enabled enabled\n",
    );
    sandbox.answer("working_directory", &format!("{dir_text}\n"));
    sandbox.answer(
        "exec_start",
        &format!("{{ path={dir_text}/MoonBot ; argv[]={dir_text}/MoonBot ; }}\n"),
    );
    sandbox.answer("active_state", "inactive\n");

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        format!("LOCATE_CANDIDATE=UNIT_01.service|{dir_text}|1|0\n")
    );
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(calls.contains("list-unit-files --type=service --no-legend --plain\n"));
    assert!(
        calls.contains("show -p Id,ExecStart,WorkingDirectory,ActiveState -- UNIT_01.service\n")
    );
    assert!(!calls.contains("show --type=service --all"));
}

/// A matching MoonProto INI port confirms a stopped candidate; a conflicting one excludes it.
#[test]
fn locate_reads_stopped_core_port_from_ini() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.answer_at(&dir.join("MoonBot.ini"), "MoonProtoPort=3302\r\n");
    let dir_text = sandbox.posix(&dir);
    sandbox.answer("systemctl_all_units", "UNIT_01.service enabled enabled\n");
    sandbox.answer("working_directory", &format!("{dir_text}\n"));
    sandbox.answer("active_state", "inactive\n");

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    assert_eq!(
        stdout(&output),
        format!("LOCATE_CANDIDATE=UNIT_01.service|{dir_text}|0|1\n")
    );
    sandbox.answer_at(&dir.join("MoonBot.ini"), "MoonProtoPort=4402\n");
    let other = sandbox.run(&["locate", "3302"]);
    assert_ok(&other);
    assert_eq!(stdout(&other), "LOCATE_NONE=1\n");
}

/// A marker for another port must not make that stopped core a candidate for this port.
#[test]
fn locate_rejects_stopped_core_on_another_port() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.answer_at(&dir.join(".moonterminal-move"), "4402\n");
    let dir_text = sandbox.posix(&dir);
    sandbox.answer("systemctl_all_units", "UNIT_01.service enabled enabled\n");
    sandbox.answer("working_directory", &format!("{dir_text}\n"));
    sandbox.answer("exec_start", &format!("{dir_text}/MoonBot\n"));
    sandbox.answer("active_state", "inactive\n");

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "LOCATE_NONE=1\n");
}

/// A missing UDP owner and no installed MoonBot service must produce the frozen no-core reply.
#[test]
fn locate_reports_none() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };

    let output = sandbox.run(&["locate", "3302"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "LOCATE_NONE=1\n");
}

/// Broadening the unmarked `mb` allowlist would let a hand-installed core be reconfigured.
#[test]
fn mb_allows_only_status_and_retry_on_unmarked_directory() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("mb"), "#!/bin/sh\nexit 0\n");
    let dir = sandbox.posix(&dir);

    for allowed in ["--status", "--retry-login"] {
        assert_ok(&sandbox.run(&["mb", &dir, allowed]));
    }
    let refused = sandbox.run(&["mb", &dir, "--exchange", "X"]);
    assert!(!refused.status.success());
}

/// Accepting a systemd action outside the frozen whitelist could run an unintended operation.
#[test]
fn service_refuses_reload() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };

    let output = sandbox.run(&["service", "reload", "3302"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(sandbox.read(&sandbox.root.join("systemctl_calls")), "");
}

/// Admitting an arbitrary log count would remove the bounded-output contract.
#[test]
fn log_refuses_unsupported_line_count() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };

    let output = sandbox.run(&["log", "3302", "300"]);

    assert_eq!(output.status.code(), Some(2));
}

/// Reading the whole file or ignoring the selected count would make log transfers unbounded.
#[test]
fn log_returns_exactly_the_selected_tail_from_a_stopped_core() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    let lines = (1..=501)
        .map(|line| format!("line-{line}\n"))
        .collect::<String>();
    sandbox.answer_at(&dir.join("logs").join("LOG_2026-01-02.log"), &lines);
    let dir_text = sandbox.posix(&dir);

    for (count, first) in [(200, 302), (500, 2)] {
        let output = sandbox.run(&["log", "3302", &count.to_string(), &dir_text]);
        assert_ok(&output);
        let expected = format!(
            "LOG_BEGIN\n{}",
            (first..=501)
                .map(|line| format!("line-{line}\n"))
                .collect::<String>()
        );
        assert_eq!(stdout(&output), expected);
    }
    let relative = sandbox.run(&["log", "3302", "200", "root/moonbot-a"]);
    assert!(!relative.status.success());
    assert!(stdout(&relative).is_empty());
}

/// Opening the new rule after deleting the old one or marking first could strand the changed endpoint.
#[test]
fn retarget_rewrites_only_the_port_and_moves_the_firewall_rule() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nufw\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    sandbox.answer("retarget_marker", &format!("{}\n", sandbox.posix(&marker)));
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "RETARGET_OK=3305\n");
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw4\nufw6\n");
    assert_eq!(
        sandbox.read(&sandbox.root.join("retarget_order")),
        format!(
            "allow from 0.0.0.0/0 to any port 3305 proto udp comment MoonBot {dir_text}|marker=3302\n\
             allow from ::/0 to any port 3305 proto udp comment MoonBot {dir_text}|marker=3302\n\
             delete allow 3302/udp|marker=3302\n"
        )
    );
}

/// Deleting and re-adding the same firewall rule could leave a working core unreachable.
#[test]
fn retarget_to_same_port_keeps_marker_and_firewall_rule() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nufw\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3302"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "RETARGET_OK=3302\n");
    assert_eq!(sandbox.read(&marker), "3302\nufw\nkept\n");
    assert_eq!(sandbox.read(&sandbox.root.join("ufw_calls")), "");
}

/// Passing a leading-dash unit hint to systemctl could turn it into an option instead of a unit.
#[test]
fn service_refuses_option_like_unit_hint_without_an_action() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };

    let output = sandbox.run(&["service", "start", "3302", "--root=x.service"]);

    assert!(!output.status.success());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains("start --"));
    assert!(!calls.contains("--root=x.service"));
}

/// Computing uptime for an inactive unit would show a stale active-enter timestamp as current uptime.
#[test]
fn info_leaves_stopped_unit_uptime_empty() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, "UNIT_01.service", "MoonBot");
    sandbox.answer("active_state", "inactive\n");
    sandbox.answer("active_entered", "42000000\n");

    let output = sandbox.run(&["info", "3302"]);

    assert_ok(&output);
    let text = stdout(&output);
    assert!(
        text.contains("INFO_ACTIVE=inactive\nINFO_UPTIME=\n"),
        "{text}"
    );
}

/// Rejecting a valid cached unit or requiring an unused wrapper would make Stop one-way.
#[test]
fn service_start_accepts_a_cached_unit_without_mb() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.answer("working_directory", &format!("{}\n", sandbox.posix(&dir)));
    sandbox.answer("active_state", "inactive\n");

    let output = sandbox.run(&["service", "start", "3302", "UNIT_01.service"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "SERVICE_OK=active\n");
    assert!(
        sandbox
            .read(&sandbox.root.join("systemctl_calls"))
            .contains("start -- UNIT_01.service\n")
    );
}

/// Systemd's escaped hyphen is part of a valid unit name, not shell syntax.
#[test]
fn service_start_accepts_a_systemd_escaped_unit() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.executable(&dir.join("mb"), "#!/bin/sh\nexit 0\n");
    sandbox.answer("working_directory", &format!("{}\n", sandbox.posix(&dir)));
    sandbox.answer("active_state", "inactive\n");
    let unit = r"moonbot@opt-CORE\x2dA.service";

    let output = sandbox.run(&["service", "start", "3302", unit]);

    assert_ok(&output);
    assert!(
        sandbox
            .read(&sandbox.root.join("systemctl_calls"))
            .contains(&format!("start -- {unit}\n"))
    );
}

/// A successful systemctl action must report the resulting service state.
#[test]
fn service_stop_reaches_systemctl_and_reports_inactive() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, "UNIT_01.service", "MoonBot");
    sandbox.answer("active_state", "active\n");

    let output = sandbox.run(&["service", "stop", "3302"]);

    assert_ok(&output);
    assert_eq!(stdout(&output), "SERVICE_OK=inactive\n");
    assert!(
        sandbox
            .read(&sandbox.root.join("systemctl_calls"))
            .contains("stop -- UNIT_01.service\n")
    );
}

/// Ignoring systemctl's exit status would report a failed restart as a success.
#[test]
fn service_restart_returns_the_journal_on_action_failure() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, "UNIT_01.service", "MoonBot");
    sandbox.answer("active_state", "active\n");
    sandbox.answer("service_action_fail", "1\n");
    sandbox.answer("journal", "restart failed\n");

    let output = sandbox.run(&["service", "restart", "3302"]);

    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "restart failed\n");
    assert!(
        sandbox
            .read(&sandbox.root.join("systemctl_calls"))
            .contains("restart -- UNIT_01.service\n")
    );
}

/// Old-rule ownership must not decide whether the new endpoint is opened.
#[test]
fn retarget_opens_a_closed_new_port_without_owned_old_rule() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw4\nufw6\n");
    assert!(
        sandbox
            .read(&sandbox.root.join("ufw_calls"))
            .contains(&format!(
                "insert 1 allow from 0.0.0.0/0 to any port 3305 proto udp comment MoonBot {dir_text}\n"
            ))
    );
    assert!(
        !sandbox
            .read(&sandbox.root.join("ufw_calls"))
            .contains("delete allow 3302/udp")
    );
}

/// A pre-existing IPv4 user allow must stay user-owned while the missing IPv6 allow is opened.
#[test]
fn retarget_does_not_claim_a_preexisting_user_rule() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nufw\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    sandbox.answer("ufw_rules", "3305/udp ALLOW IN Anywhere # user\n");
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw6\n");
    assert_eq!(
        sandbox.read(&sandbox.root.join("ufw_rules")),
        format!(
            "3305/udp (v6) ALLOW IN Anywhere (v6) # MoonBot {dir_text}\n\
             3305/udp ALLOW IN Anywhere # user\n"
        )
    );
    let calls = sandbox.read(&sandbox.root.join("ufw_calls"));
    assert!(!calls.contains("from 0.0.0.0/0 to any port 3305"));
    assert!(calls.contains("delete allow 3302/udp"));
}

/// A generic allow must not claim or later delete a user's pre-existing IPv6-only rule.
#[test]
fn retarget_preserves_a_preexisting_ipv6_rule_through_remove() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nufw\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    sandbox.answer("ufw_rules", "3305/udp (v6) ALLOW IN Anywhere (v6) # user\n");
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw4\n");
    assert_eq!(
        sandbox.read(&sandbox.root.join("ufw_rules")),
        format!(
            "3305/udp ALLOW IN Anywhere # MoonBot {dir_text}\n\
             3305/udp (v6) ALLOW IN Anywhere (v6) # user\n"
        )
    );
    assert_ok(&sandbox.run(&["remove", &dir_text]));
    assert_eq!(
        sandbox.read(&sandbox.root.join("ufw_rules")),
        "3305/udp (v6) ALLOW IN Anywhere (v6) # user\n"
    );
}

/// Appending after a broader deny would report success while the new endpoint remained blocked.
#[test]
fn retarget_inserts_new_allows_ahead_of_a_broader_deny() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    sandbox.answer("ufw_rules", "3300:3399/udp DENY IN Anywhere\n");
    let dir_text = sandbox.posix(&dir);

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(
        sandbox.read(&sandbox.root.join("ufw_rules")),
        format!(
            "3305/udp (v6) ALLOW IN Anywhere (v6) # MoonBot {dir_text}\n\
             3305/udp ALLOW IN Anywhere # MoonBot {dir_text}\n\
             3300:3399/udp DENY IN Anywhere\n"
        )
    );
}

/// A failed marker rewrite must not orphan the terminal-created allows when retarget is retried.
#[test]
fn retarget_retry_recovers_interrupted_firewall_ownership() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    let interrupted = dir.join(".moonterminal-move.retarget");
    sandbox.answer_at(&marker, "3302\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    fs::create_dir(&interrupted).expect("block the first temporary marker write");
    let dir_text = sandbox.posix(&dir);

    let first = sandbox.run(&["retarget", &dir_text, "3305"]);
    assert!(!first.status.success());
    fs::remove_dir(&interrupted).expect("restore marker writes");
    assert_ok(&sandbox.run(&["retarget", &dir_text, "3305"]));
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw4\nufw6\n");

    assert_ok(&sandbox.run(&["retarget", &dir_text, "3306"]));
    let rules = sandbox.read(&sandbox.root.join("ufw_rules"));
    assert!(!rules.contains("3305/udp"), "{rules}");
    assert!(rules.contains("3306/udp"), "{rules}");
}

/// Recovery in one family must not transfer ownership of the other family's user rule.
#[test]
fn retarget_recovers_only_the_interrupted_terminal_family() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    let marker = dir.join(".moonterminal-move");
    sandbox.answer_at(&marker, "3302\nkept\n");
    sandbox.answer("ufw_active", "1\n");
    let dir_text = sandbox.posix(&dir);
    sandbox.answer(
        "ufw_rules",
        &format!(
            "3305/udp ALLOW IN Anywhere # user\n\
             3305/udp (v6) ALLOW IN Anywhere (v6) # MoonBot {dir_text}\n"
        ),
    );

    let output = sandbox.run(&["retarget", &dir_text, "3305"]);

    assert_ok(&output);
    assert_eq!(sandbox.read(&marker), "3305\nkept\nufw6\n");
    assert_ok(&sandbox.run(&["remove", &dir_text]));
    assert_eq!(
        sandbox.read(&sandbox.root.join("ufw_rules")),
        "3305/udp ALLOW IN Anywhere # user\n"
    );
}
