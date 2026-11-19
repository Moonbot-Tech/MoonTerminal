use std::net::Ipv4Addr;

use super::*;

/// The primary address is not an alternative: binding to it would change nothing.
#[test]
fn options_offer_every_address_but_the_primary() {
    let text = "EGRESS_SYSTEMD=257\nEGRESS_PRIMARY=203.0.113.11\n\
                EGRESS_ADDRESS=203.0.113.11\nEGRESS_ADDRESS=203.0.113.12\n\
                EGRESS_ADDRESS=bogus\nEGRESS_ADDRESS=203.0.113.13\n";
    let options = EgressOptions::parse(text);
    assert!(options.supported());
    assert_eq!(options.primary, Some(Ipv4Addr::new(203, 0, 113, 11)));
    assert_eq!(
        options.alternatives(),
        [
            Ipv4Addr::new(203, 0, 113, 12),
            Ipv4Addr::new(203, 0, 113, 13)
        ]
    );
}

/// systemd 254 has no NFTSet=, and a server that did not report a version is not assumed new.
#[test]
fn options_refuse_systemd_before_255_and_an_unreported_version() {
    assert!(!EgressOptions::parse("EGRESS_SYSTEMD=254\n").supported());
    assert!(!EgressOptions::parse("CHECK_OK\n").supported());
    assert!(EgressOptions::parse("EGRESS_SYSTEMD=255\n").supported());
}

/// An unbound core reports an empty address; a bound one out of its set is not `bound`.
#[test]
fn info_reads_address_membership_and_connections() {
    let unbound = EgressInfo::parse("INFO_EGRESS=\nEGRESS_SYSTEMD=257\n");
    assert_eq!(unbound.address, None);
    assert!(!unbound.bound);
    assert!(unbound.options.supported());

    let lost = EgressInfo::parse(
        "INFO_EGRESS=203.0.113.12\nINFO_EGRESS_BOUND=0\nINFO_EGRESS_CONNECTIONS=\n",
    );
    assert_eq!(lost.address, Some(Ipv4Addr::new(203, 0, 113, 12)));
    assert!(!lost.bound);
    assert_eq!(lost.connections, None);

    let bound = EgressInfo::parse(
        "INFO_EGRESS=203.0.113.12\nINFO_EGRESS_BOUND=1\nINFO_EGRESS_CONNECTIONS=42\n",
    );
    assert!(bound.bound);
    assert_eq!(bound.connections, Some(42));
}

/// A binding set up by hand is read as every set it names, and the file the script will not edit
/// as `foreign_shared`; a core without one has neither.
#[test]
fn info_reads_a_foreign_binding() {
    let foreign = EgressInfo::parse(
        "INFO_EGRESS=\n\
         INFO_EGRESS_FOREIGN=cgroup:inet:moon_srcip:via_extra\n\
         INFO_EGRESS_FOREIGN=cgroup:inet:other:set_b\n\
         INFO_EGRESS_FOREIGN_SHARED=/etc/systemd/system/moonbot@.service.d/10-srcip.conf\n",
    );
    assert_eq!(
        foreign.foreign,
        vec![
            "cgroup:inet:moon_srcip:via_extra",
            "cgroup:inet:other:set_b"
        ]
    );
    assert_eq!(
        foreign.foreign_shared.as_deref(),
        Some("/etc/systemd/system/moonbot@.service.d/10-srcip.conf")
    );

    let own = EgressInfo::parse("INFO_EGRESS=203.0.113.12\nINFO_EGRESS_BOUND=1\n");
    assert!(own.foreign.is_empty());
    assert_eq!(own.foreign_shared, None);
}
