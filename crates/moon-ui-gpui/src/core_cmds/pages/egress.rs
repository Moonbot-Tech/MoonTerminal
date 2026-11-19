//! The core's outgoing address: what it is now, whether the core really leaves from it, and the
//! server's other addresses to bind it to.

use moon_remote::egress::{EgressInfo, MIN_SYSTEMD};

use super::*;

/// Show the binding from the latest `info`, and offer a change only where the server can make it.
pub(super) fn build(facts: &CoreFacts, view: &ViewState) -> Page {
    let running = matches!(phase(facts, view), CmdPhase::Running { .. });
    let mut page = Page::new(Kind::Egress, title(view.cmd))
        .block(Block::Text(t!("core_cmds.egress.sub").to_string()));
    let Some(info) = facts.info.as_ref() else {
        if running {
            page.body.push(Block::State(
                Tone::Run,
                t!("core_cmds.service.running").to_string(),
                String::new(),
            ));
        }
        return page;
    };
    let egress = &info.egress;
    // Info the window made up itself (a core that did not answer) carries no server facts: say
    // so rather than read its zero systemd version as an old one.
    if egress.options.systemd == 0 {
        page.body
            .push(Block::Text(t!("core_cmds.egress.unread").to_string()));
        return with_running(page, running);
    }
    let active = info.located.active == Some(true);
    page.body.push(Block::Kv(now_rows(egress, active)));
    // A binding set up by hand: its rules, not ours, decide the address, so the page names the
    // set instead of claiming "primary", and says what a pick will do to it.
    if !egress.foreign.is_empty() {
        let set = foreign_label(egress);
        let why = match &egress.foreign_shared {
            Some(file) => t!(
                "core_cmds.egress.foreign_shared_why",
                set = set,
                file = file
            ),
            None => t!("core_cmds.egress.foreign_why", set = set),
        };
        page.body.push(Block::State(
            Tone::Warn,
            t!("core_cmds.egress.foreign").to_string(),
            why.to_string(),
        ));
    }
    if let Some(address) = egress.address
        && !egress.present
    {
        // The rules drop every connection of a core whose address is gone.
        page.body.push(Block::State(
            Tone::Bad,
            t!("core_cmds.egress.lost", address = address.to_string()).to_string(),
            t!("core_cmds.egress.lost_why").to_string(),
        ));
    } else if egress.address.is_some() && active && !egress.bound {
        // A bound core outside its set leaves from the primary address while it reads as bound.
        page.body.push(Block::State(
            Tone::Bad,
            t!("core_cmds.egress.unbound").to_string(),
            t!("core_cmds.egress.unbound_why").to_string(),
        ));
    }
    if !active {
        page.body
            .push(Block::Text(t!("core_cmds.egress.stopped").to_string()));
    }
    if !egress.options.supported() {
        page.body.push(Block::State(
            Tone::Warn,
            t!(
                "core_cmds.egress.old_systemd",
                found = egress.options.systemd,
                min = MIN_SYSTEMD
            )
            .to_string(),
            t!("core_cmds.egress.old_systemd_why").to_string(),
        ));
    } else if egress.foreign_shared.is_none() {
        let choices = choices(egress, active);
        // A foreign binding is switched off by "primary" alone, so even one row is a choice.
        if choices.len() < 2 && egress.foreign.is_empty() {
            page.body
                .push(Block::Text(t!("core_cmds.egress.single").to_string()));
        } else {
            page.body.push(Block::EgressChoices(choices));
            page.body
                .push(Block::Text(t!("core_cmds.egress.more").to_string()));
        }
    }
    if let Some(Confirm::Egress(address)) = &view.confirm {
        page.body.push(Block::Confirm {
            danger: false,
            title: t!(
                "core_cmds.egress.confirm",
                address = label(*address, egress)
            )
            .to_string(),
            // A stopped core is not restarted: it takes the address on its next start.
            why: confirm_why(egress, active),
            buttons: vec![
                Button::new(
                    if active {
                        "core_cmds.egress.do"
                    } else {
                        "core_cmds.addr.apply"
                    },
                    Action::Confirm,
                )
                .primary()
                .enabled(!running),
                Button::new("core_cmds.common.cancel", Action::CancelConfirm),
            ],
        });
    }
    with_running(page, running)
}

