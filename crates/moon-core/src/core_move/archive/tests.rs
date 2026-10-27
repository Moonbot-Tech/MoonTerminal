use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;

use super::*;

/// A snapshot ZIP of `entries`.
fn snapshot(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// One entry of a composed archive.
fn entry(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut file = zip.by_name(name).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).unwrap();
    Some(data)
}

const INI: &[u8] = b"[Global]\r\nLang=1\r\n[Special]\r\nUseTestOrder=0\r\n[Charts]\r\nX=2\r\n";

#[test]
fn a_snapshot_gains_the_rebuilt_database_and_the_first_start_switch() {
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("MoonBot.ini", INI),
        ("data/ByBit Futures-USDT-strat.txt", b"strats"),
        ("bak/ByBit Futures-ChartFigures.zip", b"figs"),
    ]);
    let db = std::env::temp_dir().join(format!("moon-core-move-db-{}", std::process::id()));
    std::fs::write(&db, b"sqlite").unwrap();
    let archive = compose(&zip, None, Some(&db)).unwrap();

    assert_eq!(archive.exchange, "ByBit Futures");
    assert!(archive.has_strategies);
    assert!(archive.ini_flag_added);
    assert_eq!(archive.report_db.as_deref(), Some("data/ByBit Futures.db"));
    assert_eq!(
        entry(&archive.bytes, "data/ByBit Futures.db").unwrap(),
        b"sqlite"
    );
    assert_eq!(entry(&archive.bytes, "BotConfig.bcfg").unwrap(), b"cfg");
    assert_eq!(
        entry(&archive.bytes, "bak/ByBit Futures-ChartFigures.zip").unwrap(),
        b"figs"
    );
    assert_eq!(
        entry(&archive.bytes, "MoonBot.ini").unwrap(),
        b"[Global]\r\nLang=1\r\n[Special]\r\nKeepOrdersAfterCrash=1\r\nUseTestOrder=0\r\n[Charts]\r\nX=2\r\n"
    );
}

#[test]
fn a_switch_the_user_already_set_is_left_alone_and_not_claimed() {
    let ini = b"[special]\nkeepordersaftercrash=0\n";
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("MoonBot.ini", ini),
        ("data/Hyper-USDC-strat.txt", b""),
    ]);
    let archive = compose(&zip, None, None).unwrap();
    assert!(!archive.ini_flag_added);
    assert_eq!(entry(&archive.bytes, "MoonBot.ini").unwrap(), ini);
    assert_eq!(archive.report_db, None);
}

#[test]
fn a_missing_section_or_ini_is_created() {
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("MoonBot.ini", b"[Global]\nLang=1"),
        ("data/Gate-USDT-strat.txt", b""),
    ]);
    let archive = compose(&zip, None, None).unwrap();
    assert_eq!(
        entry(&archive.bytes, "MoonBot.ini").unwrap(),
        b"[Global]\nLang=1\n[Special]\nKeepOrdersAfterCrash=1\n"
    );

    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("data/Gate-USDT-strat.txt", b""),
    ]);
    let archive = compose(&zip, None, None).unwrap();
    assert_eq!(
        entry(&archive.bytes, "MoonBot.ini").unwrap(),
        b"[Special]\r\nKeepOrdersAfterCrash=1\r\n"
    );
}

#[test]
fn several_exchanges_need_an_explicit_name() {
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("data/Binance-USDT-strat.txt", b""),
        ("data/Binance Futures-USDT-strat.txt", b""),
    ]);
    assert!(compose(&zip, None, None).is_err());
    let archive = compose(&zip, Some("Binance Futures"), None).unwrap();
    assert_eq!(archive.exchange, "Binance Futures");
}

/// The list a core-reported exchange is checked against: one entry per exchange, whatever the
/// base currency, nothing from outside `data/` — the shape a live core's snapshot had.
#[test]
fn strategy_exchanges_lists_each_exchange_once() {
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("data/Binance Futures-USDC-strat.txt", b""),
        ("data/Binance Futures-USDT-strat.txt", b""),
        ("data/ByBit Futures-USDT-strat.txt", b""),
        ("bak/Hyper-USDC-strat.txt", b""),
    ]);
    assert_eq!(
        strategy_exchanges(&zip).unwrap(),
        ["Binance Futures", "ByBit Futures"]
    );
}

#[test]
fn unsafe_or_foreign_snapshots_are_refused() {
    let no_config = snapshot(&[("data/Gate-USDT-strat.txt", b"")]);
    assert!(compose(&no_config, None, None).is_err());

    for bad in [
        "../evil",
        "/etc/passwd",
        "data/../../x",
        "C:/x",
        "MoonBot",
        "lib/libssl.so.3",
    ] {
        let zip = snapshot(&[("BotConfig.bcfg", b"cfg"), (bad, b"x")]);
        assert!(
            compose(&zip, Some("Gate"), None).is_err(),
            "{bad} must be refused"
        );
    }

    let carries_db = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        ("data/Gate.db", b"own"),
        ("data/Gate-USDT-strat.txt", b""),
    ]);
    let db = std::env::temp_dir().join(format!("moon-core-move-db2-{}", std::process::id()));
    std::fs::write(&db, b"sqlite").unwrap();
    assert!(compose(&carries_db, None, Some(&db)).is_err());
    let kept = compose(&carries_db, None, None).unwrap();
    assert_eq!(kept.report_db.as_deref(), Some("data/Gate.db"));
}

#[test]
fn a_snapshot_of_only_the_configuration_says_it_carries_no_strategies() {
    let zip = snapshot(&[
        ("BotConfig.bcfg", b"cfg"),
        // A strategy-named file outside data/ is not the core's strategy file.
        ("bak/Hyper-USDC-strat.txt", b""),
    ]);
    let archive = compose(&zip, Some("Hyper"), None).unwrap();
    assert!(!archive.has_strategies);
}
