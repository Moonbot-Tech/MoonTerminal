//! Fresh quiet reads must authorize trace mapping without opening Settings.

use super::trace_listing;
use crate::backend::station::cores_sync::{LocalCore, trace_uid};
use moon_core::station_api::{ListedCore, Status};
use moon_remote::station::bot::BotState;

/// A wire-shaped status keeps an unsupported listing distinct from an unavailable API.
fn read(cores: Option<Vec<ListedCore>>) -> BotState {
    let mut status: Status = serde_json::from_str(
        r#"{"station_version":"fixture","cores_ready":1,"cores_total":1,"bot":null}"#,
    )
    .unwrap();
    status.cores = cores;
    BotState {
        station: Some(Box::new(status)),
        ..BotState::default()
    }
}

/// Removing the worker's quiet read would strand unopened Settings or use another core's uid.
#[test]
fn freshly_fetched_listing_maps_station_identity() {
    let here = [LocalCore {
        endpoint_override: String::new(),
        key_address: Some("198.51.100.1:4510".into()),
        uid: 3,
        name: "Local".into(),
        address: Some("198.51.100.1:4510".into()),
        key_fp: "fixture".into(),
    }];
    let listing = trace_listing(None, || {
        Ok(read(Some(vec![ListedCore {
            endpoint_override: None,
            uid: 9,
            name: "Station".into(),
            address: Some("198.51.100.1:4510".into()),
            key_fp: None,
        }])))
    })
    .unwrap();
    assert_eq!(trace_uid(3, &here, Some(listing.as_deref())), Some(9));
    let old = trace_listing(None, || Ok(read(None))).unwrap();
    assert_eq!(trace_uid(3, &here, Some(old.as_deref())), Some(3));
    let empty = trace_listing(None, || Ok(read(Some(vec![])))).unwrap();
    assert_eq!(trace_uid(3, &here, Some(empty.as_deref())), None);
}

/// A stopped or failed read must retry rather than masquerading as an old station.
#[test]
fn unavailable_read_retries_and_cached_listing_needs_no_read() {
    assert!(trace_listing(None, || Ok(BotState::default())).is_err());
    assert!(trace_listing(None, || Err(anyhow::anyhow!("fixture read failure"))).is_err());
    assert_eq!(
        trace_listing(Some(None), || panic!(
            "cached old station must not be fetched"
        ))
        .unwrap(),
        None
    );
}
