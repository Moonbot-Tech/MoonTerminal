//! Menu lifecycle regressions use a fake transport; no Telegram or tunnel is started.

use super::*;

/// Build a public tunnel observation without starting the Mini App owner.
fn tunnel(url: &str) -> MiniAppStatus {
    MiniAppStatus::Tunneling {
        port: 1234,
        url: url.into(),
    }
}

/// Capture the same published intent that the bot consumes after its current long poll.
fn intent(chats: &[i64], enabled: bool, status: MiniAppStatus, label: &str) -> MenuIntent {
    let shared = Arc::new(Mutex::new(MenuIntent::stopped(chats)));
    MenuIntent::publish(
        &shared,
        chats,
        enabled,
        &status,
        &BTreeMap::from([("menu_miniapp".into(), label.into())]),
    );
    let snapshot = shared.lock().unwrap().clone();
    snapshot
}

/// URL rotation and locale changes must replace the remote link without duplicate writes.
#[test]
fn current_tunnel_replaces_menu_for_private_paired_chats() {
    let mut sync = MenuSync::default();
    let now = Instant::now();
    let mut sent = Vec::new();
    for desired in [
        intent(
            &[7, -9, 0],
            true,
            tunnel("https://first.trycloudflare.com"),
            "Open",
        ),
        intent(
            &[7],
            true,
            tunnel("https://first.trycloudflare.com"),
            "Open",
        ),
        intent(
            &[7, 8],
            true,
            tunnel("https://second.trycloudflare.com"),
            "Open",
        ),
        intent(
            &[7, 8],
            true,
            tunnel("https://second.trycloudflare.com"),
            "Abrir",
        ),
    ] {
        sync.sync(&desired, now, |chat, button| {
            sent.push((chat, serde_json::to_value(button).unwrap()));
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(
        sent,
        vec![
            (
                7,
                serde_json::json!({"type":"web_app","text":"Open","web_app":{"url":"https://first.trycloudflare.com"}})
            ),
            (
                7,
                serde_json::json!({"type":"web_app","text":"Open","web_app":{"url":"https://second.trycloudflare.com"}})
            ),
            (
                8,
                serde_json::json!({"type":"web_app","text":"Open","web_app":{"url":"https://second.trycloudflare.com"}})
            ),
            (
                7,
                serde_json::json!({"type":"web_app","text":"Abrir","web_app":{"url":"https://second.trycloudflare.com"}})
            ),
            (
                8,
                serde_json::json!({"type":"web_app","text":"Abrir","web_app":{"url":"https://second.trycloudflare.com"}})
            ),
        ]
    );
}

/// Disable, tunnel failure and removed pairing must explicitly override the old web-app menu.
#[test]
fn unavailable_or_removed_targets_clear_the_previous_link() {
    for unavailable in [
        intent(&[7], false, tunnel("https://old.trycloudflare.com"), "Open"),
        intent(&[7], true, MiniAppStatus::Stopped, "Open"),
        intent(
            &[7],
            true,
            MiniAppStatus::Failed {
                reason: super::super::mini_app::MiniAppFailReason::Tunnel,
            },
            "Open",
        ),
        intent(&[], true, tunnel("https://old.trycloudflare.com"), "Open"),
    ] {
        let mut sync = MenuSync::default();
        let now = Instant::now();
        let ready = intent(&[7], true, tunnel("https://old.trycloudflare.com"), "Open");
        sync.sync(&ready, now, |_, _| Ok(())).unwrap();
        let mut sent = Vec::new();
        sync.sync(&unavailable, now, |chat, button| {
            sent.push((chat, serde_json::to_value(button).unwrap()));
            Ok(())
        })
        .unwrap();
        assert_eq!(sent, vec![(7, serde_json::json!({"type":"commands"}))]);
    }
}

/// A timeout may mean the old write reached Telegram; recovery must apply the newest intent.
#[test]
fn failed_write_retries_latest_intent_after_cooldown() {
    let mut sync = MenuSync::default();
    let now = Instant::now();
    let ready = intent(&[7], true, tunnel("https://old.trycloudflare.com"), "Open");
    sync.sync(&MenuIntent::stopped(&[7]), now, |_, _| Ok(()))
        .unwrap();
    assert_eq!(
        sync.sync(&ready, now, |_, _| Err(ApiError::Timeout)),
        Err(ApiError::Timeout)
    );
    let disabled = MenuIntent::stopped(&[7]);
    sync.sync(&disabled, now, |_, _| {
        panic!("failed writes must not hot-loop")
    })
    .unwrap();
    let mut sent = Vec::new();
    sync.sync(&disabled, now + Duration::from_secs(31), |chat, button| {
        sent.push((chat, serde_json::to_value(button).unwrap()));
        Ok(())
    })
    .unwrap();
    assert_eq!(sent, vec![(7, serde_json::json!({"type":"commands"}))]);
    sync.sync(&disabled, now + Duration::from_secs(32), |_, _| {
        panic!("acknowledged state must not repeat")
    })
    .unwrap();
}

/// Startup must clear persisted links even when the new process has no applied-state cache.
#[test]
fn startup_clears_previous_process_menu_before_tunnel_is_ready() {
    let mut sync = MenuSync::default();
    let mut sent = Vec::new();
    sync.sync(
        &MenuIntent::stopped(&[7]),
        Instant::now(),
        |chat, button| {
            sent.push((chat, serde_json::to_value(button).unwrap()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(sent, vec![(7, serde_json::json!({"type":"commands"}))]);
}

/// Restarted transport must clear revoked identities without adding them to current recipients.
#[test]
fn restarted_service_clears_retired_chat_and_keeps_new_chat_launcher() {
    let mut sync = MenuSync::with_cleanup(&[7]);
    let ready = intent(&[8], true, tunnel("https://new.trycloudflare.com"), "Open");
    let mut sent = Vec::new();
    sync.sync(&ready, Instant::now(), |chat, button| {
        sent.push((chat, serde_json::to_value(button).unwrap()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        sent,
        vec![
            (7, serde_json::json!({"type":"commands"})),
            (
                8,
                serde_json::json!({"type":"web_app","text":"Open","web_app":{"url":"https://new.trycloudflare.com"}})
            ),
        ]
    );
}

/// A blocked or deleted first chat must not starve later paired chats on every retry.
#[test]
fn one_failed_chat_does_not_block_other_menus() {
    let mut sync = MenuSync::default();
    let ready = intent(
        &[7, 8],
        true,
        tunnel("https://new.trycloudflare.com"),
        "Open",
    );
    let mut sent = Vec::new();
    assert_eq!(
        sync.sync(&ready, Instant::now(), |chat, _| {
            sent.push(chat);
            if chat == 7 {
                Err(ApiError::Transport)
            } else {
                Ok(())
            }
        }),
        Err(ApiError::Transport)
    );
    assert_eq!(sent, vec![7, 8]);
}
