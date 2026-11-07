//! Security regressions for browser consent, token binding, rotation and revocation.

use super::*;

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
                        cores: vec![99],
                        scopes: vec![Scope::Read]
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
                    cores: vec![11],
                    scopes: vec![Scope::Read],
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
            cores: vec![11],
            scopes: vec![Scope::Read],
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
    f.0.disk
        .grants
        .values_mut()
        .next()
        .unwrap()
        .info
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
