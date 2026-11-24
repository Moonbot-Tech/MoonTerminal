//! The swap an install — fresh or a move — gives a small server before the core's first start.

use sha2::{Digest, Sha256};

use super::*;

/// The download, the unpack and the swap tools, as functions: Git Bash puts its own `curl`, `dd`,
/// `df` and `unzip` ahead of the fake ones on `PATH`. The swap tools log into `systemctl_calls`,
/// so one file shows whether the swap came before the core's start.
const SWAP_ENV: &str = r#"
curl() {
    while test $# -gt 0; do
        case $1 in --output) : > "$2"; shift ;; esac
        shift
    done
}
unzip() {
    case ${1:-} in
    -Z1) printf 'BotConfig.bcfg\n' ;;
    -q)
        case $2 in
        */state.zip) printf 'bcfg' > "$4/BotConfig.bcfg" ;;
        *) cp -R "$MB_TEST_ROOT/package/." "$4/" ;;
        esac
        ;;
    *) return 1 ;;
    esac
}
df() {
    cat "$MB_TEST_ROOT/df"
}
dd() {
    for dd_arg; do
        case $dd_arg in of=*) dd_out=${dd_arg#of=} ;; esac
    done
    printf 'dd %s\n' "$*" >> "$MB_TEST_ROOT/systemctl_calls"
    printf 'swap' > "$dd_out"
}
mkswap() {
    printf 'mkswap %s\n' "$*" >> "$MB_TEST_ROOT/systemctl_calls"
}
swapon() {
    printf 'swapon %s\n' "$*" >> "$MB_TEST_ROOT/systemctl_calls"
    if test -s "$MB_TEST_ROOT/swapon_fail"; then
        printf 'swapon: %s: swapon failed: Operation not permitted\n' "$2" >&2
        return 255
    fi
}
swapoff() {
    printf 'swapoff %s\n' "$*" >> "$MB_TEST_ROOT/systemctl_calls"
}
"#;

/// A root file system with `free_mb` MiB available, as `df -Pk` prints it.
fn df_with_free(free_mb: u64) -> String {
    format!(
        "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
         /dev/vda1 25000000 1000000 {} 5% /\n",
        free_mb * 1024
    )
}

