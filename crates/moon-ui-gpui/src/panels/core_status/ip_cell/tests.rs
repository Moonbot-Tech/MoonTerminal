//! Regression proofs for the By-IP cell resolver and its column-control affordance.

use std::net::{IpAddr, Ipv4Addr};

use super::{IpCell, ip_cell, mask_affordance};

/// Documentation-range endpoint used to cover address-present resolver states safely.
const ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 17));

/// `ip_cell.rs:ip_cell` must keep `None` ahead of a panel-wide mask; testing `masked` first would
/// render an unknown endpoint as `************`, falsely claiming the panel has an address to hide.
#[test]
fn an_unknown_endpoint_stays_unknown_while_the_column_is_masked() {
    assert_eq!(ip_cell(None, true), IpCell::Unknown);
}

/// `ip_cell.rs:ip_cell` must retain its unmasked present-address arm; returning `Unknown` here
/// would make a known server address disappear from the By-IP column.
#[test]
fn a_present_endpoint_is_shown_while_the_column_is_unmasked() {
    assert_eq!(ip_cell(Some(ADDRESS), false), IpCell::Shown(ADDRESS));
}

/// `ip_cell.rs:ip_cell` must retain its masked present-address arm; showing the address here would
/// reveal every known endpoint when the user has hidden the column for screen sharing.
#[test]
fn a_present_endpoint_is_masked_while_the_column_is_masked() {
    assert_eq!(ip_cell(Some(ADDRESS), true), IpCell::Masked);
}

/// `ip_cell.rs:ip_cell` must retain its unmasked absent-address arm; masking the missing value
/// here would falsely imply that an unknown endpoint has an address to reveal.
#[test]
fn an_unknown_endpoint_stays_unknown_while_the_column_is_unmasked() {
    assert_eq!(ip_cell(None, false), IpCell::Unknown);
}

/// `ip_cell.rs:mask_affordance` must not swap the two branch tuples; a swapped pair makes the only
/// column control offer the opposite action to every server in the panel.
#[test]
fn the_mask_control_names_the_action_that_its_click_will_take() {
    assert_eq!(
        mask_affordance(false),
        ("icons/eye-off.svg", "core_status.hide_ip")
    );
    assert_eq!(
        mask_affordance(true),
        ("icons/eye.svg", "core_status.show_ip")
    );
}
