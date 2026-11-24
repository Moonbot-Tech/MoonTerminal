use super::*;

#[test]
fn the_check_reports_what_the_server_has() {
    let out = "Installing: unzip\nCHECK_OS=Debian GNU/Linux 13 (trixie)\nCHECK_GLIBC=2.41\n\
               CHECK_MEM_TOTAL_MB=961\nCHECK_MEM_AVAILABLE_MB=540\nCHECK_UFW=active\nCHECK_OK\n";
    assert_eq!(
        ServerCheck::parse(out),
        ServerCheck {
            os: "Debian GNU/Linux 13 (trixie)".into(),
            glibc: "2.41".into(),
            mem_total_mb: 961,
            mem_available_mb: 540,
            ufw_active: true,
            egress: Default::default(),
        }
    );
}

/// Each `SWAP_*` line of an install step reads as what it says; no line is no swap needed.
#[test]
fn the_install_reports_its_swap() {
    let ok = "MOVE_DIRECTORY=/root/a\nMOVE_SERVICE=moonbot@root-a.service\n";
    assert_eq!(Swap::parse(ok), Swap::NotNeeded);
    assert_eq!(
        Swap::parse(&format!("SWAP_MADE=1024\n{ok}")),
        Swap::Made { mb: 1024 }
    );
    assert_eq!(
        Swap::parse(&format!("SWAP_NO_ROOM=2900\n{ok}")),
        Swap::NoRoom { free_mb: 2900 }
    );
    assert_eq!(
        Swap::parse("SWAP_FAILED=swapon: /swapfile: swapon failed: Invalid argument\n"),
        Swap::Failed("swapon: /swapfile: swapon failed: Invalid argument".into())
    );
}

/// A number that does not parse is never taken for a made swap.
#[test]
fn a_garbled_swap_line_is_a_failure_not_a_made_swap() {
    assert_eq!(
        Swap::parse("SWAP_MADE=lots\n"),
        Swap::Failed("SWAP_MADE=lots".into())
    );
}