/// A sandbox for `install-fresh` on a server of `mem_mb` MiB with `swap_mb` MiB of swap on and
/// `free_mb` MiB free on the disk: a package that passes its checksum, and a core that answers
/// `mb --status` at once.
fn server(mem_mb: u64, swap_mb: u64, free_mb: u64) -> Option<Sandbox> {
    let sandbox = Sandbox::new()?;
    sandbox.answer("bash_env", &format!("{BASH_ENV}{SWAP_ENV}"));
    sandbox.answer_at(
        &sandbox.proc.join("meminfo"),
        &format!(
            "MemTotal:       {} kB\nMemAvailable:   {} kB\nSwapTotal:      {} kB\n",
            mem_mb * 1024,
            mem_mb * 512,
            swap_mb * 1024
        ),
    );
    sandbox.answer("df", &df_with_free(free_mb));
    sandbox.answer("swapon_fail", "");
    fs::create_dir_all(sandbox.systemd_dir()).expect("create fixture systemd directory");
    fs::create_dir_all(sandbox.root.join("usr-local-bin")).expect("create fixture bin directory");
    let package = sandbox.root.join("package");
    let mut sums = String::new();
    for (name, body) in [
        ("MoonBot", "#!/bin/sh\nexit 0\n"),
        ("mb", "#!/bin/sh\nexit 0\n"),
        ("moonbot@.service", "[Service]\n"),
    ] {
        sandbox.executable(&package.join(name), body);
        let digest = Sha256::digest(body.as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        sums.push_str(&format!("{hex}  {name}\n"));
    }
    sandbox.answer_at(&package.join("SHA256SUMS"), &sums);
    Some(sandbox)
}

/// Run a fresh install into the fixture's `/root/fresh`.
fn install_fresh(sandbox: &Sandbox) -> Output {
    sandbox.run(&[
        "install-fresh",
        &sandbox.posix(&sandbox.root.join("root").join("fresh")),
        "3302",
        "https://example.invalid/MoonBot.zip",
    ])
}

/// The tool calls in the order they ran.
fn calls(sandbox: &Sandbox) -> Vec<String> {
    sandbox
        .read(&sandbox.root.join("systemctl_calls"))
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Where in `calls` the first call starting with `prefix` is.
fn position(calls: &[String], prefix: &str) -> Option<usize> {
    calls.iter().position(|call| call.starts_with(prefix))
}

/// A 1 GiB server without swap gets a swap file, turned on and in fstab on a line of its own,
/// before the core's first start.
#[test]
fn a_small_server_without_swap_gets_it_before_the_first_start() {
    let Some(sandbox) = server(961, 0, 20_000) else {
        return;
    };
    // A last line without its newline must not swallow the one the install adds.
    sandbox.answer("fstab", "UUID=abcd / ext4 errors=remount-ro 0 1");

    let output = install_fresh(&sandbox);

    assert_ok(&output);
    let text = stdout(&output);
    assert!(text.contains("SWAP_MADE=1024\n"), "{text}");
    assert!(text.contains("MOVE_DIRECTORY="), "{text}");
    let swap_file = sandbox.posix(&sandbox.root.join("swapfile"));
    assert!(sandbox.root.join("swapfile").exists());
    let fstab = sandbox.read(&sandbox.root.join("fstab"));
    assert_eq!(
        fstab,
        format!("UUID=abcd / ext4 errors=remount-ro 0 1\n{swap_file} none swap sw,nofail 0 0\n")
    );
    let calls = calls(&sandbox);
    let swapon = position(&calls, "swapon").expect("swapon ran");
    let start = position(&calls, "enable --now").expect("the core was started");
    assert!(swapon < start, "{calls:?}");
    assert!(position(&calls, "dd ").expect("dd ran") < position(&calls, "mkswap").unwrap());
}

/// A swap that cannot be turned on is said with the tool's reason, leaves no file and no fstab
/// line behind, and the core is installed and started all the same.
#[test]
fn a_swap_that_fails_is_reported_and_the_install_goes_on() {
    let Some(sandbox) = server(961, 0, 20_000) else {
        return;
    };
    sandbox.answer("swapon_fail", "1\n");
    sandbox.answer("fstab", "UUID=abcd / ext4 defaults 0 1\n");

    let output = install_fresh(&sandbox);

    assert_ok(&output);
    let text = stdout(&output);
    let swap_file = sandbox.posix(&sandbox.root.join("swapfile"));
    assert!(
        text.contains(&format!(
            "SWAP_FAILED=swapon: {swap_file}: swapon failed: Operation not permitted\n"
        )),
        "{text}"
    );
    assert!(text.contains("MOVE_DIRECTORY="), "{text}");
    assert!(!sandbox.root.join("swapfile").exists());
    assert!(!sandbox.root.join("swapfile.moonterminal-part").exists());
    assert_eq!(
        sandbox.read(&sandbox.root.join("fstab")),
        "UUID=abcd / ext4 defaults 0 1\n"
    );
    assert!(position(&calls(&sandbox), "enable --now").is_some());
}

/// A server over 1 GiB, or one with swap of its own on, is left as it is and says nothing.
#[test]
fn a_server_with_enough_memory_or_its_own_swap_is_left_alone() {
    for (mem_mb, swap_mb) in [(1025, 0), (961, 512)] {
        let Some(sandbox) = server(mem_mb, swap_mb, 20_000) else {
            return;
        };

        let output = install_fresh(&sandbox);

        assert_ok(&output);
        let text = stdout(&output);
        assert!(!text.contains("SWAP_"), "{mem_mb}/{swap_mb}: {text}");
        assert!(position(&calls(&sandbox), "dd ").is_none());
        assert!(!sandbox.root.join("fstab").exists());
    }
}

/// A disk that would keep less than 2 GiB beside the swap file gets none, and says how much it
/// has; exactly 3 GiB free is enough.
#[test]
fn a_disk_without_room_gets_no_swap() {
    let Some(sandbox) = server(961, 0, 3071) else {
        return;
    };

    let output = install_fresh(&sandbox);

    assert_ok(&output);
    assert!(stdout(&output).contains("SWAP_NO_ROOM=3071\n"));
    assert!(position(&calls(&sandbox), "dd ").is_none());
    assert!(!sandbox.root.join("swapfile").exists());

    let Some(sandbox) = server(961, 0, 3072) else {
        return;
    };
    let output = install_fresh(&sandbox);
    assert_ok(&output);
    assert!(stdout(&output).contains("SWAP_MADE=1024\n"));
}

/// A swap file that exists but is off is someone's own: never overwritten, and said so.
#[test]
fn an_existing_swap_file_that_is_off_is_left_as_it_is() {
    let Some(sandbox) = server(961, 0, 20_000) else {
        return;
    };
    sandbox.answer("swapfile", "theirs");

    let output = install_fresh(&sandbox);

    assert_ok(&output);
    assert!(stdout(&output).contains("already exists and is not in use: left as it is\n"));
    assert_eq!(sandbox.read(&sandbox.root.join("swapfile")), "theirs");
    assert!(position(&calls(&sandbox), "dd ").is_none());
}

/// A move's install makes the swap as well, and still leaves the core unstarted for the old
/// core's stop.
#[test]
fn a_move_install_makes_swap_and_does_not_start_the_core() {
    let Some(sandbox) = server(961, 0, 20_000) else {
        return;
    };
    let work = sandbox.root.join("work");
    sandbox.answer_at(&work.join("state.zip"), "zip");
    sandbox.answer_at(&work.join("key"), "packet");

    let output = sandbox.run(&[
        "install",
        &sandbox.posix(&work),
        &sandbox.posix(&sandbox.root.join("root").join("moved")),
        "3302",
        "https://example.invalid/MoonBot.zip",
    ]);

    assert_ok(&output);
    let text = stdout(&output);
    assert!(text.contains("SWAP_MADE=1024\n"), "{text}");
    assert!(text.contains("MOVE_DIRECTORY="), "{text}");
    assert!(sandbox.root.join("swapfile").exists());
    let calls = calls(&sandbox);
    assert!(position(&calls, "swapon").is_some(), "{calls:?}");
    assert!(position(&calls, "enable --now").is_none(), "{calls:?}");
}

/// The half-written file of an install killed mid-way is cleared by the next install even when
/// that server needs no swap any more, and is never taken for someone's own swap file.
#[test]
fn a_leftover_of_an_interrupted_install_is_cleared() {
    let Some(sandbox) = server(2048, 0, 20_000) else {
        return;
    };
    sandbox.answer("swapfile.moonterminal-part", "half");

    let output = install_fresh(&sandbox);

    assert_ok(&output);
    assert!(!stdout(&output).contains("SWAP_"));
    assert!(!sandbox.root.join("swapfile.moonterminal-part").exists());
    assert!(!sandbox.root.join("swapfile").exists());
}
