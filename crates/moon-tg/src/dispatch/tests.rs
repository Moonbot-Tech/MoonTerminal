//! Source contract on the Mini App session check.

/// `dispatch.rs:mini_request` must reject both a disabled Mini App and a chat absent from the
/// current paired set, while `telegram::web::App::handle_session` maps that rejection to 403;
/// otherwise a stale bound server can keep returning a successful session after a user disables
/// Mini App access or revokes the chat.
#[test]
fn mini_app_session_guard_rechecks_both_live_authorization_conditions() {
    let dispatch = include_str!("../dispatch.rs").replace("\r\n", "\n");
    let guard = "if !telegram.mini_app_enabled || !telegram.authorized_chat_ids.contains(&chat_id) {\n                let _ = reply.try_send(Err(MiniAppApiError::Rejected));\n                return;\n            }";
    assert!(
        dispatch.contains(guard),
        "the session check must reject a disabled Mini App OR a now-revoked chat before replying OK"
    );

    let http = include_str!("../../../moon-core/src/telegram/web.rs");
    assert!(
        http.contains("Ok(Err(_)) => status_response(StatusCode::FORBIDDEN, \"rejected\")"),
        "a host rejection must become a 403 response rather than the old timeout class"
    );
}
