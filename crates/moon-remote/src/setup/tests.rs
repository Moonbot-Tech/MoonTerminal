//! Safe-install regressions: the host key is confirmed before anything is sent, the firewall keeps
//! the SSH ports open and rolls back exactly, an unsupported server is refused by the probe.
//! Nothing here connects anywhere; the shell fixtures run `bootstrap.sh` against stub tools.

use super::*;

use std::io::Write;
use std::process::Stdio;

/// The source of `setup.rs` itself, for the one order contract no unit seam can observe.
const SETUP_SOURCE: &str = include_str!("../setup.rs");

const PIN: &str = "SHA256:pinned";
const SEEN: &str = "SHA256:presented";

fn is_unconfirmed(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<StationError>(),
        Some(StationError::HostKeyUnconfirmed)
    )
}

/// `setup.rs::trusted_pin`: the `(None, None) => Err(HostKeyUnconfirmed)` arm turned into trust on
/// first use would send the root password to whatever answers at the address (a man in the
/// middle), so a server nobody pinned and nobody confirmed must be no connection at all.
#[test]
fn an_unpinned_unconfirmed_server_is_never_trusted() {
    for set_up in [false, true] {
        let error = trusted_pin(None, None, set_up).unwrap_err();
        assert!(is_unconfirmed(&error), "set_up={set_up}: {error:#}");
    }
}

/// `setup.rs::trusted_pin`: a confirmation that disagrees with the pin of a server this tool set up
/// must stay a changed-key refusal; accepting it would let a swapped server keep the pin's trust.
#[test]
fn a_confirmation_against_the_pin_of_a_set_up_server_is_a_changed_key() {
    let error = trusted_pin(Some(PIN), Some(SEEN), true).unwrap_err();
    match error.downcast_ref::<OpenError>() {
        Some(OpenError::HostKeyChanged { pinned, presented }) => {
            assert_eq!((pinned.as_str(), presented.as_str()), (PIN, SEEN));
        }
        _ => panic!("expected HostKeyChanged, got {error:#}"),
    }
}

/// `setup.rs::trusted_pin`: which key a login is pinned to in each of the four allowed cases. A
/// wrong pick connects to the wrong key; the stale pin of an unfinished setup gives way to what
/// the user confirmed now.
#[test]
fn the_pin_a_login_uses_follows_the_confirmation() {
    let pick = |pinned, confirmed, set_up| trusted_pin(pinned, confirmed, set_up).unwrap();
    // The stored pin, nothing newly confirmed.
    assert_eq!(pick(Some(PIN), None, true), PIN);
    assert_eq!(pick(Some(PIN), None, false), PIN);
    // A confirmation that agrees with the pin.
    assert_eq!(pick(Some(PIN), Some(PIN), true), PIN);
    // The pin of a setup that never finished loses to the confirmation.
    assert_eq!(pick(Some(PIN), Some(SEEN), false), SEEN);
    // First contact: only the confirmation.
    assert_eq!(pick(None, Some(SEEN), false), SEEN);
}

/// `setup.rs::run`: moving the `trusted_pin(` call below the app key, the first login or the first
/// connection lets a credential or a fresh key leave before the user's confirmation is checked.
/// The order has no seam a unit test can observe (each of those steps touches the disk or the
/// network), so it is pinned in the source: the call comes first in `run`'s body.
#[test]
fn run_checks_the_host_key_before_any_login_or_key_creation() {
    let from = SETUP_SOURCE
        .find("pub fn run(")
        .expect("setup.rs has `pub fn run(`");
    let tail = &SETUP_SOURCE[from..];
    let body = &tail[..tail.find("\n}\n").expect("run's body ends at column 0")];
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("run lacks {needle}"))
    };
    let pin = at("trusted_pin(");
    for later in ["load_or_create(", "Conn::open(", "first_login("] {
        assert!(pin < at(later), "trusted_pin must come before {later}");
    }
}

/// `setup.rs::firewall_command`: dropping the port argument (or hard-coding 22 again) firewalls a
/// server whose SSH runs elsewhere shut. The oracle is the documented shape of the command line:
/// the step name, then each argument as one single-quoted POSIX word.
#[test]
fn the_firewall_step_is_told_the_ssh_port_the_setup_dials() {
    for port in [2222u16, 22022, 22] {
        let target = Target {
            host: "203.0.113.10".into(),
            port,
        };
        let command = firewall_command(&target);
        assert!(
            command.ends_with(&format!(" moon-bootstrap firewall '{port}'")),
            "port {port}: {}",
            command.lines().last().unwrap_or_default()
        );
    }
}

