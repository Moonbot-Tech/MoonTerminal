//! Pure decisions shared by the by-key screens and their actions.

use moon_core::{
    config::{CoreHost, CoreTarget},
    feed::FailureClass,
};

/// Moonbot's default UDP port, also used by legacy keys without a port.
pub(super) const FACTORY_PORT: u16 = 3000;

/// Where the user says Moonbot runs; a missing address is valid only locally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Host {
    #[default]
    WindowsServer,
    ThisComputer,
}

/// The instructions before a connection attempt is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::connect_wizard) enum Step {
    Port,
    Moonbot,
    Key,
}

/// Create the inbound UDP rule for the actual chosen port.
pub(super) fn firewall_command(port: u16) -> String {
    format!(
        "netsh advfirewall firewall add rule name=\"MoonBot-UDP-{port}\" dir=in action=allow protocol=UDP localport={port}"
    )
}

/// Accept a trimmed nonzero UDP port without guessing at malformed input.
pub(in crate::connect_wizard) fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

/// Return the key's port when it differs from the port the user opened.
/// A key without a port resolves to 3000, so a legacy key cannot expose a mismatch
/// when the chosen port is also the factory default.
pub(super) fn port_mismatch(chosen: u16, key: &CoreTarget) -> Option<u16> {
    (chosen != key.port).then_some(key.port)
}

/// Only distinguish failures supported by the retained connection evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FailScreen {
    NoResponse { silent: bool },
    KeyRefused,
    NoAddress,
    Fallback,
}

/// Missing server addresses take precedence; silence requires zero packets AND bytes.
pub(super) fn pick_failure(
    class: Option<&FailureClass>,
    key: Option<&CoreTarget>,
    host: Host,
) -> FailScreen {
    if host == Host::WindowsServer && key.is_some_and(|key| {
        key.localhost_fallback
            || matches!(&key.host, CoreHost::Ip(ip) if ip.is_loopback())
            || matches!(&key.host, CoreHost::Name(name) if name.eq_ignore_ascii_case("localhost"))
    }) {
        return FailScreen::NoAddress;
    }
    match class {
        Some(FailureClass::NoResponse {
            packets_received,
            bytes,
            ..
        }) => FailScreen::NoResponse {
            silent: *packets_received == 0 && *bytes == 0,
        },
        Some(FailureClass::Access { .. }) => FailScreen::KeyRefused,
        _ => FailScreen::Fallback,
    }
}

/// An override is valid only for the exact key and port the user explicitly accepted.
pub(super) fn pending_mismatch(
    accepted: Option<&(String, u16)>,
    key: &str,
    chosen: u16,
    target: Option<&CoreTarget>,
) -> Option<u16> {
    if accepted.is_some_and(|(text, port)| text == key && *port == chosen) {
        None
    } else {
        target.and_then(|target| port_mismatch(chosen, target))
    }
}

/// Round partial seconds up so an observed wait never reads as zero seconds.
pub(super) fn shown_seconds(elapsed_ms: u64) -> u64 {
    elapsed_ms.div_ceil(1000).max(1)
}

/// Ignore an old failed status while an explicit reconnect is still queued.
pub(super) fn should_latch(
    failed: bool,
    has_diag: bool,
    awaiting_reconnect: bool,
    timed_out: bool,
    pick: FailScreen,
) -> bool {
    pick == FailScreen::NoAddress || timed_out || (failed && has_diag && !awaiting_reconnect)
}

/// The footer's current instruction or connection phase, independent of live entities.
#[derive(Clone, Copy)]
pub(super) enum Footer {
    Instructions(Step),
    Connecting,
    Failure(FailScreen),
    Done,
}

/// A footer intent whose button is shared by production and fixture rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Back,
    Next,
    Connect,
    AcceptPort,
    Paste,
    Cancel,
    Drop,
    Retry,
    NewKey,
    SetAddress,
    Again,
    Finish,
}

/// Keep mismatch recovery identical for an initial key and a refused replacement.
pub(super) fn footer_actions(footer: Footer, mismatch: Option<u16>) -> Vec<Action> {
    use Action::*;
    let (left, right) = match footer {
        Footer::Instructions(Step::Port | Step::Moonbot) => (Some(Back), Next),
        Footer::Instructions(Step::Key) => (Some(Back), Connect),
        Footer::Connecting => (None, Cancel),
        Footer::Failure(FailScreen::KeyRefused) => (Some(Drop), NewKey),
        Footer::Failure(FailScreen::NoAddress) => (Some(Drop), SetAddress),
        Footer::Failure(_) => (Some(Drop), Retry),
        Footer::Done => (Some(Again), Finish),
    };
    let mut actions: Vec<_> = left.into_iter().collect();
    if mismatch.is_some() && matches!(right, Connect | NewKey) {
        actions.extend([AcceptPort, Paste]);
    } else {
        actions.push(right);
    }
    actions
}

/// Keep waiting and failure on the key step, reserving the final step for Ready.
pub(in crate::connect_wizard) fn rail_index(step: Step, added: bool, ready: bool) -> usize {
    if ready {
        4
    } else if added {
        3
    } else {
        match step {
            Step::Port => 1,
            Step::Moonbot => 2,
            Step::Key => 3,
        }
    }
}

#[cfg(test)]
mod tests;
