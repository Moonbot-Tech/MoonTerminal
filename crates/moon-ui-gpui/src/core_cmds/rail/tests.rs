//! Server state and installation identity must prevent unsafe navigation actions.

use super::available;
use crate::backend::core_cmds::MenuForm;
use moon_remote::core_cmds::Cmd;

/// A hand-installed or still-unidentified core must never offer configuration mutations.
#[test]
fn installation_and_service_state_gate_commands() {
    for form in [MenuForm::Hand, MenuForm::Checking] {
        for cmd in [
            Cmd::SwitchExchange,
            Cmd::ApiKeys,
            Cmd::ConnectionKey,
            Cmd::MoonProtoAddress,
            Cmd::UpdateCore,
        ] {
            assert!(!available(form, None, cmd));
        }
        for cmd in [
            Cmd::Status,
            Cmd::RetryLogin,
            Cmd::Log,
            Cmd::Restart,
            Cmd::Stop,
            Cmd::Start,
        ] {
            assert!(available(form, None, cmd));
            assert!(!available(MenuForm::NoLogin, None, cmd));
        }
    }
    assert!(!available(MenuForm::Ours, Some(true), Cmd::Start));
    assert!(available(MenuForm::Ours, Some(false), Cmd::Start));
    for cmd in [Cmd::Stop, Cmd::Restart] {
        assert!(!available(MenuForm::Ours, Some(false), cmd));
        assert!(available(MenuForm::Ours, Some(true), cmd));
    }
}
