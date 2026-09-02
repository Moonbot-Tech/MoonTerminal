//! Moonbot's price-approach alert sounds: a sound when the last price comes within N per cent of
//! an order's sell price, and the same for its buy price.
//!
//! The settings are the CORE's own (`SignalsSettings`, edited in the gear popup's General tab), but
//! the sound plays HERE: the core is a headless Moonbot on another machine, so a setting left to it
//! alone would beep where nobody is sitting.
//!
//! Built like [`super::detect_sound`], and for the same reasons: feed draining calls this hundreds
//! of times a second, so a revision gate does the work only when something actually arrived; a
//! core's first visit SEEDS silently rather than announcing every order that was already near; and
//! at most one sound leaves per drain, because the player interrupts itself and a burst would be
//! heard as one truncated noise anyway.
//!
//! Two things this had to learn about the feed, either of which alone made the first version fire
//! almost never:
//!
//! - **The price is not on the row.** An `OrderRow` is rebuilt only when an order-TABLE batch is
//!   published, and price movement does not publish one — it arrives as `OrderEvent::TracePoint`,
//!   which takes the order-LINE path (`feed::live`'s `has_orders_table_event` excludes it). A
//!   resting take-profit can therefore carry a `price` field minutes old. The live figure comes
//!   from the market source instead, read once per MARKET rather than once per order, because that
//!   read takes the source's lock.
//! - **The gate is a PAIR of revisions.** `orders_table_rev` moves when the order SET changes;
//!   `order_lines_rev` is the one that moves when the PRICE under those orders does, because the
//!   trace point carrying it bumps that revision and no other. Gating on the table alone made the
//!   alert wait for an unrelated order change before it would look at a price at all.
//!
//! The alert is an EDGE, not a level. A price sitting inside the band would otherwise re-fire on
//! every batch for as long as it stayed there — several times a second on a moving market. Each
//! core therefore retains the set of order legs currently inside their band, and only a leg that
//! was outside it last time is announced.

use std::collections::{HashMap, HashSet};

use crate::Backend;
use crate::order_math::{pct_to_entry, pct_to_exit};

/// Which of an order's two prices an alert is watching.
///
/// Part of the retained key rather than two sets, so an order that is PARTLY filled — holding a
/// position while its entry leg still works — can be inside one band and outside the other without
/// the two states overwriting each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AlertLeg {
    /// The exit price — Moonbot's "sell price" for a long and for a short alike.
    Exit,
    /// The entry price, while the entry leg is still working.
    Entry,
}

