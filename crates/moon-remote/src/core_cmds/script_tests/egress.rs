//! The `egress` step and the binding the install and remove steps carry.

use super::*;

/// The located core's unit in these fixtures.
const UNIT: &str = "UNIT_01.service";

/// A running core on UDP 3302 whose unit systemd has put into the set of `address`.
fn bound_core(sandbox: &Sandbox, address: &str) -> PathBuf {
    let dir = sandbox.instance("moonbot-a");
    sandbox.executable(&dir.join("MoonBot"), "#!/bin/sh\nexit 0\n");
    sandbox.live_owner(&dir, UNIT, "MoonBot");
    sandbox.answer("active_state", "active\n");
    sandbox.answer(
        "nft_set",
        &format!(
            "table inet moonbot_egress {{\n\tset e{} {{\n\t\ttype cgroupsv2\n\t\t\
             elements = {{ \"system.slice/{UNIT}\" }}\n\t}}\n}}\n",
            address.replace('.', "_")
        ),
    );
    dir
}

/// The drop-in of the fixture's core.
fn dropin(sandbox: &Sandbox) -> PathBuf {
    sandbox
        .systemd_dir()
        .join(format!("{UNIT}.d"))
        .join("moonterminal-egress.conf")
}

/// Binding writes the unit's NFTSet= drop-in, a table that SNATs exactly that address out of the
/// default route's interface and refuses IPv6, the loader unit, and restarts the core.
#[test]
fn egress_binds_the_core_and_restarts_it() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert_ok(&output);
    assert!(stdout(&output).ends_with("EGRESS_OK=203.0.113.12\n"));
    let dropin = sandbox.read(&dropin(&sandbox));
    assert!(
        dropin.contains("NFTSet=cgroup:inet:moonbot_egress:e203_0_113_12\n"),
        "{dropin}"
    );
    assert!(
        dropin.contains("After=moonbot-egress.service\n"),
        "{dropin}"
    );
    // Without the table the core does not start, rather than start unbound.
    assert!(
        dropin.contains("ExecStartPre=+nft list set inet moonbot_egress e203_0_113_12\n"),
        "{dropin}"
    );
    let table = sandbox.read(&sandbox.root.join("moonbot-egress.nft"));
    assert!(
        table.contains("meta mark 0x4d540001 oifname \"eth0\" counter snat ip to 203.0.113.12\n"),
        "{table}"
    );
    assert!(
        table.contains("socket cgroupv2 level 2 @e203_0_113_12 meta nfproto ipv6 counter drop\n"),
        "{table}"
    );
    // A template instance's cgroup sits one level deeper, under system-moonbot.slice: measured on
    // a Debian 13 server, where a level-2-only rule never matched the core.
    assert!(
        table.contains("socket cgroupv2 level 3 @e203_0_113_12 meta mark set 0x4d540001\n"),
        "{table}"
    );
    let loader = sandbox.read(&sandbox.systemd_dir().join("moonbot-egress.service"));
    assert!(loader.contains("PartOf=nftables.service\n"), "{loader}");
    assert!(
        loader.contains("ExecStartPost=-systemctl daemon-reload\n"),
        "{loader}"
    );
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(
        calls.contains("restart -- moonbot-egress.service\n"),
        "{calls}"
    );
    assert!(calls.contains(&format!("restart -- {UNIT}\n")), "{calls}");
}

/// Before systemd 255 there is no NFTSet=: refused before anything is written or restarted.
#[test]
fn egress_refuses_an_old_systemd_without_touching_the_core() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    sandbox.answer("systemd_version", "systemd 252 (252.39-1~deb12u1)\n");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("systemd 255 or newer"), "{stderr}");
    assert!(stderr.contains("this server has systemd 252"), "{stderr}");
    assert!(!dropin(&sandbox).exists());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains("restart"), "{calls}");
}

/// An address the main interface does not have would never carry a connection.
#[test]
fn egress_refuses_an_address_the_server_does_not_have() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.99");

    let output = sandbox.run(&["egress", "3302", "203.0.113.99"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not on this server"));
    assert!(!dropin(&sandbox).exists());
}

/// A restarted core that systemd did not put into its set would leave from the primary address
/// unnoticed: the step fails instead of reporting success.
#[test]
fn egress_fails_when_the_restarted_core_is_not_in_its_set() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    sandbox.answer("nft_set", "table inet moonbot_egress {\n}\n");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside its outgoing address"));
    // The previous state — unbound — is put back, so the core does not read as bound.
    assert!(!dropin(&sandbox).exists());
    assert!(!sandbox.root.join("moonbot-egress.nft").exists());
}

