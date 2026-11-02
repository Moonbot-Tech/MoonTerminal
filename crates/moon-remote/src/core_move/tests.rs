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
        }
    );
}
