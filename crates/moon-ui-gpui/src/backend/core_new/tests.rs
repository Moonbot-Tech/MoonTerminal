use super::{NewJob, NewPhase, reset_core_new_for_login, unique_name};
use gpui::Task;

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
