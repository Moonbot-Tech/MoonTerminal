//! The "Coin lists" card: the `CoinsBlackList` and `CoinsWhiteList` fields as the user edits
//! them — chips that can be removed, restored and added by typing or pasting.
//!
//! Each field shows the WORKING list plus what the edit took out of the saved one: a saved
//! coin the user removed stays in the field, struck through with an undo, and an unsaved
//! addition has an amber outline. The value written on Save never includes a removed entry.
//!
//! The blacklist is ordered newest first and shaded by WHEN each coin entered the list: the
//! exact date on hover, the age as brightness. Brightness is relative to THIS list, not to the
//! calendar — the ramp spends its full range on whatever spread the list has, so it answers
//! "which of these came last", never "how old is this". A list saved in one go is uniformly
//! bright.
//!
//! The whitelist carries no dates, so its chips are plain and sorted by token.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonInputEvent, MoonInputState, MoonPalette, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;
use super::fields::{ChipKind, FieldChip, ListFieldActs, ListFieldModel};
use super::state::CoinLists;

use crate::load_state::LoadState;
use moon_core::db::coin_lists::{CoinListEntries, CoinListRows};

/// How faded the OLDEST coin is against the newest. Not down to invisible: an old entry is
/// still part of the value the user is about to save, so it has to stay readable.
const OLDEST_ALPHA: f32 = 0.42;

/// The floor is a legibility promise, so it is enforced where it cannot drift: lowering it
/// past this fails the build rather than quietly making old entries unreadable.
const _: () = assert!(OLDEST_ALPHA > 0.3);

/// One entry of a field as it draws it.
///
/// Every string here is built ONCE, when the field is folded — not per frame. A field can
/// hold hundreds of these and GPUI rebuilds the whole element tree on any repaint.
struct PickCoin {
    /// The entry AS SAVED, or the coin token for an unsaved addition. This exact string is
    /// what gets written to the field, so the chip and the saved value cannot drift.
    text: SharedString,
    /// The matching token the edit acts on (remove / restore).
    token: SharedString,
    /// Element id, stable across frames.
    id: SharedString,
    /// The hover line: coin, its cores, its date or its edit state.
    tip: SharedString,
    /// When this coin's presence begins, as far as anything can tell. `None` for an unsaved
    /// addition, for the whitelist, and for the few coins no retained version attests to.
    at: Option<i64>,
    /// In the working list but not written to any strategy yet.
    pending: bool,
    /// Saved, but taken out of the working list: shown struck through, never written.
    removed: bool,
    /// 0.0 = oldest, 1.0 = newest. Position on the brightness ramp.
    fresh: f32,
}

/// The built fields, kept across repaints.
struct FieldCache {
    /// Held by strong reference, so `ptr_eq` cannot be fooled by a reused allocation.
    src: Arc<CoinListRows>,
    /// The working-list revision this was built for — the fields follow the EDIT.
    rev: u64,
    /// Display zone used to precompute the hover dates.
    zone: chrono_tz::Tz,
    black: Vec<PickCoin>,
    white: Vec<PickCoin>,
    /// How many blacklist entries carry a date, counted once per rebuild for the probe.
    dated: usize,
}

/// State of the picker: the background read plus the fields built from it.
#[derive(Default)]
pub(in crate::analytics) struct CoinListsState {
    pub(in crate::analytics::tuner) rows: LoadState<CoinListRows>,
    /// Request generation, so a reply for a scope the user has left is discarded rather than
    /// published under the new selection's name.
    pub(in crate::analytics::tuner) seq: u64,
    field: Option<FieldCache>,
}

impl CoinListsState {
    /// The scope changed — retire the in-flight request and drop what it was for.
    ///
    /// Advances the generation even though no load starts here: without it a reply already on
    /// its way for the PREVIOUS selection passes its own check and publishes that selection's
    /// list under this one's heading.
    pub(in crate::analytics) fn invalidate(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        self.rows = LoadState::default();
        self.field = None;
    }

    /// Drop only the rendered fields so dates rebuild after the display zone changes.
    ///
    /// Returns:
    ///     Nothing; source rows and working edits remain intact.
    pub(in crate::analytics) fn invalidate_display_time(&mut self) {
        self.field = None;
    }
}

