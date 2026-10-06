//! Masked edit satisfaction and echo rejection comparisons.

use super::*;

/// Whether the core's snapshot already carries everything this write would set IN THE AREAS IT
/// NAMES.
///
/// Restricted to the areas the entry names, like the confirmation that shares its comparison: the
/// rest of its projection is the surface's own copy, frozen when it seeded, and a field that
/// drifted there says nothing about whether THIS edit still has work to do. Comparing it made an OK
/// that changed nothing send the whole snapshot whenever anything else on the core had moved since.
///
/// The marked-market half asks the same question of the LIST — by membership, for the reason
/// [`fav_met`] states — so a star pressed onto a state the core already holds sends nothing.
pub(super) fn edit_satisfied(config: &SharedConfig, op: &QueuedOp) -> bool {
    let actual = core_config_from_proto(config);
    match op {
        QueuedOp::Config { config, touched } => touched.agrees_within(config.as_ref(), &actual),
        QueuedOp::Fav { market, on } => {
            crate::feed::fav_markets_has(&actual.fav_markets, market) == *on
        }
    }
}

/// What the echo disagreed with the terminal about, restricted to the fields `touched` actually
/// names — never the whole projection, so a concurrent core-side change to an untouched field
/// cannot read as this edit's rejection. `None` means every touched field matches, which is also
/// what CONFIRMS a write and what tells the queue an edit needs no send at all: the three questions
/// are one comparison, and they were not always asked the same way.
pub(super) fn rejection_within_mask(
    expected: &CoreConfig,
    actual: &CoreConfig,
    touched: FieldMask,
) -> Option<CoreConfigRejection> {
    // Destructured rather than read through `touched.`: a bit added to the mask must then be
    // NAMED here or the pattern does not compile (E0027). That is the half worth having — leaving
    // a named bit unused is only a warning, so the pattern makes the omission impossible to miss
    // rather than impossible to make. It matters more than it reads: this function is now also
    // what decides an edit is already satisfied, so a bit with no arm would make every edit naming
    // it drop without ever being sent.
    let FieldMask {
        auto_buy,
        auto_start,
        btc_blink,
        general,
        gestures,
        interface,
        order_rules,
        leverage,
        signals,
        special,
        telegram,
        ignore_strat_sell_price,
    } = touched;
    let mut areas = Vec::new();
    if auto_buy && expected.auto_buy != actual.auto_buy {
        areas.push(CoreConfigArea::AutoBuy);
    }
    if auto_start && expected.auto_start != actual.auto_start {
        areas.push(CoreConfigArea::AutoStart);
    }
    if btc_blink && expected.btc_blink != actual.btc_blink {
        areas.push(CoreConfigArea::BtcBlink);
    }
    if general && expected.general != actual.general {
        areas.push(CoreConfigArea::General);
    }
    if gestures && expected.gestures != actual.gestures {
        areas.push(CoreConfigArea::Gestures);
    }
    if interface && expected.interface != actual.interface {
        areas.push(CoreConfigArea::Interface);
    }
    if order_rules && expected.order_rules != actual.order_rules {
        areas.push(CoreConfigArea::OrderRules);
    }
    if leverage && expected.leverage != actual.leverage {
        areas.push(CoreConfigArea::Leverage);
    }
    if signals && expected.signals != actual.signals {
        areas.push(CoreConfigArea::Signals);
    }
    if special && expected.special != actual.special {
        areas.push(CoreConfigArea::Special);
    }
    if telegram && expected.telegram != actual.telegram {
        areas.push(CoreConfigArea::Telegram);
    }
    if ignore_strat_sell_price
        && expected.manual.ignore_strat_sell_price != actual.manual.ignore_strat_sell_price
    {
        areas.push(CoreConfigArea::Manual);
    }
    (!areas.is_empty()).then_some(CoreConfigRejection::Areas(areas))
}
