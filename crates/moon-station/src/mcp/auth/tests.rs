//! Security regressions for browser consent, token binding, rotation and revocation.

use super::*;

/// Read permission on each of these cores, the least an approval may grant.
fn read_only(cores: &[u64]) -> Vec<CoreAccess> {
    cores
        .iter()
        .map(|&core| CoreAccess {
            core,
            scopes: vec![Scope::Read],
        })
        .collect()
}

/// Read on core 11 plus these permissions there, as a change request for the fixture's grant.
fn with_on_11(extra: &[Scope]) -> Vec<CoreAccess> {
    let mut scopes = vec![Scope::Read];
    scopes.extend_from_slice(extra);
    vec![CoreAccess { core: 11, scopes }]
}

/// Change the fixture's only grant from what the owner's page currently shows.
fn set_access(f: &mut Fixture, access: Vec<CoreAccess>, at: i64) -> Result<AdminState> {
    let state = f.0.status();
    let grant = &state.grants[0];
    let request = AdminRequest::SetAccess {
        grant_id: grant.id.clone(),
        expected: grant.access.clone(),
        access,
    };
    f.0.admin(request, &[11, 22], at)
}

/// The feature's whole point: a client already holding a token gets the new permissions on its next
/// call, and loses them just as fast. No reconnect, and no window where a narrowed client still acts.
#[test]
fn a_change_of_access_applies_to_the_tokens_already_issued() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    let tokens =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .expect("exchange");
    let who = f.0.bearer(&tokens.access_token, 1004).expect("bearer");
    assert!(f.0.check(&who, Scope::Trading, &[11], 1004).is_err());
    set_access(&mut f, with_on_11(&[Scope::Trading]), 1005).expect("widen");
    assert!(f.0.check(&who, Scope::Trading, &[11], 1006).is_ok());
    set_access(&mut f, with_on_11(&[]), 1007).expect("narrow");
    assert!(f.0.check(&who, Scope::Trading, &[11], 1008).is_err());
    assert!(f.0.check(&who, Scope::Read, &[11], 1008).is_ok());
}

/// Two Terminals can hold one Station. One owner's form must not silently undo what another applied
/// since it was drawn: applying blind could hand back a permission that was just taken away.
#[test]
fn a_change_of_access_is_refused_when_the_permissions_moved_elsewhere() {
    let mut f = Fixture::new();
    f.consent();
    let grant_id = f.0.status().grants[0].id.clone();
    let seen = f.0.status().grants[0].access.clone();
    set_access(&mut f, with_on_11(&[Scope::Trading]), 1005).expect("the other terminal");
    let stale = AdminRequest::SetAccess {
        grant_id,
        expected: seen,
        access: read_only(&[11, 22]),
    };
    assert!(f.0.admin(stale, &[11, 22], 1006).is_err());
    assert_eq!(f.0.status().grants[0].access, with_on_11(&[Scope::Trading]));
}

/// The owner's page must not hand a client more than it asked for when it connected. The fixture's
/// client asked for read and trading, so settings stays out of reach.
#[test]
fn a_change_of_access_cannot_exceed_what_the_client_requested() {
    let mut f = Fixture::new();
    f.consent();
    assert!(set_access(&mut f, with_on_11(&[Scope::Settings]), 1005).is_err());
    assert_eq!(
        f.0.status().grants[0].scopes_on(11),
        Some([Scope::Read].as_slice())
    );
}

/// A grant from before requests were kept has no ceiling to read. Refusing would strand it until
/// a reconnect; the owner's explicit tick is the authority left.
#[test]
fn a_grant_with_no_recorded_request_follows_the_owners_choice() {
    let mut f = Fixture::new();
    f.consent();
    let grant = f.0.disk.grants.values_mut().next().expect("grant");
    grant.info.requested = None;
    set_access(&mut f, with_on_11(&[Scope::Settings]), 1005).expect("owner's choice");
    assert_eq!(
        f.0.status().grants[0].scopes_on(11),
        Some([Scope::Read, Scope::Settings].as_slice())
    );
}

/// The same refusals an approval has: a change must not produce a grant no approval could.
#[test]
fn a_change_of_access_keeps_the_rules_of_an_approval() {
    let mut f = Fixture::new();
    f.consent();
    let write_only = vec![CoreAccess {
        core: 11,
        scopes: vec![Scope::Trading],
    }];
    for access in [write_only, Vec::new(), read_only(&[99])] {
        assert!(set_access(&mut f, access, 1005).is_err());
    }
    let unknown = AdminRequest::SetAccess {
        grant_id: "gone".into(),
        expected: Vec::new(),
        access: read_only(&[11]),
    };
    assert!(f.0.admin(unknown, &[11, 22], 1005).is_err());
    assert_eq!(f.0.status().grants[0].access, read_only(&[11]));
}