/// Build both fields unless the cache already holds them.
///
/// Args:
///     state: Coin-list state that owns the cache.
///     data: Loaded saved lists.
///     work: Current working lists.
///     saved: Persisted lists used to mark additions and removals.
///     rev: Working-list revision.
///     zone: Selected IANA display zone used by hover dates.
///
/// Returns:
///     The cached blacklist and whitelist entries.
fn fields_for<'a>(
    state: &'a mut CoinListsState,
    data: &Arc<CoinListRows>,
    work: &CoinLists,
    saved: &CoinLists,
    rev: u64,
    zone: chrono_tz::Tz,
) -> (&'a [PickCoin], &'a [PickCoin]) {
    let stale = state
        .field
        .as_ref()
        .is_none_or(|c| !Arc::ptr_eq(&c.src, data) || c.rev != rev || c.zone != zone);
    if stale {
        let black = build(data, &work.black, &saved.black, zone);
        let white = build_white(&data.white, &work.white, &saved.white);
        state.field = Some(FieldCache {
            dated: black.iter().filter(|c| c.at.is_some()).count(),
            src: Arc::clone(data),
            rev,
            zone,
            black,
            white,
        });
    }
    state
        .field
        .as_ref()
        .map(|c| (c.black.as_slice(), c.white.as_slice()))
        .unwrap_or_default()
}

/// What the saved lists say about one coin, gathered across the cores that hold it.
#[derive(Default)]
struct Saved<'a> {
    /// The entries as written, in first-seen order.
    entries: Vec<String>,
    cores: Vec<&'a str>,
    /// The most recent EXACT date across those cores; `None` if none of them observed the
    /// addition (the caller then falls back to `before`).
    since: Option<i64>,
    /// The EARLIEST bound across the cores that only have one — the earliest attestation is
    /// the strongest true claim.
    before: Option<i64>,
}