/// `setup.rs::check_probe`: deleting the apt check lets a non-Debian server be hardened and then
/// die at the firewall step half-configured. Each refusal is pinned to its own probe line.
#[test]
fn the_probe_refuses_a_server_the_setup_cannot_finish() {
    let probe = |systemd: &str, apt: &str, include: &str| {
        format!("arch=x86_64\nsystemd={systemd}\napt={apt}\nsshd_include={include}\nufw=none\n")
    };
    assert_eq!(check_probe(&probe("255", "yes", "yes")), Ok(()));
    assert_eq!(check_probe(&probe("250", "yes", "yes")), Ok(()));
    assert_eq!(
        check_probe(&probe("249", "yes", "yes")),
        Err(StationError::SystemdTooOld)
    );
    assert_eq!(
        check_probe(&probe("none", "yes", "yes")),
        Err(StationError::SystemdMissing)
    );
    assert_eq!(
        check_probe(&probe("255", "no", "yes")),
        Err(StationError::NoAptGet)
    );
    assert_eq!(
        check_probe(&probe("255", "yes", "no")),
        Err(StationError::SshdNoInclude)
    );
    // A probe that says nothing about apt is not a pass either.
    assert_eq!(
        check_probe("systemd=255\nsshd_include=yes\n"),
        Err(StationError::NoAptGet)
    );
}

/// What `step_firewall 2222` printed and which `ufw` calls it made, against a stub `ufw` whose
/// `status` says `ufw_status` and whose `allow` prints `allow_reply`, and a stub `sshd` listening
/// on 22. The step runs the embedded `bootstrap.sh` functions as POSIX `sh`, minus the trailing
/// dispatcher; no real firewall is involved.
fn firewall_fixture(ufw_status: &str, allow_reply: &str) -> (Vec<String>, Vec<String>) {
    let functions = script::BOOTSTRAP
        .split("\nstep=${1:-}\n")
        .next()
        .expect("bootstrap.sh has a dispatcher");
    assert!(functions.contains("step_firewall()"));
    let code = format!(
        r#"UFW_STATUS='{ufw_status}'
ALLOW_REPLY='{allow_reply}'
LOG=$(mktemp)
OUT=$(mktemp)
id() {{ printf '0\n'; }}
sshd() {{ printf 'port 22\n'; }}
ufw() {{
    printf '%s\n' "$*" >>"$LOG"
    case "$1" in
    status) printf 'Status: %s\n' "$UFW_STATUS" ;;
    allow) printf '%s\n' "$ALLOW_REPLY" ;;
    esac
}}
{functions}
step_firewall 2222 >"$OUT"
echo '==OUT=='
cat "$OUT"
echo '==LOG=='
cat "$LOG"
rm -f "$OUT" "$LOG"
"#
    );
    // Stdin avoids Windows command-line length limits and shell-wrapper quoting entirely.
    let mut child = std::process::Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git Bash sh or POSIX sh is required for the firewall fixture");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let (_, rest) = text.split_once("==OUT==\n").expect("fixture marker");
    let (stdout, log) = rest.split_once("==LOG==\n").expect("fixture marker");
    let lines = |s: &str| s.lines().map(str::to_owned).collect();
    (lines(stdout), lines(log))
}

/// Removing RSA from `step_admin`'s filter aborts setup and loses the provider key as an
/// administrator login. Run the embedded step and check both input lines reach authorized_keys.
#[test]
fn the_administrator_keeps_the_app_key_and_the_provider_rsa_key() {
    let app_line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH fixture-app";
    let provider = crate::keys::parse(include_str!("../keys/fixtures/rsa-pkcs1.pem"), None)
        .expect("synthetic provider RSA key");
    let provider_line = crate::keys::authorized_line(&provider).expect("provider public line");
    let input = format!("{app_line}\n{provider_line}\n");
    let (functions, _) = script::BOOTSTRAP
        .split_once("\nstep=${1:-}\n")
        .expect("bootstrap.sh has a dispatcher");
    let code = format!(
        r#"FIXTURE_HOME=$(mktemp -d)
trap 'rm -f "${{keys_in:-}}"; rm -rf "$FIXTURE_HOME"' EXIT
id() {{
    case "$1" in
    -u) printf '0\n' ;;
    -gn) printf 'fixture-group\n' ;;
    *) return 1 ;;
    esac
}}
getent() {{ printf 'fixture-admin:x:1000:1000::%s:/bin/sh\n' "$FIXTURE_HOME"; }}
useradd() {{ :; }}
chown() {{ :; }}
chmod() {{ :; }}
install() {{
    case "$1" in
    -d) mkdir -p "$8" ;;
    -m) : >"$8" ;;
    *) return 1 ;;
    esac
}}
{functions}
step_admin fixture-admin <<'FIXTURE_KEYS'
{input}FIXTURE_KEYS
printf '==KEYS==\n'
cat "$FIXTURE_HOME/.ssh/authorized_keys"
"#,
    );
    // As in the firewall fixture, stdin carries code without Windows argv length limits.
    let mut child = std::process::Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git Bash sh or POSIX sh is required for the administrator fixture");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(code.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "administrator step failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let (stdout, authorized_keys) = text.split_once("==KEYS==\n").expect("fixture marker");
    assert_eq!(
        stdout.lines().filter(|line| *line == "key=added").count(),
        2
    );
    assert_eq!(authorized_keys, input);
}

