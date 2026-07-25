//! Writing the coin lists into the strategies: "Save" and "Make a copy" for the
//! "By coin" axis.
//!
//! Both go through the SHARED confirmation dialog (`open_change_dialog` / `open_copy_with`)
//! and the shared write path, so a coin list travels the same route as a threshold: the edits
//! are grouped per core and the echo is re-read three times. Nothing here talks to the session
//! directly. The dialog shows the EDIT for these fields rather than "now → next" — see
//! `edit_note` for why that comparison cannot be made on a set.
//!
//! WHAT MAKES THIS AXIS DIFFERENT from the others: its value is a SET, the strategies do not
//! share it, the core edits it too, and the wire only takes whole fields.
//!
//! So Save does not write a value at all — it writes an EDIT. The delta ("which coins did the
//! user tick, which did they untick") is replayed onto each selected strategy's OWN live list,
//! producing one string per strategy. Three things follow that a single shared value cannot
//! give:
//!
//! - a coin nobody touched keeps whatever each strategy has. The panel's working set is the
//!   UNION of the selected lists, so writing IT copied one strategy's list onto every other:
//!   select a copy holding 373 coins beside one holding none, tick a single coin, and the
//!   empty one came back with 374. Half of all same-named pairs on the real database hold
//!   different lists, so this was the normal case, not an edge one;
//! - the write differs from what is stored by exactly the edit, so the confirmation is
//!   readable and the version history records one coin instead of "the whole field changed";
//! - a lost update is NARROWED, not eliminated. A coin the core added is in the live list, is
//!   in neither delta, and survives — but that list comes from the strategies REPLICA, and the
//!   command lands as a whole-field replace on the core's own snapshot. Whatever the core
//!   wrote between the replica's last commit and the write landing is still overwritten, and
//!   so is whatever it writes while the confirmation dialog sits open. This doc used to claim
//!   the window was closed; it is not, and its width is replica lag plus dialog dwell time.
//!
//! A strategy whose live list cannot be read is SKIPPED and counted in the dialog, not written
//! blind: there is nothing to replay the edit onto, and the edit alone would erase the rest.
//! Refusing the whole save instead made any selection containing a strategy deleted on its
//! core permanently unsavable.

use std::collections::{HashMap, HashSet};

use gpui::*;
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::super::shared::SaveTarget;

/// The strategy parameters this axis edits. Also the fields' on-screen labels — the names the
/// user knows from Moonbot — so a label and the written key cannot drift apart.
pub(super) const FIELD: &str = "CoinsBlackList";
pub(super) const WHITE_FIELD: &str = "CoinsWhiteList";

/// How many dropped coins the warning names before it stops counting them out. Enough to
/// recognise the situation; past that the number is the point, not the roll call.
const NAMED_IN_WARN: usize = 8;

