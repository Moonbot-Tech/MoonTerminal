use super::{
    CoreHost, EndpointOverride, InvalidEndpoint, parse_endpoint_override, station_endpoint,
};

/// The station gets the override only where it is ticked: without the gate a LAN address typed for
/// this terminal reaches a station on a VPS that cannot dial it (#616).
#[test]
fn the_station_gets_only_a_ticked_override() {
    assert_eq!(
        station_endpoint("core.example.net:5017", true),
        "core.example.net:5017"
    );
    assert_eq!(station_endpoint("192.168.1.5", false), "");
    assert_eq!(station_endpoint("", true), "");
}

/// Parse `text` that must be accepted.
fn ok(text: &str) -> EndpointOverride {
    parse_endpoint_override(text)
        .unwrap_or_else(|_| panic!("{text:?} must parse"))
        .unwrap_or_else(|| panic!("{text:?} must not be empty"))
}

fn ip(text: &str) -> Option<CoreHost> {
    Some(CoreHost::Ip(text.parse().unwrap()))
}

/// An empty field is "follow the key". Reading blanks as an error would paint every untouched row
/// red; reading them as an override would replace every key's address with nothing.
#[test]
fn a_blank_field_follows_the_key() {
    for text in ["", "   ", "\t"] {
        assert_eq!(parse_endpoint_override(text), Ok(None), "{text:?}");
    }
}

/// Each accepted form keeps exactly the parts it names; the others come from the key.
#[test]
fn each_form_overrides_only_what_it_names() {
    assert_eq!(
        ok("192.168.1.5"),
        EndpointOverride {
            host: ip("192.168.1.5"),
            port: None
        }
    );
    assert_eq!(
        ok(" 192.168.1.5:5020 "),
        EndpointOverride {
            host: ip("192.168.1.5"),
            port: Some(5020)
        }
    );
    assert_eq!(
        ok(":5020"),
        EndpointOverride {
            host: None,
            port: Some(5020)
        }
    );
    assert_eq!(
        ok("Core.Example.NET:5017"),
        EndpointOverride {
            host: Some(CoreHost::Name("core.example.net".into())),
            port: Some(5017)
        }
    );
    assert_eq!(
        ok("minipc"),
        EndpointOverride {
            host: Some(CoreHost::Name("minipc".into())),
            port: None
        }
    );
}

/// A bare IPv6 address must not be split at its last colon into a host and a port, and a bracketed
/// one carries its port unambiguously.
#[test]
fn ipv6_is_read_whole_or_bracketed() {
    assert_eq!(ok("::1").host, ip("::1"));
    assert_eq!(ok("::1").port, None);
    assert_eq!(ok("2001:db8::1").port, None);
    assert_eq!(
        ok("[2001:db8::1]:5020"),
        EndpointOverride {
            host: ip("2001:db8::1"),
            port: Some(5020)
        }
    );
    assert_eq!(ok("[2001:db8::1]").port, None);
}

/// Every one of these would connect somewhere the user did not mean, or nowhere, so it must turn
/// the field red instead. The numeric ones matter most: the system resolver accepts `192.168.1` as
/// an address of its own.
#[test]
fn typos_are_rejected_not_reinterpreted() {
    for text in [
        "192.168.1.300",
        "192.168.1",
        "10.0.0.1:0",
        "10.0.0.1:65536",
        "10.0.0.1:+5",
        "10.0.0.1:",
        ":",
        ":0",
        "2001:db8::1:5020x",
        "[2001:db8::1]:0",
        "[not-v6]",
        "host name",
        "-bad.example",
        "bad-.example",
        "a..b",
        "http://core.example",
        "core_1.example",
    ] {
        assert_eq!(
            parse_endpoint_override(text),
            Err(InvalidEndpoint),
            "{text:?}"
        );
    }
}
