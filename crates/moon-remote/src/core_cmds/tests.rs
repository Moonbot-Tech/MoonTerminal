use super::*;
use crate::core_move::{SCRIPT, command};

/// Extract a found location or fail when the parser chose another outcome.
fn located(text: &str) -> Located {
    let LocateResult::Found(located) = parse_locate(text) else {
        panic!("expected a located core");
    };
    located
}

/// Losing ownership, executable flags or the no-core outcome would enable commands on the wrong core.
#[test]
fn locate_reads_ours_hand_installed_and_missing_cores() {
    let ours = located(
        "LOCATE_DIRECTORY=/root/moonbot-a\nLOCATE_SERVICE=moonbot@root-moonbot-a.service\nLOCATE_OURS=1\nLOCATE_MB=1\n",
    );
    assert_eq!(ours.dir, "/root/moonbot-a");
    assert_eq!(ours.unit, "moonbot@root-moonbot-a.service");
    assert!(ours.ours && ours.has_mb);
    assert_eq!(ours.active, None);

    let hand = located(
        "LOCATE_DIRECTORY=/opt/CORE_A\nLOCATE_SERVICE=UNIT_01.service\nLOCATE_OURS=0\nLOCATE_MB=0\n",
    );
    assert!(!hand.ours && !hand.has_mb);
    assert_eq!(parse_locate("LOCATE_NONE=1\n"), LocateResult::None);
}

/// Promoting an unknown-port candidate could start an unrelated trading service.
#[test]
fn locate_promotes_only_a_lone_candidate_with_a_confirmed_port() {
    let confirmed = "LOCATE_CANDIDATE=UNIT_01.service|/root/moonbot-a|1|1\n";
    let LocateResult::Found(one) = locate_reply(confirmed) else {
        panic!("a port-confirmed lone candidate must be usable");
    };
    assert_eq!(one.unit, "UNIT_01.service");
    assert!(one.port_known);
    assert_eq!(one.active, Some(false));

    let unknown = "LOCATE_CANDIDATE=UNIT_01.service|/root/moonbot-a|1|0\n";
    let LocateResult::Candidates(one) = locate_reply(unknown) else {
        panic!("an unknown-port candidate must not become the requested core");
    };
    assert_eq!(one.len(), 1);
    assert!(!one[0].port_known);

    let two = parse_locate(
        "LOCATE_CANDIDATE=UNIT_01.service|/root/moonbot-a|1|1\nLOCATE_CANDIDATE=UNIT_02.service|/root/moonbot-b|0|0\n",
    );
    let LocateResult::Candidates(two) = two else {
        panic!("expected two candidates");
    };
    assert_eq!(two.len(), 2);
    assert_eq!(two[1].dir, "/root/moonbot-b");
    assert!(!two[1].has_mb);
}

/// Treating an empty memory value as zero would show a stopped core as using no memory rather than unknown.
#[test]
fn info_keeps_empty_memory_unknown() {
    let info = parse_info(
        "LOCATE_DIRECTORY=/root/moonbot-a\nLOCATE_SERVICE=UNIT_01.service\nLOCATE_OURS=1\nLOCATE_MB=1\nINFO_ACTIVE=active\nINFO_UPTIME=42\nINFO_MEMORY=\n",
    )
    .unwrap();
    assert_eq!(info.active, "active");
    assert_eq!(info.located.active, Some(true));
    assert_eq!(info.uptime_s, Some(42));
    assert_eq!(info.memory, None);
}

/// Keeping SSH or journal prefixes would make protocol noise appear as core log content.
#[test]
fn log_drops_everything_through_the_begin_marker() {
    let tail = parse_log("SSH banner\nLOG_BEGIN\nfirst raw line\nsecond raw line\n");
    assert_eq!(tail.lines, ["first raw line", "second raw line"]);
    assert!(parse_log("no marker\n").lines.is_empty());
}