impl AnalyticsView {
    /// "Save": write the lists the field is showing into every selected strategy.
    ///
    /// Opens the dialog only after EVERY target's live value has been re-read, so the
    /// confirmation can state what the overwrite would discard — and say so when it could not
    /// look, rather than letting silence pass for "checked".
    ///
    /// Args:
    ///     cx: GPUI context used to read live values and open the guarded dialog.
    pub(in crate::analytics::tuner) fn coins_open_save_dialog(&mut self, cx: &mut Context<Self>) {
        let targets = self.selected_targets();
        if targets.is_empty() {
            return;
        }
        // No edit, nothing to write. Guarded here as well as on the button, because the
        // button only *looks* disabled — a stale render could still deliver the click.
        if !self.coins.has_changes() {
            log::info!("analytics: 'Save' (coins) — the lists match what is saved");
            return;
        }
        let Some(changes) = self.coin_list_changes() else {
            // The saved lists have not been read, so the entries as written are unknown and
            // the value would be folded tokens — which would delete every contract-suffixed
            // entry the strategy holds. Refuse rather than write a lossy list.
            self.set_write_error(t!("analytics.coins.save_not_loaded").to_string(), cx);
            return;
        };
        if changes.is_empty() {
            // `has_changes` said yes and the fold produced nothing: the two disagree, which is
            // a defect rather than a no-op. Logged only — an empty dialog would be worse, and
            // the state is unreachable unless the two go out of step.
            log::warn!("analytics: 'Save' (coins) — an edit is pending but no field changed");
            return;
        }
        // A new attempt answers the previous complaint.
        self.write_error = None;
        // THE EDIT ITSELF, as tokens: what the user ticked and what they unticked, measured
        // against the snapshot the panel was showing.
        //
        // This is the whole point of the axis' write. `saved` is the UNION of the selected
        // strategies' lists, so a coin that came from merging them sits in BOTH sets and
        // therefore in NEITHER delta — "leave it alone". Writing the working set instead made
        // one strategy's list appear in every other selected strategy: select a copy with 373
        // coins beside a copy with none, tick ONE coin, and the empty one came back with 374.
        // Measured on the real database: of 754 pairs of same-named strategies that both hold
        // a list, half hold DIFFERENT lists.
        let delta: Vec<(String, Vec<String>, Vec<String>)> = changes
            .iter()
            .map(|(k, _)| {
                let (saved, work) = if k == WHITE_FIELD {
                    (&self.coins.saved.white, &self.coins.work.white)
                } else {
                    (&self.coins.saved.black, &self.coins.work.black)
                };
                (
                    k.clone(),
                    work.difference(saved).cloned().collect(),
                    saved.difference(work).cloned().collect(),
                )
            })
            .collect();
        let keys: Vec<String> = changes.iter().map(|(k, _)| k.clone()).collect();
        // What this Save is ABOUT, re-checked after the read: a selection change during it
        // would otherwise open the dialog on the OLD targets, and confirming would write to a
        // strategy the panel had stopped showing.
        let req: Vec<(i64, Option<u64>)> = targets.iter().map(|t| (t.sid, t.core)).collect();
        // The working lists' revision. A period/side/core-filter change calls `reload`, which
        // invalidates them WITHOUT touching the selection — so comparing targets alone let the
        // dialog open on a delta whose baseline had already been thrown away. A tick bumps it
        // too, and that is equally a reason to drop: the delta predates it.
        let rev = self.coins.lists_rev;
        self.op_started();
        cx.spawn(async move |this, cx| {
            let executor = cx.update(|cx| cx.background_executor().clone());
            let read_keys = keys.clone();
            let (targets, live, unresolved) = executor
                .spawn(async move {
                    // A target with NO core is resolved to one target PER live core FIRST.
                    //
                    // Left alone it read the list of a SINGLE core and `send_bulk_changes`
                    // then fanned that one string to EVERY core of the strategy — writing one
                    // core's coin list over all the others. That is the same "one list copied
                    // onto another" this axis exists to prevent, surviving one level down.
                    // All four review passes found it independently.
                    let mut expanded: Vec<SaveTarget> = Vec::with_capacity(targets.len());
                    // A target whose cores cannot be resolved is COUNTED, never quietly
                    // dropped: `strategy_cores` also returns empty when strategies.sqlite is
                    // missing or will not open, so "could not look" would otherwise become
                    // "there was nothing to write to".
                    let mut unresolved = 0usize;
                    for t in targets {
                        match t.core {
                            Some(_) => expanded.push(t),
                            None => {
                                let cores = moon_core::db::tuner::strategy_cores(t.sid);
                                if cores.is_empty() {
                                    unresolved += 1;
                                }
                                expanded.extend(cores.into_iter().map(|c| SaveTarget {
                                    sid: t.sid,
                                    core: Some(c),
                                    name: t.name.clone(),
                                }));
                            }
                        }
                    }
                    let targets = expanded;
                    let live: Vec<Option<HashMap<String, String>>> = targets
                        .iter()
                        .map(|t| {
                            moon_core::db::tuner::strategy_current_values_opt(
                                t.sid, t.core, &read_keys,
                            )
                        })
                        .collect();
                    (targets, live, unresolved)
                })
                .await;
            let _ = cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    'completion: {
                        let now: Vec<(i64, Option<u64>)> = this
                            .selected_targets()
                            .iter()
                            .map(|t| (t.sid, t.core))
                            .collect();
                        if now != req {
                            // The user moved to another strategy. Silence is right: they are
                            // looking elsewhere, and a banner about the old one would be noise.
                            log::info!(
                                "analytics: 'Save' (coins) - the selection changed, dropped"
                            );
                            break 'completion;
                        }
                        if this.coins.lists_rev != rev {
                            // Same strategies, but the lists underneath moved (a tick, or a
                            // period/filter change). The delta was computed against the old
                            // baseline, so it cannot be trusted — and unlike a selection change
                            // the user is still looking at this panel, so a dropped click needs
                            // to leave a mark rather than nothing at all.
                            this.set_write_error(t!("analytics.coins.save_moved").to_string(), cx);
                            break 'completion;
                        }
                        // ONE VALUE PER STRATEGY: its own live list with the edit replayed on top.
                        //
                        // A target whose list could not be read is DROPPED together with its
                        // value, so the two stay index-aligned. Refusing the whole save instead
                        // made any selection containing a strategy deleted on its core permanently
                        // unsavable, because that read never succeeds.
                        let mut writable: Vec<SaveTarget> = Vec::with_capacity(targets.len());
                        let mut per_target: Vec<Vec<(String, String)>> =
                            Vec::with_capacity(targets.len());
                        // Seeded with the targets whose cores never resolved: both mean "this
                        // strategy was not written and we could not find out why".
                        let mut unreadable = unresolved;
                        for (t, cur) in targets.into_iter().zip(live.iter()) {
                            let Some(map) = cur else {
                                // Every write is a whole-field overwrite, so without this
                                // strategy's current list there is nothing to replay the edit
                                // ONTO, and the edit alone would erase everything else it holds.
                                unreadable += 1;
                                continue;
                            };
                            per_target.push(
                                delta
                                    .iter()
                                    .map(|(key, added, removed)| {
                                        let fresh = map.get(key).map(String::as_str).unwrap_or("");
                                        (key.clone(), apply_delta(fresh, added, removed))
                                    })
                                    .collect(),
                            );
                            writable.push(t);
                        }
                        if writable.is_empty() {
                            this.set_write_error(
                                t!("analytics.coins.save_unreadable", n = unreadable).to_string(),
                                cx,
                            );
                            break 'completion;
                        }
                        let n_targets = writable.len();
                        let mut notes: Vec<Option<String>> = delta
                            .iter()
                            .map(|(_, added, removed)| Some(edit_note(added, removed, n_targets)))
                            .collect();
                        // A field that ends up EMPTY somewhere is called out: an untick-all wipes
                        // that strategy's list, and a bare "-N (...)" reads like an ordinary
                        // removal. The dialog's own blank-value warning cannot fire here, because
                        // a row carrying a note never draws the value branch.
                        for (i, (key, _, _)) in delta.iter().enumerate() {
                            let empties = per_target.iter().any(|v| {
                                v.iter().any(|(k, val)| k == key && val.trim().is_empty())
                            });
                            if empties {
                                if let Some(Some(note)) = notes.get_mut(i) {
                                    note.push_str(" · ");
                                    note.push_str(&t!("analytics.tuner.save_clears"));
                                }
                            }
                        }
                        let mut warns = Vec::new();
                        if unreadable > 0 {
                            warns.push(
                                t!("analytics.coins.save_skipped", n = unreadable).to_string(),
                            );
                        }
                        // The dialog draws one row per FIELD; the anchor's values fill it, and the
                        // note beside each says what the EDIT is - the same for every target,
                        // because it is the edit rather than a value.
                        let shown = per_target.first().cloned().unwrap_or_default();
                        this.open_change_dialog(
                            writable,
                            shown,
                            Some(per_target),
                            notes,
                            warns,
                            false,
                            cx,
                        );
                    }
                    // Balance and schedule only after every guarded completion path has stored
                    // its dialog or failure state.
                    this.op_finished(cx);
                    this.schedule_report_refresh(cx);
                });
            });
        })
        .detach();
    }

    /// "Make a copy": the same lists, but into a NEW strategy rather than over the source.
    ///
    /// Single-target by construction — the button is hidden in multi-select, and a copy of
    /// "several strategies at once" has no meaning.
    ///
    /// No drift check: a copy writes a NEW strategy, so there is no existing value anywhere
    /// for a concurrent edit to be lost from. Unchanged lists are fine too — that is simply a
    /// duplicate of the strategy, which is a legitimate thing to ask for.
    pub(in crate::analytics::tuner) fn coins_open_copy_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.selected_targets().into_iter().next() else {
            return;
        };
        let changes = match self.coin_list_changes() {
            Some(c) => c,
            // Lists unread. With no pending edit that is harmless — the copy inherits the
            // source's own fields. With one, going ahead would hand back a duplicate that
            // quietly does NOT contain the ticks the user just made, while Save refuses the
            // very same state out loud. Refuse here too.
            None if self.coins.has_changes() => {
                self.set_write_error(t!("analytics.coins.save_not_loaded").to_string(), cx);
                return;
            }
            None => Vec::new(),
        };
        self.open_copy_with(target, changes, Vec::new(), window, cx);
    }
}