impl Backend {
    /// Play the price-approach alerts, at most one sound for the whole drain.
    ///
    /// Cheap when nothing arrived: every core sits behind the revision pair the module doc
    /// describes, and a core with both alerts switched off is dropped before its orders are read.
    ///
    /// Args:
    ///     detect_played: Whether the detect scan already used this drain's one sound.
    pub(crate) fn play_price_alert_sounds(&mut self, detect_played: bool) {
        // Cloned once for the whole pass rather than per core: the handle is an `Arc` clone, while
        // each `latest_price` below takes the source's read lock.
        let source = self.session.market_source();
        // One sound for the DRAIN and not one per core: the player replaces whatever is playing, so
        // announcing two cores in the same pass would leave only the second audible while spending
        // the first one's edge silently. The first crossing wins; the rest stay counted.
        //
        // It starts at whatever the DETECT scan just did, for the same reason: that scan runs first
        // in the same drain and shares the one player, so a price alert firing now would clip a
        // detect sound a few microseconds old.
        let mut played = detect_played;
        for (core, data) in self.session.store().cores() {
            let rev = (data.orders_table_rev, data.order_lines_rev);
            if self.last_orders_alert_rev.get(&core) == Some(&rev) {
                continue;
            }
            self.last_orders_alert_rev.insert(core, rev);
            // No config yet means the core has not sent its snapshot; there is nothing to arm the
            // alert with, and inventing a default would beep on settings the user never chose.
            let Some(cfg) = data.core_config.as_ref().map(|c| c.signals) else {
                continue;
            };
            if !cfg.play_sell_alert && !cfg.play_buy_alert {
                // Both alerts off: drop whatever this core had retained, so switching one back on
                // seeds silently through the branch below instead of comparing against a set that
                // stopped being maintained while the prices moved on.
                self.price_alert_near.remove(&core);
                continue;
            }
            if data.orders.is_empty() {
                // Everything closed: drop the retained set rather than skipping past it. Keeping
                // it would leave stale `(uid, leg)` entries that suppress the first alert of a
                // later order whose uid the core reused — the very case the whole-set rebuild
                // below exists to avoid.
                self.price_alert_near.remove(&core);
                continue;
            }

            // Which legs are inside their band NOW. Built whole rather than diffed in place: an
            // order that closed since the last pass has to leave the retained set, or its slot
            // would suppress the alert of a later order that reused the uid.
            let was = self.price_alert_near.get(&core);
            let mut near: HashSet<(u64, AlertLeg)> = HashSet::new();
            let mut price_of: HashMap<&str, Option<f64>> = HashMap::new();
            // Whether any market this core trades had no price on this pass. It decides whether the
            // pass is fit to SEED from; see the seed branch below.
            let mut prices_missing = false;
            for row in &data.orders {
                if row.job_is_done {
                    continue;
                }
                let last = *price_of.entry(row.market.as_str()).or_insert_with(|| {
                    source
                        .latest_price(core, &row.market)
                        .ok()
                        .map(f64::from)
                        .filter(|p| *p > 0.0)
                });
                let Some(last) = last else {
                    prices_missing = true;
                    // No price for this market yet — a reconnect's first snapshot lands before the
                    // prices do. Carry the previous membership over rather than reading the gap as
                    // "the leg left its band": dropping it here would re-announce every leg the
                    // moment prices arrive, which is the burst the silent seed exists to prevent.
                    if let Some(was) = was {
                        for leg in [AlertLeg::Exit, AlertLeg::Entry] {
                            if was.contains(&(row.uid, leg)) {
                                near.insert((row.uid, leg));
                            }
                        }
                    }
                    continue;
                };
                // BOTH legs are tracked whenever this core is scanned at all, and NOT only the ones
                // whose alert is armed: a leg left out of the set while its own alert is off would
                // read as a fresh crossing the moment the user arms it, announcing a price that has
                // not moved. Only the announcement below is gated on the flags.
                //
                // The two are not exclusive. `filled` means the order HOLDS A POSITION — a fill of
                // any size, or a placed sell — so a partly filled entry has a live exit AND a buy
                // leg still working for the remainder; `fill_pct` is what says the entry is done.
                let sell_level = f64::from(cfg.sell_alert_level);
                let buy_level = f64::from(cfg.buy_alert_level);
                if row.filled && pct_to_exit(row, last).is_some_and(|left| left <= sell_level) {
                    near.insert((row.uid, AlertLeg::Exit));
                }
                if row.fill_pct < 100.0
                    && pct_to_entry(row, last).is_some_and(|left| left <= buy_level)
                {
                    near.insert((row.uid, AlertLeg::Entry));
                }
            }

            let Some(was) = was else {
                // First visit: seed and stay silent. Otherwise every order already sitting inside
                // its band would announce itself at startup and on every reconnect.
                //
                // But only from a pass that could actually SEE the prices. A first connect delivers
                // the order snapshot before the market data, and a seed taken then is empty for a
                // reason that has nothing to do with where the prices are — so the next pass would
                // read every leg as a fresh inward crossing and fire on a price that never moved.
                // The carry-over above cannot help here: there is nothing retained to carry. So the
                // seed waits for a pass with prices, which is the next one or two.
                if !prices_missing {
                    self.price_alert_near.insert(core, near);
                }
                continue;
            };
            let leg = leg_to_announce(&near, was, |leg| match leg {
                AlertLeg::Exit => cfg.play_sell_alert,
                AlertLeg::Entry => cfg.play_buy_alert,
            });
            self.price_alert_near.insert(core, near);
            let Some(leg) = leg else { continue };
            let ordinal = match leg {
                AlertLeg::Exit => cfg.signal_sound_2,
                AlertLeg::Entry => cfg.buy_signal_sound,
            };
            // The same `channels.detects` log the detect sounds write to, and for the same reason:
            // "it did not beep" has several causes here — the alert switched off, the level never
            // reached, quiet mode, an ordinal this build cannot name, a core with no config yet, the
            // silent first pass, and another core having taken this drain's one sound — and they
            // are indistinguishable from outside. The line names which one it was.
            let name = crate::media::sound::mb_sound_name(ordinal);
            moon_core::detect_diag::line(&format!(
                "[price-alert] core={} leg={leg:?} sound={ordinal} ({}){}",
                moon_core::feed::core_label(core),
                name.unwrap_or("НЕТ ТАКОГО ЗВУКА"),
                match (self.quiet_sleeping, played) {
                    (true, _) => ", silent: quiet mode",
                    (_, true) => ", silent: another core took this drain's sound",
                    _ => "",
                }
            ));
            // Quiet mode silences the sound but NOT the bookkeeping above: the edge is spent either
            // way, so a night spent inside a band does not empty itself into the morning. Detect
            // sounds advance their cursors under quiet mode for the same reason. There is no
            // per-alert bypass as detects have — that exists for a detect NUMBER the operator wants
            // to be woken by, and an order approaching its price has no such axis to name.
            if !self.quiet_sleeping && !played {
                crate::media::sound::play_ordinal(ordinal);
                played = true;
            }
        }
    }
}

/// Which leg, if any, this pass should announce: an ARMED one that is inside its band now and was
/// not last time.
///
/// At most one, because the player interrupts itself — two sounds in one pass are heard as one
/// truncated noise, so the second would only cut the first short. The EXIT wins that tie: a
/// position about to close is the one worth hearing, and its alert is the one with money already
/// committed behind it.
///
/// `armed` is applied BEFORE the tie is broken rather than to the winner afterwards: an unarmed
/// exit crossing in the same batch would otherwise take the pass and silence an armed entry.
///
/// Args:
///     near: Legs inside their band on this pass, armed or not.
///     was: Legs that were inside it on the previous pass.
///     armed: Whether this leg's alert is switched on for the core.
///
/// Returns:
///     The leg to announce, or `None` when nothing armed crossed inward.
fn leg_to_announce(
    near: &HashSet<(u64, AlertLeg)>,
    was: &HashSet<(u64, AlertLeg)>,
    armed: impl Fn(AlertLeg) -> bool,
) -> Option<AlertLeg> {
    [AlertLeg::Exit, AlertLeg::Entry]
        .into_iter()
        .filter(|leg| armed(*leg))
        .find(|leg| near.iter().any(|e| e.1 == *leg && !was.contains(e)))
}

#[cfg(test)]
mod tests;
