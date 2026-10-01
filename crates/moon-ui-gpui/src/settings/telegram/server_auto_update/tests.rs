//! The switch's view of a synthetic station status.

use super::AutoUpdateView;
use moon_core::station_api::Status;

/// A status as a station of version 2 sends it, with `auto_update` as given.
fn status(auto_update: Option<bool>) -> Status {
    Status {
        station_version: "v0.31.0 (abc)".into(),
        cores_ready: 1,
        cores_total: 1,
        bot: None,
        tape: None,
        host: None,
        last_update: None,
        auto_update,
    }
}

/// The switch shows the station's own value, greys out for a station older than the switch, and
/// never claims "on" for a station it has not read — showing a guess as the station's value would
/// let the user believe updates run where they do not.
#[test]
fn the_switch_follows_the_stations_status() {
    assert_eq!(
        AutoUpdateView::of(Some(&status(Some(true)))),
        AutoUpdateView::Known(true)
    );
    assert_eq!(
        AutoUpdateView::of(Some(&status(Some(false)))),
        AutoUpdateView::Known(false)
    );
    assert_eq!(
        AutoUpdateView::of(Some(&status(None))),
        AutoUpdateView::OldStation
    );
    assert_eq!(AutoUpdateView::of(None), AutoUpdateView::Unread);
}
