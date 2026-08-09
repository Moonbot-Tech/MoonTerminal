//! Transfer-target regressions for Assets workspace scope changes.

use moon_core::feed::WalletKind;

use super::{PendingTransfer, pending_transfer_matches_wallet_core};

/// Build one pending transfer captured for `core`.
///
/// Args:
///     core: Core identity captured when the dialog opened.
///
/// Returns:
///     Minimal transfer suitable for target-validation tests.
fn pending(core: u64) -> PendingTransfer {
    PendingTransfer {
        core,
        asset: "USDT".to_string(),
        from: WalletKind::Spot,
        to: WalletKind::Futures,
        free: 1.0,
    }
}

/// `wallets.rs:pending_transfer_matches_wallet_core` must refuse Overview and another Auto core.
///
/// Mutation: accept any `Some` wallet core or treat Overview as the retained Classic core. The
/// corresponding assertion then permits a transfer captured before workspace navigation.
#[test]
fn stale_pending_transfer_cannot_dispatch_after_workspace_change() {
    let transfer = pending(7);

    assert!(pending_transfer_matches_wallet_core(&transfer, Some(7)));
    assert!(!pending_transfer_matches_wallet_core(&transfer, None));
    assert!(!pending_transfer_matches_wallet_core(&transfer, Some(9)));
}

/// `wallets.rs:AssetsView::confirm_transfer` must invoke target validation at dispatch time.
///
/// Mutation: remove the `pending_transfer_matches_wallet_core` call from `confirm_transfer`. The
/// structural assertion fails even though the pure decision helper remains correct but bypassed.
#[test]
fn confirm_transfer_revalidates_pending_core_at_dispatch() {
    let source = include_str!("../wallets.rs");
    let compact: String = source.chars().filter(|ch| !ch.is_whitespace()).collect();

    assert!(
        compact.contains("if!pending_transfer_matches_wallet_core(&pt,effective_wallet_core){")
    );
}