/// Unbinding the last core takes our table, file and loader unit out with the drop-in.
#[test]
fn egress_off_on_the_last_core_removes_every_rule() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    assert_ok(&sandbox.run(&["egress", "3302", "203.0.113.12"]));

    let output = sandbox.run(&["egress", "3302", "off"]);

    assert_ok(&output);
    assert!(stdout(&output).ends_with("EGRESS_OK=off\n"));
    assert!(!dropin(&sandbox).exists());
    assert!(!sandbox.root.join("moonbot-egress.nft").exists());
    assert!(
        !sandbox
            .systemd_dir()
            .join("moonbot-egress.service")
            .exists()
    );
    let nft = sandbox.read(&sandbox.root.join("nft_calls"));
    assert!(nft.contains("delete table inet moonbot_egress\n"), "{nft}");
}

/// Asked for the binding it already has, a trading core is not restarted; the rules are rewritten
/// and reloaded, which replaces a table an older script wrote.
#[test]
fn egress_does_not_restart_a_core_already_bound_there() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    assert_ok(&sandbox.run(&["egress", "3302", "203.0.113.12"]));
    fs::write(sandbox.root.join("systemctl_calls"), "").expect("clear calls");
    fs::write(sandbox.root.join("moonbot-egress.nft"), "stale\n").expect("age the table");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert_ok(&output);
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains(&format!("restart -- {UNIT}")), "{calls}");
    assert!(
        calls.contains("restart -- moonbot-egress.service\n"),
        "{calls}"
    );
    let table = sandbox.read(&sandbox.root.join("moonbot-egress.nft"));
    assert!(table.contains("snat ip to 203.0.113.12\n"), "{table}");
}

/// A recorded address the server lost drops its cores' connections instead of letting them out
/// through the primary address, while another core's address keeps its SNAT.
#[test]
fn a_lost_address_fails_closed_and_leaves_the_others_translated() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let lost = sandbox
        .systemd_dir()
        .join("moonbot@root-old.service.d")
        .join("moonterminal-egress.conf");
    sandbox.answer_at(&lost, "# MOONTERMINAL_EGRESS=203.0.113.40\n");

    assert_ok(&sandbox.run(&["egress", "3302", "203.0.113.12"]));

    let table = sandbox.read(&sandbox.root.join("moonbot-egress.nft"));
    assert!(
        table.contains("socket cgroupv2 level 2 @e203_0_113_40 counter drop\n"),
        "{table}"
    );
    assert!(!table.contains("snat ip to 203.0.113.40"), "{table}");
    assert!(table.contains("snat ip to 203.0.113.12\n"), "{table}");
}

/// info reports the binding, its membership, the address's connection count and the choices.
#[test]
fn info_reports_the_binding_and_the_choices() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    assert_ok(&sandbox.run(&["egress", "3302", "203.0.113.12"]));
    sandbox.answer(
        "nft_chain",
        "table inet moonbot_egress {\n\tchain egress_snat {\n\t\ttype nat hook postrouting priority \
         srcnat; policy accept;\n\t\tmeta mark 0x4d540001 oifname \"eth0\" counter packets 17 \
         bytes 1020 snat ip to 203.0.113.12\n\t}\n}\n",
    );

    let output = sandbox.run(&["info", "3302"]);

    assert_ok(&output);
    let text = stdout(&output);
    assert!(
        text.contains(
            "INFO_EGRESS=203.0.113.12\nINFO_EGRESS_PRESENT=1\nINFO_EGRESS_BOUND=1\n\
             INFO_EGRESS_CONNECTIONS=17\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "EGRESS_SYSTEMD=257\nEGRESS_PRIMARY=203.0.113.11\nEGRESS_ADDRESS=203.0.113.11\n\
             EGRESS_ADDRESS=203.0.113.12\n"
        ),
        "{text}"
    );
}

