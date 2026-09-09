use super::*;

/// A frame shaped exactly like the wire's: SockJS `a[...]`, holding a STOMP MESSAGE whose body is
/// a JSON array of trades. The trade fields are the real one-letter keys.
fn frame(body: &str) -> String {
    let stomp = format!(
        "MESSAGE\ndestination:/data/trades\nsubscription:sub-0\nmessage-id:1\ncontent-length:{}\n\n{body}{NUL}",
        body.len()
    );
    format!("a{}", serde_json::to_string(&[stomp]).expect("json"))
}

#[test]
fn a_real_frame_yields_its_trades() {
    let payload = r#"[{"i":97545521,"bi":404,"t":"@moon_bot_kurilka","l":1786574490,"c":"PUMP","b":0.00271936,"s":0.002707,"o":47.95,"u":-0.27,"p":-0.56,"a":0,"f":1}]"#;
    let trades = trades_in(&frame(payload));
    assert_eq!(trades.len(), 1);
    assert_eq!(trades[0].coin, "PUMP");
    assert!((trades[0].profit + 0.27).abs() < 1e-9);
    // The wire's close time belongs to another clock; the reader stamps arrivals itself.
    assert_eq!(trades[0].at_ms, 0);
}

#[test]
fn several_trades_in_one_frame_all_arrive() {
    let payload = r#"[{"c":"BTC","u":12.5},{"c":"ETH","u":-3.25}]"#;
    let trades = trades_in(&frame(payload));
    assert_eq!(trades.len(), 2);
    assert_eq!(trades[1].coin, "ETH");
}

#[test]
fn a_malformed_frame_takes_nothing_down() {
    // Every one of these has been seen in some form on a long-poll transport, and none of them
    // may end the stream: the thread that parses them is the stream.
    for broken in [
        "a[".to_string(),
        "a[\"not stomp\"]".to_string(),
        frame("not json at all"),
        frame(r#"[{"c":"","u":1.0}]"#),
        frame(r#"[{"u":1.0}]"#),
        frame(r#"[{"c":"BTC"}]"#),
        frame(r#"[{"c":"BTC","u":null}]"#),
    ] {
        assert!(trades_in(&broken).is_empty(), "{broken} produced trades");
    }
}

#[test]
fn a_session_id_is_eight_alphanumerics() {
    let mut rng = Rng::new(7);
    for _ in 0..64 {
        let id = session_id(&mut rng);
        assert_eq!(id.len(), 8);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
    }
}
