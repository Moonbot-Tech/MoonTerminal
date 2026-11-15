use super::{NewJob, NewPhase, reset_core_new_for_login, unique_name};
use gpui::Task;

/// Host correction must preserve the exported port and must not acquire station ownership.
#[test]
fn address_save_preserves_key_port_and_open_draft() {
    let mut saved = address_config();
    let mut draft = Some(saved.clone());
    draft.as_mut().unwrap().servers[0].name = "Draft name".into();
    let mut committed = None;
    super::save_wizard_address(&mut saved, &mut draft, 42, "203.0.113.24", |candidate| {
        committed = Some(candidate.clone());
        Ok(())
    })
    .unwrap();
    for config in [&saved, draft.as_ref().unwrap(), committed.as_ref().unwrap()] {
        let row = &config.servers[0];
        let endpoint = moon_core::config::parse_endpoint_override(&row.endpoint_override).unwrap();
        let target =
            moon_core::config::target_from_key(row.key.expose(), endpoint.as_ref()).unwrap();
        assert_eq!(target.text(), "203.0.113.24:4321");
        assert!(!row.endpoint_to_station);
    }
    assert_eq!(draft.unwrap().servers[0].name, "Draft name");
    assert_eq!(
        saved.servers[0].endpoint_override,
        committed.unwrap().servers[0].endpoint_override
    );
}

/// A host correction must retain a custom override port in both saved config and an edited draft.
#[test]
fn address_save_preserves_existing_override_port() {
    for host in ["203.0.113.24", "core.example", "2001:db8::24"] {
        let mut saved = address_config();
        saved.servers[0].endpoint_override = ":5010".into();
        saved.servers[0].endpoint_to_station = true;
        let mut draft = Some(saved.clone());
        draft.as_mut().unwrap().servers[0].endpoint_override = ":6000".into();
        let mut committed = None;
        super::save_wizard_address(&mut saved, &mut draft, 42, host, |candidate| {
            committed = Some(candidate.clone());
            Ok(())
        })
        .unwrap();
        for config in [&saved, draft.as_ref().unwrap(), committed.as_ref().unwrap()] {
            let row = &config.servers[0];
            let endpoint = moon_core::config::parse_endpoint_override(&row.endpoint_override)
                .unwrap()
                .unwrap();
            let target =
                moon_core::config::target_from_key(row.key.expose(), Some(&endpoint)).unwrap();
            assert_eq!(target.port, 5010);
            assert_eq!(target.host.to_string(), host);
            assert!(row.endpoint_to_station);
            assert_eq!(row.endpoint_override, saved.servers[0].endpoint_override);
        }
    }
}

/// Host validation belongs to persistence and must reject a supplied port before saving anything.
#[test]
fn address_save_rejects_invalid_host_before_persistence() {
    for host in [
        "",
        ":5010",
        "203.0.113.24:5010",
        "not a host",
        "203.0.113.999",
    ] {
        let mut saved = address_config();
        let mut draft = Some(saved.clone());
        let error = super::save_wizard_address(&mut saved, &mut draft, 42, host, |_| {
            panic!("invalid input reached persistence")
        })
        .unwrap_err();
        assert!(error.is::<super::InvalidWizardAddress>());
        assert!(saved.servers[0].endpoint_override.is_empty());
        assert!(draft.unwrap().servers[0].endpoint_override.is_empty());
    }
}

/// Disk failure must publish neither the replacement address nor an open draft change.
#[test]
fn address_save_failure_publishes_nothing() {
    let mut saved = address_config();
    saved.servers[0].endpoint_to_station = true;
    let mut draft = Some(saved.clone());
    assert!(
        super::save_wizard_address(&mut saved, &mut draft, 42, "203.0.113.24", |_| {
            Err(anyhow::anyhow!("synthetic disk failure"))
        })
        .is_err()
    );
    for config in [&saved, draft.as_ref().unwrap()] {
        assert!(config.servers[0].endpoint_override.is_empty());
        assert!(config.servers[0].endpoint_to_station);
    }
}

/// Frozen synthetic MoonProto V1 export carries port 4321, independently of override code.
fn address_config() -> moon_core::config::AppConfig {
    let mut row = crate::settings::connections::table::new_server_row(
        1,
        "CORE_A".into(),
        moon_core::config::servers::default_group(),
        "sX85BQAAAAD4HMdln7gLXlN0DqD1Qs810ml1VLTx0vkRfwzU9VrjS+XMkD1SzrhZWGd2JDVy92AArwH8gJLfmM/47yuKci+sFrrtNibJShbRnc1HGycnqLRazhICIMdoPAhGryNcv1KZClUCEhH6mRG/Np81EodJlA==",
        [0, 0, 0],
    );
    row.uid = 42;
    moon_core::config::AppConfig::headless(vec![row])
}

#[test]
fn a_taken_name_gets_the_first_free_number() {
    let taken = ["mbcore", "mbcore 2", "BB1"];
    assert_eq!(unique_name("mbcore", taken.iter().copied()), "mbcore 3");
    assert_eq!(unique_name("mbnew", taken.iter().copied()), "mbnew");
}

/// Build a backend job with no worker or filesystem effects for reset-path tests.
fn job(phase: NewPhase) -> Option<NewJob> {
    Some(NewJob {
        phase,
        lines: Vec::new(),
        status: None,
        exchanges: Vec::new(),
        public_ip: String::new(),
        proto_port: 0,
        dir: None,
        uid: None,
        demo: false,
        row_error: None,
        commands: None,
        _task: Task::ready(()),
    })
}

/// Login-only reuse must evict old probes and waiting installs, never a busy installation.
#[test]
fn login_reset_cancels_probe_and_waiting_jobs_only() {
    for phase in [
        NewPhase::Probing,
        NewPhase::Keys,
        NewPhase::Stuck {
            status: None,
            error: None,
        },
        NewPhase::Unreachable,
    ] {
        let mut current = job(phase);
        assert!(reset_core_new_for_login(&mut current));
        assert!(current.is_none());
    }

    let mut installing = job(NewPhase::Installing);
    assert!(!reset_core_new_for_login(&mut installing));
    assert!(installing.is_some());
}