fn allows(log: &[String]) -> Vec<&str> {
    log.iter()
        .map(String::as_str)
        .filter(|l| l.starts_with("allow "))
        .collect()
}

fn touches_policy(log: &[String]) -> bool {
    log.iter()
        .any(|l| l.starts_with("default") || l.contains("enable") || l.contains("disable"))
}

/// `bootstrap.sh::step_firewall` (active branch): adding `ufw default deny incoming` (or an
/// `enable`) there rewrites the policy of a firewall the user already runs and may cut off its
/// other services. An active ufw gains only the SSH-port rules, and a rule that already existed
/// is reported `unchanged` so the rollback never deletes the user's own rule.
#[test]
fn an_active_firewall_only_gains_the_ssh_rules() {
    let (stdout, log) = firewall_fixture("active", "Skipping adding existing rule");
    assert_eq!(allows(&log), ["allow 22/tcp", "allow 2222/tcp"]);
    assert!(!touches_policy(&log), "policy touched: {log:?}");
    assert_eq!(
        stdout.last().map(String::as_str),
        Some("firewall=unchanged")
    );
    assert!(!stdout.iter().any(|l| l.starts_with("firewall_added=")));

    let (stdout, log) = firewall_fixture("active", "Rule added");
    assert!(!touches_policy(&log), "policy touched: {log:?}");
    assert_eq!(
        stdout.last().map(String::as_str),
        Some("firewall=rule-added")
    );
    assert!(stdout.contains(&"firewall_added=2222".to_owned()));
}

/// `bootstrap.sh::step_firewall` (inactive branch): enabling ufw before the SSH rules are in (or
/// allowing a port other than sshd's own and the client's) locks the administrator out at the
/// moment the firewall turns on.
#[test]
fn an_inactive_firewall_opens_both_ssh_ports_before_it_is_enabled() {
    let (stdout, log) = firewall_fixture("inactive", "Rule added");
    assert_eq!(allows(&log), ["allow 22/tcp", "allow 2222/tcp"]);
    let at = |line: &str| {
        log.iter()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("no `ufw {line}` call in {log:?}"))
    };
    let enable = at("--force enable");
    assert!(at("allow 22/tcp") < enable && at("allow 2222/tcp") < enable);
    assert_eq!(stdout.last().map(String::as_str), Some("firewall=enabled"));
    assert!(stdout.contains(&"firewall_added=22".to_owned()));
    assert!(stdout.contains(&"firewall_added=2222".to_owned()));
}

/// `setup.rs::firewall_rollback`: the no-end-line case of an already active firewall turned into an
/// undo deletes the user's own SSH rule on a failed run, and an `unchanged` run must undo nothing.
/// Only rules this run reported adding are ever removed.
#[test]
fn an_active_firewall_is_not_rolled_back_beyond_the_rules_added() {
    assert_eq!(firewall_rollback("firewall=unchanged\n", "active"), None);
    // The step died before reporting anything, on a firewall that was already active.
    assert_eq!(firewall_rollback("", "active"), None);
    // Died after adding one rule: that rule goes, nothing else, and the firewall stays on.
    let undo = firewall_rollback("firewall_added=2222\n", "active").expect("an added rule");
    assert!(undo.ends_with(" moon-bootstrap firewall-unallow '2222'"));
    let undo = firewall_rollback("firewall_added=2222\nfirewall=rule-added\n", "active")
        .expect("an added rule");
    assert!(undo.ends_with(" moon-bootstrap firewall-unallow '2222'"));
}

/// `setup.rs::firewall_rollback`: a firewall this run enabled is turned off again with the policy it
/// found and exactly the rules it added; dropping either leaves the server closed or the old
/// policy unrestored.
#[test]
fn a_firewall_this_run_enabled_is_rolled_back_exactly() {
    let stdout =
        "firewall_added=22\nfirewall_added=2222\nfirewall_prev_in=DROP\nfirewall=enabled\n";
    let undo = firewall_rollback(stdout, "inactive").expect("it enabled the firewall");
    assert!(undo.ends_with(" moon-bootstrap firewall-off 'DROP' '22' '2222'"));
    // Half-way, with no end line, on a firewall that was off: the same undo.
    let half = "firewall_added=2222\nfirewall_prev_in=DROP\n";
    let undo = firewall_rollback(half, "inactive").expect("it was off before");
    assert!(undo.ends_with(" moon-bootstrap firewall-off 'DROP' '2222'"));
}