/// Weakening either ownership gate could reconfigure a hand-installed core or invoke a missing wrapper.
#[test]
fn gate_refuses_reconfiguration_and_missing_mb() {
    let hand = Located {
        dir: "/opt/CORE_A".into(),
        unit: "UNIT_01.service".into(),
        ours: false,
        has_mb: true,
        active: None,
        port_known: true,
    };
    for cmd in [
        Cmd::SwitchExchange,
        Cmd::ApiKeys,
        Cmd::ConnectionKey,
        Cmd::MoonProtoAddress,
        Cmd::UpdateCore,
    ] {
        assert_eq!(gate(&hand, cmd), Err(Refusal::NotOurs));
    }

    let no_mb = Located {
        ours: true,
        has_mb: false,
        ..hand
    };
    assert_eq!(gate(&no_mb, Cmd::Status), Err(Refusal::NoMb));
    assert_eq!(gate(&no_mb, Cmd::RetryLogin), Err(Refusal::NoMb));
    assert_eq!(gate(&no_mb, Cmd::Log), Ok(()));
    assert_eq!(gate(&no_mb, Cmd::Restart), Ok(()));
}

/// Changing argument quoting would make the server parse a different step, port or instance path.
#[test]
fn command_lines_use_the_frozen_steps_and_quoting() {
    assert!(command("locate", &["3302"]).ends_with("moonbot-move locate '3302'"));
    assert!(
        command("service", &["restart", "3302"]).ends_with("moonbot-move service 'restart' '3302'")
    );
    assert!(command("log", &["3302", "500"]).ends_with("moonbot-move log '3302' '500'"));
    assert!(
        command("mb", &["/root/moonbot-a", "--moonProtoKey"])
            .ends_with("moonbot-move mb '/root/moonbot-a' '--moonProtoKey'")
    );
    assert!(
        command("retarget", &["/root/moonbot-a", "3305"])
            .ends_with("moonbot-move retarget '/root/moonbot-a' '3305'")
    );
}

/// Returning a retarget error directly would discard the key after MoonProto was already configured.
#[test]
fn address_change_keeps_key_and_redacts_retarget_failure() {
    let changed = address_changed(
        Zeroizing::new("CORE_A".to_owned()),
        Err(anyhow::anyhow!("SECRETPACKET")),
    );
    assert_eq!(changed.key.as_str(), "CORE_A");
    let error = changed.retarget.unwrap_err();
    assert_eq!(
        error,
        "MoonProto was configured, but its marker and firewall rule could not be updated"
    );
    assert!(!error.contains("SECRETPACKET"));
}

/// Cloning or failing to trim a key would leave extra secret copies or an unusable connection packet.
#[test]
fn connection_key_is_moved_and_trimmed() {
    let key = connection_key(serde_json::json!({"key": "  CORE_A\n"})).unwrap();
    assert_eq!(key.as_str(), "CORE_A");
}

/// Including malformed or missing key material in an error would disclose secret reply content.
#[test]
fn invalid_connection_key_errors_exclude_the_input() {
    for reply in [
        serde_json::json!({"key": "  \n"}),
        serde_json::json!({"other": "SECRETPACKET"}),
        serde_json::json!({"key": ["SECRETPACKET"]}),
    ] {
        let error = connection_key(reply).unwrap_err();
        assert!(!format!("{error:#}").contains("SECRETPACKET"));
    }
}

/// Including stdout in a parse error would disclose the MoonProto connection key to logs and UI.
#[test]
fn unreadable_moonproto_key_reply_excludes_the_key() {
    let error =
        crate::core_new::parse_reply(true, "{\"ok\":true,\"key\":\"SECRETPACKET", "").unwrap_err();
    assert!(!format!("{error:#}").contains("SECRETPACKET"));
}

/// Formatting raw service output into Display would leak the journal instead of using its typed tail.
#[test]
fn failed_service_display_excludes_journal_text() {
    let error = service_output(Output {
        status: Some(1),
        stdout: b"partial output".to_vec(),
        stderr: b"JOURNAL_SECRET\nsecond line\n".to_vec(),
    })
    .unwrap_err();
    assert!(!format!("{error:#}").contains("JOURNAL_SECRET"));
    assert!(!format!("{error:#}").contains("partial output"));
    let failure = error.downcast_ref::<ServiceFailed>().unwrap();
    assert_eq!(failure.tail, ["JOURNAL_SECRET", "second line"]);
}

/// Removing a script arm or broadening the exception would leave the typed API calling a missing step.
#[test]
fn embedded_script_has_all_steps_and_only_safe_mb_exceptions() {
    for arm in ["locate)", "info)", "service)", "log)", "retarget)"] {
        assert!(SCRIPT.contains(arm), "missing script arm {arm}");
    }
    assert!(SCRIPT.contains("--status|--retry-login"));
}
