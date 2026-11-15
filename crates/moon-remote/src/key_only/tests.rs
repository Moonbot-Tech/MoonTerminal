use std::io::Write;
use std::process::Stdio;

use super::*;

/// Run `body` under `sh` after the bootstrap's functions, on a fixture where `sshd -T` answers
/// the way OpenSSH 10.5 does — capitalised keywords — and reports no password only once one of
/// the two drop-ins exists. `deaf` makes it keep saying that passwords are taken whatever the
/// files hold. Returns stdout.
fn bootstrap_fixture(deaf: bool, body: &str) -> String {
    let (functions, _) = script::BOOTSTRAP
        .split_once("\nstep=${1:-}\n")
        .expect("bootstrap.sh has a dispatcher");
    let deaf = if deaf { "yes" } else { "no" };
    let code = format!(
        r#"FIX=$(mktemp -d)
trap 'rm -rf "$FIX"' EXIT
id() {{ printf '0\n'; }}
{functions}
KEY_ONLY_DROPIN="$FIX/00-moon-key-only.conf"
SSHD_DROPIN="$FIX/00-moon-station.conf"
sshd_includes_dropins() {{ :; }}
install() {{ cp "$7" "$8"; }}
systemctl() {{ :; }}
sshd() {{
    case "$1" in
    -t) return 0 ;;
    -T)
        printf 'Port 22\nKbdInteractiveAuthentication no\n'
        if [ {deaf} = no ] && {{ [ -f "$KEY_ONLY_DROPIN" ] || [ -f "$SSHD_DROPIN" ]; }}; then
            printf 'PasswordAuthentication no\nPermitRootLogin no\n'
        else
            printf 'PasswordAuthentication yes\nPermitRootLogin yes\n'
        fi
        ;;
    esac
}}
{body}
"#
    );
    // Stdin carries the code, as in the setup's fixtures: no argv length limit, no quoting.
    let mut child = std::process::Command::new("sh")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Git Bash sh or POSIX sh is required for the sshd fixture");
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
    String::from_utf8_lossy(&out.stdout).replace('\r', "")
}

/// Comparing `sshd -T` as printed reads OpenSSH 10.5's `PasswordAuthentication no` as a setting
/// that never took: the step would then remove its own file and fail on every such server. The
/// second run must find the server closed and leave it alone.
#[test]
fn the_step_closes_a_server_whose_sshd_prints_capitalised_keywords() {
    let out = bootstrap_fixture(
        false,
        r#"step_key_only
step_key_only
echo '==FILE=='
cat "$KEY_ONLY_DROPIN"
echo '==PORTS=='
ssh_ports"#,
    );
    assert_eq!(
        out,
        "sshd=changed\nsshd=already\n==FILE==\n\
         # MoonBot core: key logins only. Sorted first so no later file can reopen it.\n\
         PasswordAuthentication no\nKbdInteractiveAuthentication no\n==PORTS==\n22\n"
    );
}

/// A drop-in that sshd does not follow must not stay: it sorts first, so it would silently take
/// effect the day the setting that overrode it goes away.
#[test]
fn a_file_sshd_does_not_follow_is_removed_again() {
    let out = bootstrap_fixture(
        true,
        r#"if (step_key_only) 2>/dev/null; then echo closed; else echo failed; fi
if [ -f "$KEY_ONLY_DROPIN" ]; then echo left; else echo removed; fi"#,
    );
    assert_eq!(out, "failed\nremoved\n");
}

/// The station's own closing step compared `sshd -T` in lower case as well, so on OpenSSH 10.5
/// it rolled itself back with "sshd still does not say".
#[test]
fn the_station_step_accepts_capitalised_keywords_too() {
    assert_eq!(bootstrap_fixture(false, "step_harden"), "sshd=changed\n");
}

/// Closed means the server names no password method; reading the flag the other way round
/// would report an open server as closed.
#[test]
fn only_a_refusal_without_password_methods_counts_as_closed() {
    let refused = |password_offered| -> Result<(), OpenError> {
        Err(OpenError::Refused { password_offered })
    };
    assert!(password_refused(refused(false), "root").is_ok());
    let open = password_refused(refused(true), "root").expect_err("a password is still offered");
    assert!(open.to_string().contains("still offers a password"));
    assert!(password_refused(Ok(()), "root").is_err());
}