/// Widening a client's reach belongs on the record with what it did. Re-sending the same rights is
/// not an event, so the journal stays worth reading.
#[test]
fn a_change_of_access_is_journalled_only_when_something_changed() {
    let mut f = Fixture::new();
    f.consent();
    set_access(&mut f, read_only(&[11]), 1005).expect("same");
    assert!(f.0.status().audit.is_empty());
    set_access(&mut f, with_on_11(&[Scope::Trading]), 1006).expect("widen");
    let journal = f.0.status().audit;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].action, "access_change");
    assert_eq!(journal[0].client_name, "Test AI");
    assert_eq!(journal[0].changes[0].field, "#11");
    assert_eq!(
        journal[0].changes[0].after.as_deref(),
        Some("read, trading")
    );
}

/// A browser's request for read and trading, returning the code its owner would type.
fn request_consent(auth: &mut Auth) -> String {
    let (client, _) = auth
        .register(
            "Test AI".into(),
            vec!["https://ai.example/callback".into()],
            "none".into(),
            1000,
        )
        .expect("register");
    let (_, code) = auth
        .begin(
            client.id.clone(),
            client.redirects[0].clone(),
            hash(&"a".repeat(43)),
            "state-value".into(),
            vec![Scope::Read, Scope::Trading],
            "browser-secret",
            1000,
        )
        .expect("begin");
    code
}

/// The point of per-core access: read everywhere must not turn into trading everywhere, and the
/// flat union a token carries for OAuth must not widen any single core.
#[test]
fn a_permission_on_one_core_does_not_reach_another() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    f.0.disk
        .grants
        .values_mut()
        .next()
        .expect("grant")
        .info
        .access = vec![
        CoreAccess {
            core: 11,
            scopes: vec![Scope::Read, Scope::Trading],
        },
        CoreAccess {
            core: 22,
            scopes: vec![Scope::Read],
        },
    ];
    let tokens =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .expect("exchange");
    assert_eq!(tokens.scope, "read trading");
    let who = f.0.bearer(&tokens.access_token, 1004).expect("bearer");
    assert!(f.0.check(&who, Scope::Trading, &[11], 1004).is_ok());
    assert!(f.0.check(&who, Scope::Read, &[22], 1004).is_ok());
    assert!(f.0.check(&who, Scope::Trading, &[22], 1004).is_err());
    assert!(f.0.check(&who, Scope::Trading, &[11, 22], 1004).is_err());
    assert!(f.0.check(&who, Scope::Read, &[33], 1004).is_err());
    assert!(f.0.check(&who, Scope::Settings, &[], 1004).is_err());
}

/// Every read tool assumes a covered core is readable, so a core granted writes alone must not
/// exist; nor may one core appear twice with two different answers.
#[test]
fn an_approval_needs_read_on_each_core_and_each_core_once() {
    let mut f = Fixture::new();
    let code = request_consent(&mut f.0);
    let approve = |access| AdminRequest::Approve {
        code: code.clone(),
        access,
    };
    let write_only = vec![CoreAccess {
        core: 11,
        scopes: vec![Scope::Trading],
    }];
    assert!(f.0.admin(approve(write_only), &[11, 22], 1001).is_err());
    let twice = [read_only(&[11]), read_only(&[11])].concat();
    assert!(f.0.admin(approve(twice), &[11, 22], 1001).is_err());
    assert!(f.0.admin(approve(Vec::new()), &[11, 22], 1001).is_err());
    assert!(f.0.status().grants.is_empty());
    let mixed = vec![
        CoreAccess {
            core: 22,
            scopes: vec![Scope::Trading, Scope::Read],
        },
        CoreAccess {
            core: 11,
            scopes: vec![Scope::Read],
        },
    ];
    f.0.admin(approve(mixed), &[11, 22], 1001).expect("approve");
    let grant = &f.0.status().grants[0];
    assert_eq!(
        grant.scopes_on(22),
        Some([Scope::Read, Scope::Trading].as_slice())
    );
    assert_eq!(grant.scopes_on(11), Some([Scope::Read].as_slice()));
}

