//! Wallet actions, folder projections and strategy publication signatures.

use super::news_trace::wire_text;
use super::*;

/// Maps moonproto `ExchangeKind` to a domain wallet (the inverse of `assets::to_exchange_kind`).
fn wallet_kind_from_proto(k: moonproto::state::ExchangeKind) -> WalletKind {
    match k {
        moonproto::state::ExchangeKind::Spot => WalletKind::Spot,
        moonproto::state::ExchangeKind::Futures => WalletKind::Futures,
        moonproto::state::ExchangeKind::Quarterly => WalletKind::Quarterly,
    }
}

/// Projects `Event::EngineAction` into a terminal result for UI toasts.
pub(in crate::feed::live) fn engine_action_result(
    e: &moonproto::EngineActionEvent,
) -> EngineActionResult {
    use moonproto::EngineActionKind as K;
    let kind = match &e.kind {
        K::CancelAllOrders => EngineActionKind::CancelAllOrders,
        K::SetLeverage {
            market,
            new_leverage,
        } => EngineActionKind::SetLeverage {
            market: market.clone(),
            leverage: *new_leverage,
        },
        K::SetHedgeMode { hedge_mode } => EngineActionKind::SetHedgeMode { on: *hedge_mode },
        K::ChangePositionType { market, .. } => EngineActionKind::ChangePositionType {
            market: market.clone(),
        },
        K::ConvertDustBnb => EngineActionKind::ConvertDust,
        K::ConfirmRiskLimit { market } => EngineActionKind::ConfirmRiskLimit {
            market: market.clone(),
        },
        K::SetMaMode { ma_mode } => EngineActionKind::SetMaMode { on: *ma_mode },
        K::TransferAsset {
            asset,
            qty,
            from,
            to,
        } => EngineActionKind::TransferAsset {
            asset: asset.clone(),
            qty: *qty,
            from: wallet_kind_from_proto(*from),
            to: wallet_kind_from_proto(*to),
        },
        K::ReloadOrderBook => EngineActionKind::ReloadOrderBook,
    };
    EngineActionResult {
        kind,
        success: e.success,
        error_code: e.error_code,
        error_msg: e.error_msg.clone(),
    }
}

/// Longest folder path kept from the core's tree, in characters.
///
/// A DISPLAY guard and nothing more: this projection is never the source of an outgoing tree — the
/// feed builds those from the core's own untouched paths — so clamping here cannot ask the core to
/// rename anything. Whether a path could be sent back at all is a separate question, answered once
/// per core by `CoreFolders::editable`.
const FOLDER_PATH_MAX_CHARS: usize = 255;

/// How many folders one core's tree is kept to.
///
/// moonproto's own ceiling is 65 534 dictionary entries; this sits far below it because the list is
/// retained per core and drawn as rows. Truncating is safe for the same reason the clamp above is:
/// nothing sends this list back.
const FOLDER_PATHS_MAX: usize = 5_000;

/// Project the core's folder tree, empty folders included.
///
/// `supported` is `folders_last_modified() > 0` and nothing else: the protocol states plainly that
/// zero means either "no versioned tree has arrived" or "this core does not synchronize folders",
/// and the two are indistinguishable from here. Both readings mean the same thing to a caller — an
/// empty folder cannot be sent to this core — so nothing is gained by guessing which one holds.
///
/// Invalid raw paths are rejected before text cleanup, and the caller does the path SPLITTING: which
/// slashes separate folders is decided in one place in the window, and a second rule here would
/// disagree with the tree it feeds.
///
/// Args:
///     strats: The core's retained strategy state.
///
/// Returns:
///     The reported tree, or a `supported: false` value with no paths.
pub(in crate::feed::live) fn folders_from_proto(
    strats: &moonproto::state::StratsState,
) -> crate::feed::CoreFolders {
    let supported = strats.folders_last_modified() > 0;
    if !supported {
        return crate::feed::CoreFolders::default();
    }
    // Asked of the paths as the CORE spells them, before any cleaning: whether an edit can be sent
    // depends on what the core holds, not on what this projection made of it.
    let editable = crate::feed::folder_tree::sendable(strats.folder_paths());
    crate::feed::CoreFolders {
        supported,
        editable,
        paths: display_folder_paths(strats.folder_paths()),
    }
}

/// Reject invalid raw folder identities before text cleanup can disguise synthetic parents.
///
/// MoonProto expands `Group / Display Name` into a parent `Group ` as well as the full path. Trimming
/// that parent first would make it pass the UI's validation as a real empty folder `Group`.
/// Keep the editability decision on the complete raw tree in `folders_from_proto`; this list
/// only supplies displayable folders, sorted and bounded independently of protocol map order.
pub(super) fn display_folder_paths<'a>(raw: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut paths: Vec<String> = raw
        .filter(|path| crate::feed::folder_tree::sendable(std::iter::once(*path)))
        .map(|path| wire_text(path, FOLDER_PATH_MAX_CHARS))
        .filter(|path| !path.is_empty())
        .collect();
    // Sorted because the source order is explicitly meaningless — moonproto iterates a map and says
    // so — and an unstable order would republish an unchanged tree on every rehash, waking the
    // window and rebuilding its whole strategy tree for nothing.
    //
    // BEFORE the cap, so that which folders survive it is decided by the paths themselves rather
    // than by where a rehash happened to put them.
    paths.sort_unstable();
    paths.dedup();
    paths.truncate(FOLDER_PATHS_MAX);
    paths
}

/// Fold the strategy set into the signature that decides whether the UI is told about it.
///
/// ORDER-SENSITIVE, and that is the load-bearing property rather than an artefact of the fold. The
/// core's strategy list is an arrangement the operator made, moonproto synchronizes it, and the
/// terminal's whole confirmation path — the tree's unconfirmed-order overlay, which draws a sent
/// arrangement until the core answers — can only be retired by seeing the answer arrive. Folded
/// commutatively, as a sibling signature in `tree::cache` deliberately is, a reorder echo would be
/// invisible: the overlay would sit out its whole window and then expire onto a list it had already
/// been given. `a_reordered_set_is_a_different_signature` pins it.
///
/// Args:
///     rows: Per strategy, `(id, revision, last edit date, checked)`, in the core's own order.
///
/// Returns:
///     A signature that changes whenever the contents OR the sequence do.
pub(in crate::feed::live) fn strategies_publish_sig(
    rows: impl Iterator<Item = (u64, i32, u64, bool)>,
) -> u64 {
    let mut sig = 0u64;
    for (id, ver, last_date, checked) in rows {
        sig = sig
            .wrapping_mul(1099511628211)
            .wrapping_add(id)
            .wrapping_add((ver as u32 as u64).wrapping_shl(1))
            .wrapping_add(last_date)
            .wrapping_add(checked as u64);
    }
    sig
}
