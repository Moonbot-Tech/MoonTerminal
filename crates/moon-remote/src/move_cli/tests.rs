use moon_core::core_move::archive;
use moon_core::core_move::snapshot::TransferFailed;
use moon_remote::core_move::MoveStep;

use super::{EXCHANGE_HINT, ZIP_HINT, with_exchange_hint, with_zip_hint, worth_a_line};

/// An exchange the snapshot cannot settle keeps the pointer to `--exchange` on the command line —
/// the one path that takes a name — through the context the plan adds, and also after the ZIP
/// hint passed it by; any other failure gets no such pointer (#1419).
#[test]
fn only_an_unsettled_exchange_gets_the_exchange_pointer() {
    let unsettled = archive::unsettled("the settings snapshot holds strategies of two".into())
        .context("the old core reports no exchange name");
    let text = format!("{:#}", with_exchange_hint(with_zip_hint(unsettled)));
    assert_eq!(
        text,
        format!(
            "the old core reports no exchange name: the settings snapshot holds strategies of \
             two: {EXCHANGE_HINT}"
        )
    );
    assert!(EXCHANGE_HINT.contains("--exchange"));

    let failed = anyhow::Error::new(TransferFailed("the core sent no part".into()));
    let text = format!("{:#}", with_exchange_hint(with_zip_hint(failed)));
    assert!(!text.contains("--exchange"), "{text}");
}

/// A failed snapshot transfer keeps the pointer to a hand-saved ZIP on the command line — the one
/// path that takes `--zip` — through the context the plan adds; any other failure is left as it
/// was, since a ZIP would not help it (#1419).
#[test]
fn only_a_failed_transfer_gets_the_zip_pointer() {
    let failed = anyhow::Error::new(TransferFailed("the core sent no part".into()))
        .context("take the settings snapshot from 203.0.113.5:4510");
    let text = format!("{:#}", with_zip_hint(failed));
    assert!(
        text.starts_with("take the settings snapshot from 203.0.113.5:4510: the core sent no part"),
        "{text}"
    );
    assert!(text.ends_with(&format!("; or {ZIP_HINT}")), "{text}");
    assert!(ZIP_HINT.contains("--zip"));

    let other = anyhow::anyhow!("no core \"x\" in servers.enc");
    assert_eq!(
        format!("{:#}", with_zip_hint(other)),
        "no core \"x\" in servers.enc"
    );
}

/// The snapshot's once-a-second count prints one line per whole MiB crossed; every other step
/// prints as before.
#[test]
fn the_snapshot_count_prints_once_per_mib() {
    let mut shown = 0;
    let printed: Vec<u64> = [
        100_000u64, 900_000, 1_100_000, 1_900_000, 2_097_152, 5_000_000,
    ]
    .into_iter()
    .filter(|&bytes| worth_a_line(&MoveStep::SnapshotReceiving { bytes }, &mut shown))
    .collect();
    assert_eq!(printed, vec![1_100_000, 2_097_152, 5_000_000]);
    assert!(worth_a_line(&MoveStep::Checked, &mut shown));
    assert!(worth_a_line(
        &MoveStep::SnapshotTaken {
            bytes: 10,
            exchange: None
        },
        &mut shown
    ));
}