/// A client may hold no more than its request named, on any single core.
#[test]
fn no_core_may_exceed_what_the_client_requested() {
    let mut f = Fixture::new();
    let code = request_consent(&mut f.0);
    let access = vec![
        CoreAccess {
            core: 11,
            scopes: vec![Scope::Read],
        },
        CoreAccess {
            core: 22,
            scopes: vec![Scope::Read, Scope::Settings],
        },
    ];
    assert!(
        f.0.admin(AdminRequest::Approve { code, access }, &[11, 22], 1001)
            .is_err()
    );
}

/// After a restart no operation remains to finish these entries; a state still "in progress" would
/// be false for good. A finished entry is a fact and stays.
#[test]
fn a_restart_marks_entries_left_mid_way_as_unknown() {
    let mut disk = Disk::default();
    for (operation, outcome) in [
        ("a", "admitted"),
        ("b", "admitted:old-id"),
        ("c", "queued_in_runtime"),
        ("d", "confirmed"),
        ("e", "submitted"),
    ] {
        let mut entry = journal_entry(operation, "g");
        entry.outcome = outcome.into();
        disk.audit.push_back(entry);
    }
    disk.settle_interrupted();
    let outcomes: Vec<_> = disk.audit.iter().map(|e| e.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        ["unknown", "unknown", "unknown", "confirmed", "submitted"]
    );
}

/// A journal entry of one operation, as the Station writes it when an action is applied.
fn journal_entry(operation: &str, grant_id: &str) -> AuditEntry {
    AuditEntry {
        at_ms: 1000,
        grant_id: grant_id.into(),
        action: "strategy_edit".into(),
        cores: vec![11],
        outcome: "admitted".into(),
        operation: operation.into(),
        client_name: String::new(),
        subject: Some("Night".into()),
        changes: Vec::new(),
        changes_omitted: 0,
    }
}

/// One action stays one journal line however many results it goes through.
#[test]
fn an_operations_results_update_its_one_journal_entry() {
    let mut fixture = Fixture::new();
    fixture
        .0
        .audit(journal_entry("first", "gone"))
        .expect("first");
    fixture
        .0
        .audit(journal_entry("second", "gone"))
        .expect("second");
    fixture
        .0
        .audit_outcome("first", "confirmed")
        .expect("outcome");
    let journal = fixture.0.status().audit;
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].operation, "second");
    assert_eq!(journal[0].outcome, "admitted");
    assert_eq!(journal[1].outcome, "confirmed");
}

/// The client's name must outlive its grant, or a revoked client's changes turn anonymous exactly
/// when the owner is reading them to see what it did.
#[test]
fn a_journal_entry_keeps_the_clients_name_after_revocation() {
    let mut fixture = Fixture::new();
    let (client, verifier, code) = fixture.consent();
    fixture
        .0
        .exchange(&client.id, &code, &client.redirects[0], &verifier, 1000)
        .expect("exchange");
    let grant = fixture.0.status().grants[0].id.clone();
    fixture
        .0
        .audit(journal_entry("op", &grant))
        .expect("journal");
    fixture
        .0
        .admin(AdminRequest::Revoke { grant_id: grant }, &[11, 22], 1000)
        .expect("revoke");
    assert_eq!(fixture.0.status().audit[0].client_name, "Test AI");
}

/// The whole store can outgrow the frame the owner's page reads it through.
#[test]
fn the_status_reply_carries_only_the_newest_journal_entries() {
    let mut fixture = Fixture::new();
    for index in 0..40 {
        fixture
            .0
            .audit(journal_entry(&format!("op{index}"), "gone"))
            .expect("journal");
    }
    let journal = fixture.0.status().audit;
    assert_eq!(journal.len(), AUDIT_SHOWN);
    assert_eq!(journal[0].operation, "op39");
}

/// A store written by the version already installed on servers must still open: an unreadable
/// store fails closed and would take every client's access with it.
#[test]
fn a_store_from_before_the_change_journal_still_reads() {
    let old = r#"{"resource":"","enabled":true,"clients":{},"grants":{},"tokens":{},
        "audit":[{"at_ms":5,"grant_id":"g","action":"trading","cores":[1],"outcome":"admitted:abc"}]}"#;
    let disk: Disk = serde_json::from_str(old).expect("old store");
    let entry = &disk.audit[0];
    assert_eq!(entry.outcome, "admitted:abc");
    assert!(entry.operation.is_empty() && entry.changes.is_empty());
}

/// Each test owns an isolated credential file and removes it when the test finishes.
struct Fixture(Auth);

