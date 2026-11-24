use moon_core::core_move::archive;

use super::*;
use crate::core_new::DEMO_SETTINGS;

/// The move window's path when the old core names no exchange and its snapshot holds strategies
/// of two: the refusal says why — the core's silence, then the snapshot's two exchanges — and
/// asks for nothing, since the window has no exchange field to take a name (#1419).
#[test]
fn a_core_that_names_no_exchange_is_said_and_nothing_is_asked() {
    assert_eq!(settle_exchange(None, None, DEMO_SETTINGS).unwrap(), None);
    let composed = archive::compose(DEMO_SETTINGS, None, None).err().unwrap();
    let error = with_core_silence(composed, true);
    let text = format!("{error:#}");
    assert!(
        text.starts_with(
            "the old core reports no exchange name: the settings snapshot holds strategies of \
             several exchanges (Binance, Binance Futures)"
        ),
        "{text}"
    );
    assert!(!text.contains("pass") && !text.contains("--"), "{text}");
    // The command line still sees the mark under the context and adds its own pointer.
    assert!(archive::is_exchange_unsettled(&error));
}

/// The core's silence is said only when the core was asked and the refusal is an unsettled
/// exchange: a ZIP move that never asked it, or any other refusal, reads as it did.
#[test]
fn the_silence_is_said_only_for_an_asked_core_and_an_unsettled_exchange() {
    let composed = archive::compose(DEMO_SETTINGS, None, None).err().unwrap();
    let unasked = format!("{:#}", with_core_silence(composed, false));
    assert!(
        unasked.starts_with("the settings snapshot holds"),
        "{unasked}"
    );

    let other = anyhow::anyhow!("the ZIP has no BotConfig.bcfg");
    assert_eq!(
        format!("{:#}", with_core_silence(other, true)),
        "the ZIP has no BotConfig.bcfg"
    );
}

/// A core-reported name the strategy files do not carry is refused as an unsettled exchange,
/// with the reason alone; one they carry is taken, and the user's own name wins over both.
#[test]
fn a_reported_exchange_is_checked_against_the_strategy_files() {
    let error = settle_exchange(None, Some("Bybit".into()), DEMO_SETTINGS)
        .err()
        .unwrap();
    let text = format!("{error:#}");
    assert!(archive::is_exchange_unsettled(&error), "{text}");
    assert_eq!(
        text,
        "the old core reports exchange \"Bybit\", but the snapshot's strategy files name \
         Binance, Binance Futures"
    );

    assert_eq!(
        settle_exchange(None, Some("Binance Futures".into()), DEMO_SETTINGS).unwrap(),
        Some("Binance Futures".to_owned())
    );
    assert_eq!(
        settle_exchange(Some("Bybit"), Some("Binance".into()), DEMO_SETTINGS).unwrap(),
        Some("Bybit".to_owned())
    );
}