/// Removing an instance takes its binding out with it.
#[test]
fn remove_unbinds_the_instance() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let dir = sandbox.instance("moonbot-a");
    sandbox.answer_at(&dir.join(".moonterminal-move"), "3302\n");
    sandbox.answer("active_state", "inactive\n");
    // The fake systemd-escape names every instance's unit this.
    let unit_dropin = sandbox
        .systemd_dir()
        .join("moonbot@root-moonbot-a.service.d")
        .join("moonterminal-egress.conf");
    sandbox.answer_at(&unit_dropin, "# MOONTERMINAL_EGRESS=203.0.113.12\n");

    let output = sandbox.run(&["remove", &sandbox.posix(&dir)]);

    assert_ok(&output);
    assert!(!unit_dropin.exists());
    assert!(!sandbox.root.join("moonbot-egress.nft").exists());
}

/// An install refuses an option it does not know before it downloads anything.
#[test]
fn install_refuses_an_unknown_option() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    let output = sandbox.run(&[
        "install-fresh",
        &sandbox.posix(&sandbox.root.join("root").join("fresh")),
        "3302",
        "https://example.invalid/MoonBot.zip",
        "egres=203.0.113.12",
    ]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unknown install option"));
    assert!(!sandbox.root.join("root").join("fresh").exists());
}

/// A stopped core — the old core of a move, or one stopped on purpose — is bound for its next
/// start and never started by the binding.
#[test]
fn egress_binds_a_stopped_core_without_starting_it() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    sandbox.answer("active_state", "inactive\n");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert_ok(&output);
    assert!(stdout(&output).ends_with("EGRESS_OK=203.0.113.12\n"));
    assert!(dropin(&sandbox).exists());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains(&format!("restart -- {UNIT}")), "{calls}");
    assert!(!calls.contains(&format!("start -- {UNIT}")), "{calls}");
}

/// A reload keeps every set's elements: the table is never deleted while a core stays bound, so
/// no other bound core is left outside its rules meanwhile.
#[test]
fn a_reload_keeps_the_sets_of_other_bound_cores() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");

    assert_ok(&sandbox.run(&["egress", "3302", "203.0.113.12"]));

    let table = sandbox.read(&sandbox.root.join("moonbot-egress.nft"));
    assert!(!table.contains("delete table"), "{table}");
    assert!(
        table.contains("flush chain inet moonbot_egress egress_mark\n"),
        "{table}"
    );
    let loader = sandbox.read(&sandbox.systemd_dir().join("moonbot-egress.service"));
    assert!(!loader.contains("ExecStop"), "{loader}");
    assert!(
        loader.contains("ReloadPropagatedFrom=nftables.service\n"),
        "{loader}"
    );
}

/// A hand-written drop-in of the fixture's core that puts it into `moon_srcip/via_extra`, listed
/// in the unit's DropInPaths as systemd would.
fn foreign_dropin(sandbox: &Sandbox) -> PathBuf {
    let path = sandbox
        .systemd_dir()
        .join(format!("{UNIT}.d"))
        .join("10-srcip.conf");
    sandbox.answer_at(
        &path,
        "[Unit]\nAfter=moon-srcip.service\n\n[Service]\nNFTSet=cgroup:inet:moon_srcip:via_extra\n",
    );
    sandbox.answer("dropin_paths", &format!("{}\n", sandbox.posix(&path)));
    path
}

/// A binding set up by hand is reported with its set, so the terminal does not read the core as
/// leaving from the primary address; one in the core's own drop-in is not "shared".
#[test]
fn info_reports_a_binding_set_up_by_hand() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    foreign_dropin(&sandbox);

    let output = sandbox.run(&["info", "3302"]);

    assert_ok(&output);
    let out = stdout(&output);
    assert!(out.contains("INFO_EGRESS=\n"), "{out}");
    assert!(
        out.contains("INFO_EGRESS_FOREIGN=cgroup:inet:moon_srcip:via_extra\n"),
        "{out}"
    );
    assert!(!out.contains("INFO_EGRESS_FOREIGN_SHARED"), "{out}");
}

