use super::ChartFollow;
use crate::panels::core_status::model::ServerKey;

fn server(octet: u8) -> ServerKey {
    ServerKey::Address(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
        203, 0, 113, octet,
    )))
}

/// A core picked after a server heading takes the chart, and a heading clicked after the core
/// takes it back. Breaks when either pick stops overriding the other.
#[test]
fn the_latest_pick_wins_between_a_server_and_a_core() {
    let mut follow = ChartFollow::default();
    assert!(follow.pick_server(server(1)));
    assert!(follow.pick_core(7, Some(server(2))));
    assert_eq!((follow.core(), follow.server()), (Some(7), Some(server(2))));
    assert!(follow.pick_server(server(1)));
    assert_eq!((follow.core(), follow.server()), (None, Some(server(1))));
    assert!(
        !follow.pick_server(server(1)),
        "the same heading again is not a change"
    );
}

/// A rail switch takes the chart once; the unchanged rail on the next sample does not undo a
/// row picked after it, and a rail that moves again takes it back.
#[test]
fn the_rail_moves_the_chart_only_when_it_changes() {
    let mut follow = ChartFollow::default();
    assert!(follow.follow_rail(Some(3), Some(server(1))));
    assert_eq!(follow.core(), Some(3));

    assert!(follow.pick_core(4, Some(server(1))));
    assert!(
        !follow.follow_rail(Some(3), Some(server(1))),
        "same rail, next sample"
    );
    assert_eq!(
        follow.core(),
        Some(4),
        "the later row pick survives the resample"
    );

    assert!(follow.follow_rail(Some(5), Some(server(2))));
    assert_eq!((follow.core(), follow.server()), (Some(5), Some(server(2))));
}

/// Back to Overview names no core, so the last pick stays charted; picking the same core again
/// from the rail after Overview still counts as a rail move.
#[test]
fn overview_keeps_the_last_pick() {
    let mut follow = ChartFollow::default();
    follow.follow_rail(Some(3), None);
    assert!(!follow.follow_rail(None, None));
    assert_eq!(follow.core(), Some(3));
    follow.pick_core(9, None);
    assert!(follow.follow_rail(Some(3), None));
    assert_eq!(follow.core(), Some(3));
}

/// A core whose server is not known yet keeps the highlighted heading instead of clearing it.
#[test]
fn an_unknown_server_keeps_the_previous_heading() {
    let mut follow = ChartFollow::default();
    follow.pick_server(server(1));
    follow.pick_core(2, None);
    assert_eq!(follow.server(), Some(server(1)));
}
