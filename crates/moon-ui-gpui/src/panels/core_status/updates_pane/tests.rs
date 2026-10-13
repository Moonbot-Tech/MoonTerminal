use super::{UpdatesBlocks, updates_blocks};

/// Full empty only when nothing lags and both lists are empty. History or a live row keeps
/// the table under the caught-up header. A lag never claims the fleet is current.
#[test]
fn updates_blocks_keep_history_and_do_not_claim_caught_up_while_lagging() {
    assert_eq!(updates_blocks(false, false, false), UpdatesBlocks::Empty);
    assert_eq!(
        updates_blocks(false, true, false),
        UpdatesBlocks::HeaderAndTable
    );
    assert_eq!(
        updates_blocks(false, false, true),
        UpdatesBlocks::HeaderAndTable
    );
    assert_eq!(
        updates_blocks(false, true, true),
        UpdatesBlocks::HeaderAndTable
    );
    assert_eq!(updates_blocks(true, false, false), UpdatesBlocks::Table);
    assert_eq!(updates_blocks(true, false, true), UpdatesBlocks::Table);
    assert_eq!(updates_blocks(true, true, true), UpdatesBlocks::Table);
}