/// Binding over a hand-made binding switches its NFTSet= off — the rest of that file stays — and
/// restarts the core into our set alone.
#[test]
fn egress_replaces_a_binding_set_up_by_hand() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let foreign = foreign_dropin(&sandbox);
    sandbox.answer("nft_set_moon_srcip_via_extra", "");

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert_ok(&output);
    assert!(stdout(&output).ends_with("EGRESS_OK=203.0.113.12\n"));
    let kept = sandbox.read(&foreign);
    assert!(
        kept.contains("# MOONTERMINAL_REPLACED: NFTSet=cgroup:inet:moon_srcip:via_extra\n"),
        "{kept}"
    );
    assert!(kept.contains("After=moon-srcip.service\n"), "{kept}");
    assert!(dropin(&sandbox).exists());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(calls.contains(&format!("restart -- {UNIT}\n")), "{calls}");
}

/// "Primary" over a hand-made binding is not "already unbound": the binding goes and the core
/// restarts, though no drop-in of ours ever existed.
#[test]
fn egress_off_removes_a_binding_set_up_by_hand() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let foreign = foreign_dropin(&sandbox);
    sandbox.answer("nft_set_moon_srcip_via_extra", "");

    let output = sandbox.run(&["egress", "3302", "off"]);

    assert_ok(&output);
    assert!(stdout(&output).ends_with("EGRESS_OK=off\n"));
    let kept = sandbox.read(&foreign);
    assert!(!kept.contains("\nNFTSet="), "{kept}");
    assert!(!dropin(&sandbox).exists());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(calls.contains("daemon-reload\n"), "{calls}");
    assert!(calls.contains(&format!("restart -- {UNIT}\n")), "{calls}");
}

/// A binding in a file other cores read too (the template's drop-in) is never edited: the step
/// refuses before anything changes.
#[test]
fn egress_refuses_over_a_shared_binding() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let shared = sandbox
        .systemd_dir()
        .join("moonbot@.service.d")
        .join("10-srcip.conf");
    let body = "[Service]\nNFTSet=cgroup:inet:moon_srcip:via_extra\n";
    sandbox.answer_at(&shared, body);
    sandbox.answer("dropin_paths", &format!("{}\n", sandbox.posix(&shared)));

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("undo that binding by hand"), "{stderr}");
    assert_eq!(sandbox.read(&shared), body);
    assert!(!dropin(&sandbox).exists());
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains("restart"), "{calls}");
}

/// A core that came back still inside the hand-made set fails the step, and the hand-made binding
/// is put back with the previous (no) binding of ours.
#[test]
fn egress_restores_a_binding_set_up_by_hand_when_the_core_stays_in_it() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let foreign = foreign_dropin(&sandbox);
    sandbox.answer(
        "nft_set_moon_srcip_via_extra",
        &format!(
            "table inet moon_srcip {{\n\tset via_extra {{\n\t\ttype cgroupsv2\n\t\t\
             elements = {{ \"system.slice/{UNIT}\" }}\n\t}}\n}}\n"
        ),
    );

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("still inside the set"), "{stderr}");
    let kept = sandbox.read(&foreign);
    assert!(
        kept.contains("\nNFTSet=cgroup:inet:moon_srcip:via_extra\n"),
        "{kept}"
    );
    assert!(!kept.contains("MOONTERMINAL_REPLACED"), "{kept}");
    assert!(!dropin(&sandbox).exists());
}

/// A drop-in in the core's own directory that is a link may point at a file other cores read: it
/// counts as shared, and nothing is written through it.
#[cfg(unix)]
#[test]
fn egress_refuses_a_linked_foreign_dropin() {
    let Some(sandbox) = Sandbox::new() else {
        return;
    };
    bound_core(&sandbox, "203.0.113.12");
    let target = sandbox.root.join("shared-srcip.conf");
    let body = "[Service]\nNFTSet=cgroup:inet:moon_srcip:via_extra\n";
    sandbox.answer_at(&target, body);
    let link = sandbox
        .systemd_dir()
        .join(format!("{UNIT}.d"))
        .join("10-srcip.conf");
    fs::create_dir_all(link.parent().expect("dir")).expect("create drop-in dir");
    std::os::unix::fs::symlink(&target, &link).expect("link drop-in");
    sandbox.answer("dropin_paths", &format!("{}\n", sandbox.posix(&link)));

    let output = sandbox.run(&["egress", "3302", "203.0.113.12"]);

    assert!(!output.status.success());
    assert_eq!(sandbox.read(&target), body);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("undo that binding by hand"), "{stderr}");
    let calls = sandbox.read(&sandbox.root.join("systemctl_calls"));
    assert!(!calls.contains("restart"), "{calls}");
}