/// Fold the read into the blacklist field: the working list plus the saved coins the edit
/// removed, newest first, each with its cores and its place on the brightness ramp.
///
/// Args:
///     data: Loaded saved lists and their historical dates.
///     work: Current working blacklist tokens.
///     saved: Persisted blacklist tokens.
///     zone: Selected IANA display zone used by hover dates.
///
/// Returns:
///     Newest-first field entries with precomputed text, tooltip, and brightness.
fn build(
    data: &CoinListRows,
    work: &HashSet<String>,
    saved: &HashSet<String>,
    zone: chrono_tz::Tz,
) -> Vec<PickCoin> {
    let mut by_token: HashMap<&str, Saved> = HashMap::new();
    for row in &data.black {
        let slot = by_token.entry(row.coin.as_str()).or_default();
        for e in &row.entries {
            if !slot.entries.contains(e) {
                slot.entries.push(e.clone());
            }
        }
        if !slot.cores.contains(&row.core_name.as_str()) {
            slot.cores.push(row.core_name.as_str());
        }
        // An EXACT date outranks a bound, whichever core it came from.
        slot.since = match (slot.since, row.since_ms) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        slot.before = match (slot.before, row.before_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    let mut coins: Vec<PickCoin> = Vec::with_capacity(work.len());
    for token in work.union(saved) {
        let known = by_token.get(token.as_str());
        let pending = !saved.contains(token);
        let removed = !work.contains(token);
        // A saved coin writes back exactly the entries that are there — `BTC, BTC_0626` are
        // two entries matching one coin, and collapsing them would delete one on save.
        let mut entries: Vec<String> = known.map(|k| k.entries.clone()).unwrap_or_default();
        if entries.is_empty() {
            entries.push(token.clone());
        }
        let cores = known.map(|k| k.cores.join(", ")).unwrap_or_default();
        // Only a SAVED coin has a date; a pending addition begins now.
        let at = if pending {
            None
        } else {
            known.and_then(|k| k.since.or(k.before))
        };
        for text in entries {
            let date = at.map(|ms| moon_core::util::display_time::format_date(ms / 1000, zone));
            let tip = match (pending, removed, date) {
                (true, _, _) => format!("{text} · {}", t!("analytics.coins.pending")),
                (false, true, _) => format!(
                    "{text} · {} · {}",
                    cores,
                    t!("analytics.coins.chip_removed")
                ),
                (false, false, Some(d)) if !d.is_empty() => format!("{text} · {cores} · {d}"),
                (false, false, _) => {
                    format!("{text} · {cores} · {}", t!("analytics.coins.no_date"))
                }
            };
            coins.push(PickCoin {
                id: SharedString::from(format!("an-pick-b-{text}")),
                text: SharedString::from(text),
                token: SharedString::from(token.clone()),
                tip: SharedString::from(tip),
                at,
                pending,
                removed,
                fresh: 0.0,
            });
        }
    }
    // Newest first. A pending addition is the newest thing there is, so it leads.
    coins.sort_by(|a, b| {
        b.pending
            .cmp(&a.pending)
            .then_with(|| b.at.cmp(&a.at))
            .then_with(|| a.text.cmp(&b.text))
    });

    // The ramp runs over THIS list's own span: oldest entry at the dim end, newest at the
    // bright end. Differences, never the timestamps themselves: `f32` resolves a unix-ms
    // value only to about a day.
    let dates: Vec<i64> = coins.iter().filter_map(|c| c.at).collect();
    let (oldest, newest) = match (dates.iter().min(), dates.iter().max()) {
        (Some(lo), Some(hi)) => (*lo, *hi),
        _ => (0, 0),
    };
    let span = (newest - oldest) as f32;
    for c in &mut coins {
        c.fresh = match c.at {
            _ if c.pending => 1.0,
            // Saved in one go: nothing is older, so nothing is dimmed.
            Some(_) if span <= 0.0 => 1.0,
            Some(at) => ((at - oldest) as f32 / span).clamp(0.0, 1.0),
            // Nothing known at all — it predates everything we can see.
            None => 0.0,
        };
    }
    coins
}

/// Fold the whitelist field: working plus removed coins, sorted by token, no dates.
///
/// Args:
///     saved_rows: The strategies' whitelist entries as written.
///     work: Current working whitelist tokens.
///     saved: Persisted whitelist tokens.
///
/// Returns:
///     Field entries ordered by token, then by entry.
fn build_white(
    saved_rows: &[CoinListEntries],
    work: &HashSet<String>,
    saved: &HashSet<String>,
) -> Vec<PickCoin> {
    let by_token = entries_by_token(saved_rows);
    let tokens: BTreeSet<&String> = work.union(saved).collect();
    let mut coins = Vec::with_capacity(tokens.len());
    for token in tokens {
        let pending = !saved.contains(token);
        let removed = !work.contains(token);
        let entries: Vec<String> = match by_token.get(token.as_str()) {
            Some(entries) if !entries.is_empty() => entries.iter().map(|e| e.to_string()).collect(),
            _ => vec![token.clone()],
        };
        for text in entries {
            let tip = if pending {
                format!("{text} · {}", t!("analytics.coins.pending"))
            } else if removed {
                format!("{text} · {}", t!("analytics.coins.chip_removed"))
            } else {
                text.clone()
            };
            coins.push(PickCoin {
                id: SharedString::from(format!("an-pick-w-{text}")),
                text: SharedString::from(text),
                token: SharedString::from(token.clone()),
                tip: SharedString::from(tip),
                at: None,
                pending,
                removed,
                fresh: 1.0,
            });
        }
    }
    coins
}

/// The whitelist's saved entries per token, deduplicated, in first-seen order.
fn entries_by_token(saved: &[CoinListEntries]) -> HashMap<&str, Vec<&str>> {
    let mut by_token: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in saved {
        let slot = by_token.entry(r.coin.as_str()).or_default();
        for e in &r.entries {
            if !slot.contains(&e.as_str()) {
                slot.push(e.as_str());
            }
        }
    }
    by_token
}

/// The whitelist's value: the entries the strategies hold for every coin still listed, plus
/// the bare token for one added but never saved. Ordered by token, so two identical saves
/// produce one string.
fn white_value(saved: &[CoinListEntries], work: &HashSet<String>) -> String {
    let by_token = entries_by_token(saved);
    let mut tokens: Vec<&String> = work.iter().collect();
    tokens.sort();
    let mut out: Vec<String> = Vec::new();
    for t in tokens {
        match by_token.get(t.as_str()) {
            Some(entries) => out.extend(entries.iter().map(|e| (*e).to_string())),
            None => out.push(t.clone()),
        }
    }
    out.join(", ")
}

/// The blacklist's value as written: every entry still in the working list, newest first.
///
/// `cached` is the field already built for the same read, edit and zone; without it the
/// field is folded afresh.
fn black_value(
    data: &CoinListRows,
    work: &HashSet<String>,
    saved: &HashSet<String>,
    zone: chrono_tz::Tz,
    cached: Option<&[PickCoin]>,
) -> String {
    let built;
    let coins = match cached {
        Some(coins) => coins,
        None => {
            built = build(data, work, saved, zone);
            built.as_slice()
        }
    };
    coins
        .iter()
        .filter(|c| !c.removed)
        .map(|c| c.text.as_ref())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The coin-list fields to write, and ONLY the ones that actually changed.
///
/// Every write is a whole-field overwrite, so naming a field the user did not touch would
/// push a snapshot of it back over whatever the core has since put there.
///
/// Args:
///     data: The saved lists, `None` while they have not been read.
///     work: Current working lists.
///     saved: Persisted lists.
///     zone: Display zone (orders nothing written; only the shared fold needs it).
///     cached: The blacklist field already built for this read and edit, if current.
///
/// Returns:
///     `None` while the saved lists are unread: without them the entries as written are
///     unknown, and writing folded tokens back would delete every contract-suffixed entry.
fn list_changes(
    data: Option<&CoinListRows>,
    work: &CoinLists,
    saved: &CoinLists,
    zone: chrono_tz::Tz,
    cached: Option<&[PickCoin]>,
) -> Option<Vec<(String, String)>> {
    let data = data?;
    let mut out = Vec::new();
    if work.black != saved.black {
        out.push((
            super::save::FIELD.to_string(),
            black_value(data, &work.black, &saved.black, zone, cached),
        ));
    }
    if work.white != saved.white {
        out.push((
            super::save::WHITE_FIELD.to_string(),
            white_value(&data.white, &work.white),
        ));
    }
    Some(out)
}

/// One field entry as the shared chip renderer draws it.
fn chip_of(c: &PickCoin) -> FieldChip {
    FieldChip {
        token: c.token.clone(),
        text: c.text.clone(),
        tip: c.tip.clone(),
        id: c.id.clone(),
        kind: if c.removed {
            ChipKind::Removed
        } else if c.pending {
            ChipKind::Added
        } else {
            ChipKind::Saved
        },
        // Brightness carries the age; fading is by alpha over the normal text colour.
        alpha: OLDEST_ALPHA + (1.0 - OLDEST_ALPHA) * c.fresh,
    }
}

impl AnalyticsView {
    /// The coin-list fields to write, and ONLY the ones that actually changed (see
    /// [`list_changes`]). This VALUE is what "Make a copy" writes; Save reads only the KEYS
    /// and sends the user's edit replayed onto each strategy's own live list instead.
    pub(in crate::analytics::tuner) fn coin_list_changes(&self) -> Option<Vec<(String, String)>> {
        list_changes(
            self.coin_lists.rows.data().map(|d| d.as_ref()),
            &self.coins.work,
            &self.coins.saved,
            self.display_zone,
            self.cached_black(),
        )
    }

    /// The built blacklist field, when it was built for the current read, edit and zone.
    fn cached_black(&self) -> Option<&[PickCoin]> {
        let data = self.coin_lists.rows.data()?;
        self.coin_lists
            .field
            .as_ref()
            .filter(|c| {
                Arc::ptr_eq(&c.src, data)
                    && c.rev == self.coins.lists_rev
                    && c.zone == self.display_zone
            })
            .map(|c| c.black.as_slice())
    }

    /// Lazily-created typing field of one list: Enter adds what was typed, and a paste (or
    /// any separator typed) adds every coin in it at once.
    fn coin_list_input(
        &mut self,
        black: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<MoonInputState> {
        let slot = if black {
            &self.coins.black_input
        } else {
            &self.coins.white_input
        };
        if let Some(s) = slot {
            return s.clone();
        }
        let state = cx.new(|cx| MoonInputState::new(window, cx));
        cx.subscribe_in(
            &state,
            window,
            move |this, state, ev: &MoonInputEvent, window, cx| {
                let raw = state.read(cx).value().to_string();
                let commit = match ev {
                    MoonInputEvent::PressEnter { .. } => true,
                    // The strategy field's own separators: a pasted list commits at once.
                    MoonInputEvent::Change => raw.contains([',', ';', ' ', '\n', '\r', '\t']),
                    _ => false,
                };
                if !commit || raw.trim().is_empty() {
                    return;
                }
                this.coins.add_tokens(black, &raw);
                state.update(cx, |input, cx| input.set_value("", window, cx));
                this.arm_coin_kpi(cx);
                cx.notify();
            },
        )
        .detach();
        if black {
            self.coins.black_input = Some(state.clone());
        } else {
            self.coins.white_input = Some(state.clone());
        }
        state
    }

    /// One list's current value, comma separated as the field holds it; `None` while unread.
    fn coins_list_value(&self, black: bool) -> Option<String> {
        let data = self.coin_lists.rows.data()?;
        let value = if black {
            black_value(
                data,
                &self.coins.work.black,
                &self.coins.saved.black,
                self.display_zone,
                self.cached_black(),
            )
        } else {
            white_value(&data.white, &self.coins.work.white)
        };
        Some(value)
    }

    /// The handlers of one field, each going through the same KPI rescan as a tick.
    fn coin_field_acts(&self, black: bool, cx: &Context<Self>) -> ListFieldActs {
        let view = cx.entity();
        let edit = move |f: Rc<dyn Fn(&mut super::state::CoinsState, &str)>| {
            let view = view.clone();
            Rc::new(move |token: &SharedString, _: &mut Window, app: &mut App| {
                view.update(app, |this, cx| {
                    f(&mut this.coins, token.as_ref());
                    this.arm_coin_kpi(cx);
                    cx.notify();
                });
            }) as super::fields::TokenAct
        };
        let on_remove = edit(Rc::new(move |s, t| s.remove_token(black, t)));
        let on_restore = edit(Rc::new(move |s, t| s.restore_token(black, t)));
        let view = cx.entity();
        let on_clear: super::fields::Act = {
            let view = view.clone();
            Rc::new(move |_: &mut Window, app: &mut App| {
                view.update(app, |this, cx| {
                    this.coins.clear(black);
                    this.arm_coin_kpi(cx);
                    cx.notify();
                });
            })
        };
        let on_copy: super::fields::Act = Rc::new(move |_: &mut Window, app: &mut App| {
            if let Some(value) = view.read(app).coins_list_value(black) {
                app.write_to_clipboard(ClipboardItem::new_string(value));
            }
        });
        ListFieldActs {
            on_remove,
            on_restore,
            on_copy,
            on_clear,
        }
    }

    /// The card: the shared Save/Copy toolbar, the blacklist over the whitelist, and the
    /// "Not saved" band while an edit is pending. A retained read-hidden selection shows placeholders.
    pub(in crate::analytics::tuner) fn coins_lists_card(
        &mut self,
        p: MoonPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.visible_target_count(self.read_core_ids()) > 0;
        let inputs = selected.then(|| {
            (
                self.coin_list_input(true, window, cx),
                self.coin_list_input(false, window, cx),
            )
        });
        // `|_| false`: an empty list is answered by the field itself, not by the shared note,
        // which speaks about closed trades in a period.
        let ready = self.coin_lists.rows.view(|_| false).map(Arc::clone);
        let mut counts: Option<(usize, usize)> = None;
        let body: AnyElement = match (selected, ready) {
            (false, _) => self.coin_fields_el(
                ListFieldModel::new(true, None),
                ListFieldModel::new(false, None),
                None,
                p,
                cx,
            ),
            (true, Err(note)) => crate::load_state::note_el("an-coin-pick-note", note, 10.0, p, cx),
            (true, Ok(data)) => {
                let display_zone = self.display_zone;
                let Self {
                    coins: edit,
                    coin_lists: state,
                    ..
                } = self;
                let (black, white) = fields_for(
                    state,
                    &data,
                    &edit.work,
                    &edit.saved,
                    edit.lists_rev,
                    display_zone,
                );
                let black_m = ListFieldModel::new(true, Some(black.iter().map(chip_of).collect()));
                let white_m = ListFieldModel::new(false, Some(white.iter().map(chip_of).collect()));
                counts = Some((black_m.count.unwrap_or(0), white_m.count.unwrap_or(0)));
                self.coin_fields_el(black_m, white_m, inputs, p, cx)
            }
        };
        if super::super::super::probe_enabled() {
            super::probe(
                "coinpick",
                format!(
                    "coinpick scope={} coins={} white={} dated={}",
                    self.scope_label(),
                    counts.map_or(-1, |(b, _)| b as i64),
                    counts.map_or(-1, |(_, w)| w as i64),
                    // Only when this render actually built the fields.
                    counts
                        .and(self.coin_lists.field.as_ref().map(|f| f.dated))
                        .unwrap_or(0),
                ),
            );
        }
        v_flex().w_full().flex_none().child(body).into_any_element()
    }

    /// Both fields, blacklist over whitelist.
    fn coin_fields_el(
        &self,
        black: ListFieldModel,
        white: ListFieldModel,
        inputs: Option<(Entity<MoonInputState>, Entity<MoonInputState>)>,
        p: MoonPalette,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (black_in, white_in) = match inputs {
            Some((b, w)) => (Some(b), Some(w)),
            None => (None, None),
        };
        v_flex()
            .w_full()
            .child(super::fields::list_field_el(
                &black,
                black_in.as_ref(),
                &self.coin_field_acts(true, cx),
                p,
                cx,
            ))
            .child(super::fields::list_field_el(
                &white,
                white_in.as_ref(),
                &self.coin_field_acts(false, cx),
                p,
                cx,
            ))
            .into_any_element()
    }
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