/// Close a page with the running state while the command is under way.
fn with_running(mut page: Page, running: bool) -> Page {
    if running {
        page.body.push(Block::State(
            Tone::Run,
            t!("core_cmds.service.running").to_string(),
            String::new(),
        ));
    }
    page
}

/// What the confirmation says happens: the restart (or the next start of a stopped core), and
/// that a binding set up by hand is switched off.
fn confirm_why(egress: &EgressInfo, active: bool) -> String {
    let mut why = if active {
        t!("core_cmds.egress.confirm_why")
    } else {
        t!("core_cmds.egress.stopped")
    }
    .to_string();
    if !egress.foreign.is_empty() {
        why.push(' ');
        why.push_str(&t!(
            "core_cmds.egress.foreign_replace",
            set = foreign_label(egress)
        ));
    }
    why
}

/// The sets of a foreign binding as the page names them: `table/set` of each
/// `cgroup:<family>:<table>:<set>` value, any other value as it is.
fn foreign_label(egress: &EgressInfo) -> String {
    egress
        .foreign
        .iter()
        .map(|spec| {
            let parts: Vec<&str> = spec.split(':').collect();
            match parts.as_slice() {
                [_, _, table, set] => format!("{table}/{set}"),
                _ => spec.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The current address, and for a bound running core how many connections its address carried.
/// A foreign binding is named by its sets: our own record says nothing about it.
fn now_rows(egress: &EgressInfo, active: bool) -> Vec<(&'static str, String)> {
    if !egress.foreign.is_empty() {
        let set = foreign_label(egress);
        return vec![(
            "core_cmds.egress.now",
            t!("core_cmds.egress.foreign_now", set = set).to_string(),
        )];
    }
    let mut rows = vec![("core_cmds.egress.now", label(egress.address, egress))];
    if egress.address.is_some()
        && active
        && egress.bound
        && let Some(connections) = egress.connections
    {
        rows.push(("core_cmds.egress.connections", connections.to_string()));
    }
    rows
}

/// "Primary" plus every other address of the main interface, and a recorded address the server
/// no longer has — shown so the user sees why the core is cut off and can pick another. Each row
/// says whether it is current and whether it can be picked: the current one only to repair a
/// running core that sits outside its rules, which rewrites them without changing the address.
/// Under a foreign binding no row is current — its address is unknown — and every one replaces it.
fn choices(
    egress: &EgressInfo,
    active: bool,
) -> Vec<(Option<std::net::Ipv4Addr>, String, bool, bool)> {
    let repair = egress.address.is_some() && egress.present && active && !egress.bound;
    let foreign = !egress.foreign.is_empty();
    let row = |address: Option<std::net::Ipv4Addr>| {
        let current = !foreign && egress.address == address;
        (address, label(address, egress), current, !current || repair)
    };
    let mut out = vec![row(None)];
    let mut addresses = egress.options.alternatives();
    if let Some(current) = egress.address
        && !addresses.contains(&current)
    {
        addresses.push(current);
    }
    out.extend(addresses.into_iter().map(|a| row(Some(a))));
    out
}

/// An address as the page names it: the primary one carries its own address when known.
fn label(address: Option<std::net::Ipv4Addr>, egress: &EgressInfo) -> String {
    match (address, egress.options.primary) {
        (Some(address), _) => address.to_string(),
        (None, Some(primary)) => {
            t!("core_cmds.egress.primary", address = primary.to_string()).to_string()
        }
        (None, None) => t!("core_cmds.egress.primary_unknown").to_string(),
    }
}