/// The separator the strategy itself uses, reused verbatim.
///
/// `split_coin_list` accepts commas, semicolons, whitespace and JSON punctuation alike, so
/// picking one unconditionally rewrote every line of a field that happened to use another —
/// the same "one coin edited, the whole field changed" this module exists to avoid. The first
/// run of separator characters between two entries IS the field's convention.
fn separator_of(fresh: &str) -> &str {
    const SEP: [char; 6] = [',', ';', ' ', '\t', '\n', '\r'];
    // A LEADING run is skipped first: a field written as `, A, B` would otherwise offer its
    // opening comma as the convention. (JSON punctuation is NOT in `SEP`, so a JSON-shaped
    // value is not handled here at all — `apply_delta` returns it untouched when the edit is
    // empty, and reformats it otherwise.)
    let body = fresh.trim().trim_start_matches(SEP);
    let Some(start) = body.find(SEP) else {
        // One entry, or none: nothing to separate, and a second entry is the caller's first.
        return ", ";
    };
    let end = body[start..]
        .find(|c: char| !SEP.contains(&c))
        .map_or(body.len(), |off| start + off);
    &body[start..end]
}

/// A strategy's own list with the user's EDIT replayed on top: its entries, its order, minus
/// what was unticked, plus what was ticked.
///
/// Three properties follow, and none of them survive writing the working set instead:
///
/// - **a coin nobody touched stays as that strategy has it.** The working set is the UNION of
///   the selected strategies' lists, so writing it copies one strategy's list onto every
///   other. Half of all same-named pairs on the real database hold different lists.
/// - **the write differs from what is stored by exactly the edit**, so the confirmation is
///   readable and the version history records one coin rather than "the whole field changed".
/// - **a lost update is narrowed, NOT eliminated.** A coin the core added is in `fresh`, is
///   in neither delta, and survives — but `fresh` comes from the strategies REPLICA, and the
///   command lands as a whole-field replace on the core's own snapshot. Anything the core
///   wrote between the replica's last commit and the write landing is still overwritten, and
///   so is anything it writes while the confirmation dialog sits open. The doc used to claim
///   this could not happen; it can, and the window is replica lag plus dialog dwell time.
///
/// New ticks go to the FRONT ("the last coin written comes first", which is how the field is
/// read) and the strategy's own separator is kept: Moonbot writes `A,B,C`, and re-spacing it
/// to `A, B, C` changed every line of the confirmation for a one-coin edit.
fn apply_delta(fresh: &str, added: &[String], removed: &[String]) -> String {
    use moon_core::symbol::{coin_match_key, split_coin_list};
    // Nothing to do means the field goes back BYTE FOR BYTE. Without this the rebuild
    // normalises whatever spelling the strategy used — `split_coin_list` also accepts JSON
    // array punctuation, which no separator heuristic reproduces — and "one coin edited, the
    // whole field rewritten" returns through the door it was shown out of.
    if added.is_empty() && removed.is_empty() {
        return fresh.to_string();
    }
    let gone: HashSet<&str> = removed.iter().map(String::as_str).collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut present: HashSet<String> = HashSet::new();
    for entry in split_coin_list(fresh) {
        let token = coin_match_key(entry);
        // Unticking a COIN drops every spelling of it (`BTC`, `BTC_0626`, `BTC_0925`);
        // leaving one behind would mean the coin is still listed.
        if gone.contains(token.as_str()) {
            continue;
        }
        present.insert(token);
        kept.push(entry);
    }
    let mut fresh_ticks: Vec<&str> = added
        .iter()
        .filter(|t| !present.contains(t.as_str()))
        .map(String::as_str)
        .collect();
    // Sorted only so two identical edits produce the same string.
    fresh_ticks.sort_unstable();
    let sep = separator_of(fresh);
    fresh_ticks
        .into_iter()
        .chain(kept)
        .collect::<Vec<_>>()
        .join(sep)
}

/// What the write does, in words — the EDIT, not a value.
///
/// "Now → next" is unreadable for a set of hundreds whose wrapping shifts on every line, and
/// with one value per strategy there is no single "next" to show anyway. What every target
/// has in common is the edit, so that is what the dialog states.
fn edit_note(added: &[String], removed: &[String], n_targets: usize) -> String {
    let side = |sign: char, v: &[String]| {
        if v.is_empty() {
            return format!("{sign}0");
        }
        let mut names: Vec<&str> = v.iter().map(String::as_str).collect();
        names.sort_unstable();
        let n = names.len();
        names.truncate(NAMED_IN_WARN);
        let mut list = names.join(", ");
        if n > NAMED_IN_WARN {
            list.push('…');
        }
        format!("{sign}{n} ({list})")
    };
    let edit = t!(
        "analytics.coins.save_edit",
        plus = side('+', added),
        minus = side('−', removed)
    )
    .to_string();
    if n_targets > 1 {
        format!(
            "{edit} · {}",
            t!("analytics.coins.save_each", n = n_targets)
        )
    } else {
        edit
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
