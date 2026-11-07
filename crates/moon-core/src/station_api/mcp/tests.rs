//! Grants on the wire and on disk: what the owner approved must survive both exactly.

use super::*;
use crate::station_api::Request;

/// A store holding the one-list shape is on every server already running MCP. A wrong migration
/// either locks its clients out or hands a permission to a core it never covered.
#[test]
fn a_grant_from_before_per_core_access_keeps_what_was_approved() {
    let old = r#"{"id":"g","client_name":"Claude","cores":[1,3],"scopes":["read","trading"],
        "created_ms":5,"last_access_ms":null}"#;
    let grant: GrantInfo = serde_json::from_str(old).expect("old grant");
    assert_eq!(
        grant.scopes_on(1),
        Some([Scope::Read, Scope::Trading].as_slice())
    );
    assert_eq!(
        grant.scopes_on(3),
        Some([Scope::Read, Scope::Trading].as_slice())
    );
    assert_eq!(grant.scopes_on(2), None);
    // What it asked for was not kept then; inventing a ceiling would strand its owner's choice.
    assert_eq!(grant.requested, None);
}

/// Rights differ per core: a save and a reload must not flatten or lose them.
#[test]
fn per_core_access_survives_a_save_and_reload() {
    let grant = GrantInfo {
        id: "g".into(),
        client_name: "Claude".into(),
        access: vec![
            CoreAccess {
                core: 1,
                scopes: vec![Scope::Read],
            },
            CoreAccess {
                core: 3,
                scopes: vec![Scope::Read, Scope::Trading],
            },
        ],
        created_ms: 5,
        last_access_ms: None,
        requested: Some(vec![Scope::Read, Scope::Trading]),
    };
    let saved = serde_json::to_string(&grant).expect("save");
    let loaded: GrantInfo = serde_json::from_str(&saved).expect("reload");
    assert_eq!(loaded, grant);
    assert_eq!(loaded.scopes_on(1), Some([Scope::Read].as_slice()));
    assert_eq!(loaded.all_scopes(), vec![Scope::Read, Scope::Trading]);
}

/// The consent crosses two internally tagged enums, which cannot read a map with integer keys.
/// Keying `access` by core would compile and then fail every approval on a real Station.
#[test]
fn a_consent_crosses_the_control_channel() {
    let request = Request::Mcp(AdminRequest::Approve {
        code: "ABC".into(),
        access: vec![CoreAccess {
            core: 7,
            scopes: vec![Scope::Read, Scope::Settings],
        }],
    });
    let text = serde_json::to_string(&request).expect("encode");
    let read: Request = serde_json::from_str(&text).expect("decode");
    assert_eq!(read, request);
}