impl Fixture {
    /// Start enabled with no grants, as the owner's setup action would leave the service.
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("moon-mcp-auth-{}.json", random().expect("random")));
        let mut auth = Auth::open(path).expect("open");
        auth.admin(AdminRequest::SetEnabled { enabled: true }, &[11, 22], 1000)
            .expect("enable");
        Self(auth)
    }

    /// Complete the owner/browser handshake and return a client, PKCE verifier and single-use code.
    fn consent(&mut self) -> (Client, String, String) {
        let (client, _) = self
            .0
            .register(
                "Test AI".into(),
                vec!["https://ai.example/callback".into()],
                "none".into(),
                1000,
            )
            .expect("register");
        let verifier = "a".repeat(43);
        let (request, code) = self
            .0
            .begin(
                client.id.clone(),
                client.redirects[0].clone(),
                hash(&verifier),
                "state-value".into(),
                vec![Scope::Read, Scope::Trading],
                "browser-secret",
                1000,
            )
            .expect("begin");
        assert!(
            self.0
                .continue_browser(&request, "other-browser", 1001)
                .is_err()
        );
        assert!(
            self.0
                .continue_browser(&request, "browser-secret", 1001)
                .expect("pending")
                .is_none()
        );
        assert!(
            self.0
                .admin(
                    AdminRequest::Approve {
                        code: code.clone(),
                        access: read_only(&[99]),
                    },
                    &[11, 22],
                    1001
                )
                .is_err()
        );
        self.0
            .admin(
                AdminRequest::Approve {
                    code,
                    access: read_only(&[11]),
                },
                &[11, 22],
                1001,
            )
            .expect("approve");
        let redirect = Url::parse(
            &self
                .0
                .continue_browser(&request, "browser-secret", 1002)
                .expect("redirect")
                .expect("approved"),
        )
        .expect("URL");
        let query: BTreeMap<_, _> = redirect.query_pairs().into_owned().collect();
        assert_eq!(query.get("state").map(String::as_str), Some("state-value"));
        (client, verifier, query["code"].clone())
    }
}

impl Drop for Fixture {
    /// Test credentials never survive the fixture.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0.path);
    }
}

/// A valid code cannot be stolen through another browser, redirect, verifier or core selection.
#[test]
fn consent_is_bound_to_browser_redirect_pkce_and_exact_core_scope() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    assert!(
        f.0.exchange(
            &client.id,
            &code,
            "https://evil.example/callback",
            &verifier,
            1003
        )
        .is_err()
    );
    assert!(
        f.0.exchange(
            &client.id,
            &code,
            &client.redirects[0],
            &"b".repeat(43),
            1003
        )
        .is_err()
    );
    let tokens =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .expect("exchange");
    assert!(
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1004)
            .is_err()
    );
    let who = f.0.bearer(&tokens.access_token, 1004).expect("bearer");
    assert!(f.0.check(&who, Scope::Read, &[11], 1004).is_ok());
    assert!(f.0.check(&who, Scope::Read, &[22], 1004).is_err());
    assert!(f.0.check(&who, Scope::Trading, &[11], 1004).is_err());
    assert!(f.0.bearer(&tokens.refresh_token, 1004).is_err());
    assert!(
        f.0.check(&who, Scope::Read, &[11], 1003 + ACCESS_MS)
            .is_err()
    );
    let persisted = std::fs::read_to_string(&f.0.path).expect("stored");
    assert!(!persisted.contains(&tokens.access_token));
    assert!(!persisted.contains(&tokens.refresh_token));
}

/// Replaying a consumed refresh revokes both issued tokens and work that has not executed yet.
#[test]
fn refresh_replay_revokes_new_tokens_and_already_queued_principals() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    let first =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .expect("exchange");
    let queued =
        f.0.bearer(&first.access_token, 1004)
            .expect("queued request");
    let second =
        f.0.refresh(&client.id, &first.refresh_token, 1005)
            .expect("refresh");
    assert!(f.0.bearer(&second.access_token, 1006).is_ok());
    assert!(f.0.refresh(&client.id, &first.refresh_token, 1006).is_err());
    assert!(f.0.bearer(&second.access_token, 1006).is_err());
    assert!(f.0.check(&queued, Scope::Read, &[11], 1006).is_err());
    let mut reopened = Auth::open(f.0.path.clone()).expect("restart");
    assert!(reopened.bearer(&first.access_token, 1006).is_err());
}

/// A long-lived client keeps bounded state, and forged lineage cannot revoke a legitimate grant.
#[test]
fn refresh_lineage_survives_restart_without_accumulating_tombstones() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    let first =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .unwrap();
    let mut current = first.refresh_token.clone();
    let mut at = 1003;
    for _ in 0..100 {
        at += ACCESS_MS;
        current = f.0.refresh(&client.id, &current, at).unwrap().refresh_token;
    }
    assert_eq!(f.0.disk.tokens.len(), 1);
    let mut reopened = Auth::open(f.0.path.clone()).unwrap();
    let mut forged = current.clone();
    let signature = forged.rfind('.').unwrap() + 1;
    forged.replace_range(
        signature..signature + 1,
        if &forged[signature..signature + 1] == "A" {
            "B"
        } else {
            "A"
        },
    );
    assert!(reopened.refresh(&client.id, &forged, at + 1).is_err());
    let valid = reopened.refresh(&client.id, &current, at + 1).unwrap();
    assert!(
        reopened
            .refresh(&client.id, &first.refresh_token, at + 2)
            .is_err()
    );
    assert!(reopened.bearer(&valid.access_token, at + 2).is_err());
}

/// A valid consent window must not outlive its client registration binding.
#[test]
fn pending_consent_retains_client_until_owner_finishes() {
    let mut f = Fixture::new();
    let (client, _) =
        f.0.register(
            "AI".into(),
            vec!["https://ai.example/callback".into()],
            "none".into(),
            1000,
        )
        .unwrap();
    let verifier = "x".repeat(43);
    let (request, owner_code) =
        f.0.begin(
            client.id.clone(),
            client.redirects[0].clone(),
            hash(&verifier),
            String::new(),
            vec![Scope::Read],
            "browser",
            CONSENT_MS,
        )
        .unwrap();
    f.0.admin(
        AdminRequest::Approve {
            code: owner_code,
            access: read_only(&[11]),
        },
        &[11],
        CONSENT_MS + 2000,
    )
    .unwrap();
    f.0.authenticate_client(&client.id, None, "none").unwrap();
    let url = Url::parse(
        &f.0.continue_browser(&request, "browser", CONSENT_MS + 2001)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let code = url
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned();
    assert!(
        f.0.exchange(
            &client.id,
            &code,
            &client.redirects[0],
            &verifier,
            CONSENT_MS + 2002
        )
        .is_ok()
    );
}

/// Runtime restart starts checked strategies, so a settings-only grant must not authorize it.
#[test]
fn settings_permission_does_not_authorize_runtime_trading_start() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    f.0.disk.grants.values_mut().next().unwrap().info.access[0]
        .scopes
        .push(Scope::Settings);
    let tokens =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .unwrap();
    let who = f.0.bearer(&tokens.access_token, 1004).unwrap();
    assert!(f.0.check(&who, Scope::Settings, &[11], 1004).is_ok());
    assert!(
        f.0.check(
            &who,
            moon_core::automation::Action::RuntimeRestart.scope(),
            &[11],
            1004
        )
        .is_err()
    );
    assert_eq!(f.0.status().grants[0].last_access_ms, Some(1004));
}

/// Re-enabling after restart requires fresh owner consent rather than reviving previous authority.
#[test]
fn disabling_access_survives_restart_and_cannot_resurrect_old_tokens() {
    let mut f = Fixture::new();
    let (client, verifier, code) = f.consent();
    let tokens =
        f.0.exchange(&client.id, &code, &client.redirects[0], &verifier, 1003)
            .expect("exchange");
    f.0.admin(AdminRequest::SetEnabled { enabled: false }, &[11], 1004)
        .expect("disable");
    let mut restarted = Auth::open(f.0.path.clone()).expect("restart");
    restarted
        .admin(AdminRequest::SetEnabled { enabled: true }, &[11], 1005)
        .expect("enable");
    assert!(restarted.bearer(&tokens.access_token, 1005).is_err());
    assert!(
        restarted
            .refresh(&client.id, &tokens.refresh_token, 1005)
            .is_err()
    );
}

/// Public registration cannot send an authorization code over an unsafe callback.
#[test]
fn registrations_cannot_introduce_insecure_or_fragment_redirects() {
    let mut f = Fixture::new();
    for redirect in [
        "http://ai.example/cb",
        "https://ai.example/cb#fragment",
        "https://user:password@ai.example/cb",
        "file:///tmp/token",
    ] {
        assert!(
            f.0.register("AI".into(), vec![redirect.into()], "none".into(), 1000)
                .is_err()
        );
    }
    assert!(
        f.0.register(
            "AI".into(),
            vec!["http://127.0.0.1:8912/callback".into()],
            "none".into(),
            1000
        )
        .is_ok()
    );
}
