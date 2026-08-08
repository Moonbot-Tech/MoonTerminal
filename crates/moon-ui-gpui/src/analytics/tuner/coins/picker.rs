//! The coin picker: one big `CoinsBlackList` field, written the way Moonbot writes it —
//! coins separated by commas — but ordered and shaded by WHEN each entered the list.
//!
//! This replaced two sortable tables. A table answers "what is in the list"; this field is
//! the VALUE itself, in the form it will be saved in — which is what a review of the list
//! actually needs to see.
//!
//! Two things the plain text cannot carry are attached per coin: the exact date, on hover,
//! and its age, as brightness. Newest first and brightest, oldest last and dimmest — so the
//! recent decisions, the ones actually being reviewed, are the ones that stand out.
//!
//! Brightness is relative to THIS list, not to the calendar: the question being asked is
//! "what changed in this strategy last", and that has no answer outside the list. A list
//! saved in one go is therefore uniformly bright whatever day that was — every coin in it is
//! equally the latest — and a list with a spread runs from the newest at full brightness down
//! to the oldest.
//!
//! The consequence, stated because it is easy to misread: the ramp always spends its full
//! range on whatever spread the list happens to have, so a gap of minutes looks like a gap of
//! months. It answers "which of these came last", never "how old is this".
//!
//! Only the BLACKLIST is DRAWN here. The whitelist is read too — both lists are editable in
//! the coin table and both are written on save — but it carries no dates and no field of its
//! own, so it exists only as the entries `coin_list_changes` writes back. See
//! `moon_core::db::coin_lists`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::*;
use moon_ui::{MoonPalette, MoonTooltipView, h_flex, v_flex};
use rust_i18n::t;

use super::super::super::AnalyticsView;

use crate::design;
use crate::design::{moon, moon_alpha};
use crate::load_state::LoadState;
use moon_core::db::coin_lists::{CoinListEntries, CoinListRows};

/// How faded the OLDEST coin is against the newest. Not down to invisible: an old entry is
/// still part of the value the user is about to save, so it has to stay readable.
const OLDEST_ALPHA: f32 = 0.42;

/// The floor is a legibility promise, so it is enforced where it cannot drift: lowering it
/// past this fails the build rather than quietly making old entries unreadable.
const _: () = assert!(OLDEST_ALPHA > 0.3);

/// One entry of the field as it draws it.
///
/// Every string here is built ONCE, when the field is folded — not per frame. The card can
/// hold hundreds of these and GPUI rebuilds the whole element tree on any repaint, so a
/// `format!` in the chip builder is a `format!` per coin per frame.
struct PickCoin {
    /// The entry AS SAVED, or the coin token for an unsaved tick. This exact string is what
    /// gets written to `CoinsBlackList` — the field the user reads and the value that is
    /// saved are built from one place, so they cannot drift.
    text: SharedString,
    /// The same with its separator attached, for drawing. Precomputed like everything else
    /// here: the card holds hundreds of these and rebuilds them on every repaint.
    display: SharedString,
    /// Element id, stable across frames.
    id: SharedString,
    /// The hover line: coin, its cores, its date.
    tip: SharedString,
    /// When this coin's presence begins, as far as anything can tell. `None` for an unsaved
    /// tick (it begins now) and for the few coins no retained version can attest to.
    at: Option<i64>,
    /// Ticked in the table but not written to any strategy yet.
    pending: bool,
    /// 0.0 = oldest, 1.0 = newest. Position on the brightness ramp.
    fresh: f32,
}

/// Ceiling on how many entries are drawn.
///
/// Counted in ENTRIES, not coins — a list may spell one coin several ways. Measured on the
/// real database: the biggest single list is 474 and the union over every strategy is 819,
/// so this clears both. Past it the field states how many it left out rather than looking
/// complete.
const MAX_CHIPS: usize = 1000;

/// The built field, kept across repaints.
struct FieldCache {
    /// Held by strong reference, so `ptr_eq` cannot be fooled by a reused allocation.
    src: Arc<CoinListRows>,
    /// The working-list revision this was built for — the field follows the EDIT, so a tick
    /// must rebuild it even though the background read has not moved.
    rev: u64,
    /// Display zone used to precompute the hover dates.
    zone: chrono_tz::Tz,
    coins: Vec<PickCoin>,
    /// How many of them carry a date, counted here rather than in the probe: the probe runs
    /// on every frame it is armed for, and this does not change between rebuilds.
    dated: usize,
}

/// State of the picker: the background read plus the field built from it.
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

    /// Drop only the rendered field so dates rebuild after the display zone changes.
    ///
    /// Returns:
    ///     Nothing; source rows and working edits remain intact.
    pub(in crate::analytics) fn invalidate_display_time(&mut self) {
        self.field = None;
    }
}

/// Build the field unless the cache already holds it.
///
/// Built once per read AND per edit, never per frame: GPUI repaints the whole view on any
/// hover, and this walks and sorts every entry of the list.
///
/// Args:
///     state: Coin-list state that owns the cache.
///     data: Loaded saved lists.
///     work: Current working blacklist tokens.
///     saved: Persisted blacklist tokens used to mark pending entries.
///     rev: Working-list revision.
///     zone: Selected IANA display zone used by hover dates.
///
/// Returns:
///     Cached or freshly built field entries borrowed from `state`.
fn field_for<'a>(
    state: &'a mut CoinListsState,
    data: &Arc<CoinListRows>,
    work: &HashSet<String>,
    saved: &HashSet<String>,
    rev: u64,
    zone: chrono_tz::Tz,
) -> &'a [PickCoin] {
    let stale = state
        .field
        .as_ref()
        .is_none_or(|c| !Arc::ptr_eq(&c.src, data) || c.rev != rev || c.zone != zone);
    if stale {
        let coins = build(data, work, saved, zone);
        state.field = Some(FieldCache {
            dated: coins.iter().filter(|c| c.at.is_some()).count(),
            src: Arc::clone(data),
            rev,
            zone,
            coins,
        });
    }
    state
        .field
        .as_ref()
        .map(|c| c.coins.as_slice())
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
    /// The EARLIEST bound across the cores that only have one — the same MIN the read
    /// applies within a core, and for the same reason: the earliest attestation is the
    /// strongest true claim. Taking whichever came first in the vec picked the weakest.
    before: Option<i64>,
}

/// Fold the read into the field's entries: the WORKING list, newest first, each carrying its
/// cores and its place on the brightness ramp.
///
/// The working list is the point. The field is labelled with the strategy parameter and is
/// the value the picking produces, so it has to follow the tick boxes: a coin just ticked
/// appears, one just unticked leaves. Rendering the saved value instead made the field sit
/// still while the badge beside it counted the edits.
///
/// Args:
///     data: Loaded saved lists and their historical dates.
///     work: Current working blacklist tokens.
///     saved: Persisted blacklist tokens used to mark pending entries.
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
    // What the SAVED lists hold, indexed by token, so a working coin can pick up its
    // entries, its cores and its date.
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
        // An EXACT date outranks a bound, whichever core it came from: the read already
        // suppresses the bound wherever it knows the real thing, and merging on
        // `effective_ms` here put one core's "no later than" over another's actual date.
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
    for token in work {
        let known = by_token.get(token.as_str());
        let pending = !saved.contains(token);
        // A coin present in the saved lists writes back exactly the entries that are there —
        // `BTC, BTC_0626, BTC_0925` are three entries matching one coin, and collapsing them
        // to `BTC` would delete two of them the moment this value is saved.
        let mut entries: Vec<String> = known.map(|k| k.entries.clone()).unwrap_or_default();
        if entries.is_empty() {
            entries.push(token.clone());
        }
        let cores = known.map(|k| k.cores.join(", ")).unwrap_or_default();
        // Only a SAVED coin has a date; a pending tick begins now, which no history records.
        let at = if pending {
            None
        } else {
            known.and_then(|k| k.since.or(k.before))
        };
        for text in entries {
            let tip = match (
                pending,
                at.map(|ms| moon_core::util::display_time::format_date(ms / 1000, zone)),
            ) {
                // An unsaved tick has no date and no core: saying so is the point, since it
                // is the one entry the strategy does not hold yet.
                (true, _) => format!("{text} · {}", t!("analytics.coins.pending")),
                (false, Some(d)) if !d.is_empty() => format!("{text} · {cores} · {d}"),
                (false, _) => format!("{text} · {cores} · {}", t!("analytics.coins.no_date")),
            };
            coins.push(PickCoin {
                id: SharedString::from(format!("an-pick-{text}")),
                display: SharedString::from(text.clone()),
                text: SharedString::from(text),
                tip: SharedString::from(tip),
                at,
                pending,
                fresh: 0.0,
            });
        }
    }
    // Newest first. A pending tick is the newest thing there is — it was added just now —
    // so it leads, which is exactly "the last written coin comes first".
    coins.sort_by(|a, b| {
        b.pending
            .cmp(&a.pending)
            .then_with(|| b.at.cmp(&a.at))
            .then_with(|| a.text.cmp(&b.text))
    });

    // The separator belongs to the entry BEFORE it, so a wrap never orphans a comma at the
    // start of a line. Attached here, once, rather than in the per-frame chip builder.
    let last = coins.len().saturating_sub(1);
    for (i, c) in coins.iter_mut().enumerate() {
        if i != last {
            c.display = SharedString::from(format!("{}, ", c.text));
        }
    }

    // The ramp runs over THIS list's own span: oldest entry at the dim end, newest at the
    // bright end. Not over the calendar — the list is what the eye is comparing.
    let dates: Vec<i64> = coins.iter().filter_map(|c| c.at).collect();
    let (oldest, newest) = match (dates.iter().min(), dates.iter().max()) {
        (Some(lo), Some(hi)) => (*lo, *hi),
        _ => (0, 0),
    };
    // Differences, never the timestamps themselves: a unix-ms value is ~1.8e12 and `f32`
    // resolves it only to about a day, while the gaps this ramp is made of are far smaller.
    let span = (newest - oldest) as f32;
    for c in &mut coins {
        c.fresh = match c.at {
            // Not saved yet — newer than anything with a date.
            _ if c.pending => 1.0,
            // Saved in one go: there is no "older" here, so nothing is dimmed. This is the
            // common case — most real lists carry a single date — and the guard is what
            // keeps a zero span out of the division below.
            Some(_) if span <= 0.0 => 1.0,
            Some(at) => ((at - oldest) as f32 / span).clamp(0.0, 1.0),
            // Nothing known at all — it predates everything we can see, so it takes the far
            // end of the ramp rather than a colour of its own.
            None => 0.0,
        };
    }
    coins
}

/// The whitelist's value: the entries the strategies hold for every coin still ticked, plus
/// the bare token for one ticked but never saved.
///
/// Ordered by token rather than by date — nothing reads a whitelist date, and a stable order
/// is what keeps two identical saves from producing two different strings.
fn white_value(saved: &[CoinListEntries], work: &HashSet<String>) -> String {
    let mut by_token: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in saved {
        let slot = by_token.entry(r.coin.as_str()).or_default();
        for e in &r.entries {
            if !slot.contains(&e.as_str()) {
                slot.push(e.as_str());
            }
        }
    }
    let mut tokens: Vec<&String> = work.iter().collect();
    tokens.sort();
    let mut out: Vec<String> = Vec::new();
    for t in tokens {
        match by_token.get(t.as_str()) {
            // As written: `BTC, BTC_0626` are two entries of one coin, and collapsing them
            // to the token would delete one of them on save.
            Some(entries) => out.extend(entries.iter().map(|e| (*e).to_string())),
            None => out.push(t.clone()),
        }
    }
    out.join(", ")
}

impl AnalyticsView {
    /// The coin-list fields to write, and ONLY the ones that actually changed.
    ///
    /// Both lists are here because the coin table can tick BOTH: writing only the blacklist
    /// meant a whitelist edit lit the Save button, went nowhere, and was then erased by the
    /// reload — an edit lost without a word.
    ///
    /// Writing only what MOVED is not a tidiness choice. Every write is a whole-field
    /// overwrite, so naming a field the user did not touch would push a snapshot of it back
    /// over whatever the core has since put there.
    ///
    /// `None` while the saved lists have not been read: without them the entries as written
    /// are unknown, and writing folded tokens back would delete every contract-suffixed entry
    /// the strategy holds.
    pub(in crate::analytics::tuner) fn coin_list_changes(&self) -> Option<Vec<(String, String)>> {
        let data = self.coin_lists.rows.data()?;
        let mut out = Vec::new();
        if self.coins.work.black != self.coins.saved.black {
            // The same fold the card draws. This VALUE is what "Make a copy" writes; Save
            // reads only the KEYS from here and sends the user's edit replayed onto each
            // strategy's own live list instead.
            let coins = build(
                data,
                &self.coins.work.black,
                &self.coins.saved.black,
                self.display_zone,
            );
            out.push((
                super::save::FIELD.to_string(),
                coins
                    .iter()
                    .map(|c| c.text.as_ref())
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
        if self.coins.work.white != self.coins.saved.white {
            out.push((
                super::save::WHITE_FIELD.to_string(),
                white_value(&data.white, &self.coins.work.white),
            ));
        }
        Some(out)
    }

    /// The card: the `CoinsBlackList` field the tick boxes build, plus the shared Save/Copy
    /// toolbar that writes it.
    pub(in crate::analytics::tuner) fn coins_field_card(
        &mut self,
        p: MoonPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // `|_| false`: an empty list is answered in words below, not by the shared note —
        // that one says "no closed trades in this period", a sentence about a period this
        // card does not have.
        let ready = self.coin_lists.rows.view(|_| false).map(Arc::clone);
        let picked_any = self.sel_strategy.is_some();
        let (body, count) = match ready {
            Err(note) => (
                crate::load_state::note_el("an-coin-pick-note", note, 10.0, p, cx),
                None,
            ),
            // (the `dated` probe counter is taken from `count` below, so a state that never
            // built a field cannot report the previous one's numbers)
            Ok(data) => {
                // The field follows the EDIT: `work` is what the tick boxes have built,
                // `saved` only decides which entries count as not-yet-written.
                //
                // Borrowed through a destructure rather than cloned: the two sets hold a
                // String per coin, and cloning them per render would cost more allocations
                // every frame than the whole rest of this card — which is exactly what
                // precomputing the chip strings was for. `coins` and `coin_lists` are
                // separate fields, so the split borrow is sound.
                let display_zone = self.display_zone;
                let Self {
                    coins: edit,
                    coin_lists: state,
                    ..
                } = self;
                let coins = field_for(
                    state,
                    &data,
                    &edit.work.black,
                    &edit.saved.black,
                    edit.lists_rev,
                    display_zone,
                );
                if coins.is_empty() {
                    (
                        div()
                            .w_full()
                            .text_size(design::t_caption(cx))
                            .text_color(moon(p.text_muted))
                            .child(if picked_any {
                                t!("analytics.coins.list_empty").to_string()
                            } else {
                                t!("analytics.coins.list_pick").to_string()
                            })
                            .into_any_element(),
                        picked_any.then_some(0usize),
                    )
                } else {
                    let n = coins.len();
                    // Every entry is its own element with its own tooltip, and the tree is
                    // rebuilt on each repaint — so the count is bounded. The cut is stated
                    // rather than silent: a field labelled with the strategy parameter must
                    // never look complete when it is not.
                    let shown = n.min(MAX_CHIPS);
                    let mut items: Vec<AnyElement> = coins[..shown]
                        .iter()
                        .map(|c| coin_chip(c, p).into_any_element())
                        .collect();
                    if n > shown {
                        items.push(
                            div()
                                .flex_none()
                                .text_color(moon(p.text_muted))
                                .child(t!("analytics.coins.more", n = n - shown).to_string())
                                .into_any_element(),
                        );
                    }
                    (
                        // Wrapping, so it reads as one comma-separated value rather than a
                        // column of chips — this IS the field's text, only shaded.
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .children(items)
                            .into_any_element(),
                        Some(n),
                    )
                }
            }
        };
        if super::super::super::probe_enabled() {
            super::probe(
                "coinpick",
                format!(
                    "coinpick scope={} coins={} dated={}",
                    self.scope_label(),
                    count.map_or(-1, |n| n as i64),
                    // Only when this render actually built a field; otherwise the cache
                    // still holds the PREVIOUS selection's numbers.
                    count
                        .and(self.coin_lists.field.as_ref().map(|f| f.dated))
                        .unwrap_or(0),
                ),
            );
        }
        v_flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .rounded(design::ui_px(cx, 8.0))
            .bg(moon(p.panel))
            .border_1()
            .border_color(moon(p.border))
            .overflow_hidden()
            // The SHARED tuner toolbar, same as the other two axes: title, then Copy and
            // Save. Writing the list is not a different kind of act from writing a
            // threshold, so it must not grow its own buttons.
            .child(self.shell_toolbar(
                super::super::shared::TunerKind::Coins,
                match count {
                    Some(n) => format!("{} {n}", t!("analytics.coins.pick_title")),
                    None => t!("analytics.coins.pick_title").to_string(),
                },
                p,
                cx,
            ))
            // The field's own label, spelled as the strategy parameter it will be written to
            // — the name the user already knows from Moonbot, not a translated paraphrase.
            .child(
                div()
                    .w_full()
                    .px(design::ui_px(cx, 8.0))
                    .pb(design::ui_px(cx, 4.0))
                    .text_size(design::t_caption(cx))
                    .text_color(moon(p.text_muted))
                    .child(super::save::FIELD),
            )
            // A recessed, bordered well rather than bare text on the card: this is an input
            // in everything but editability, and it has to read as one. `shell` is the
            // theme's own "below the panel" surface, so it stays recessed in either theme
            // instead of hardcoding a dark grey.
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .px(design::ui_px(cx, 8.0))
                    .pb(design::ui_px(cx, 8.0))
                    .child(
                        div()
                            .id("an-coin-pick-field")
                            .size_full()
                            .min_h_0()
                            .overflow_y_scroll()
                            .rounded(design::ui_px(cx, 6.0))
                            .bg(moon(p.shell))
                            .border_1()
                            .border_color(moon(p.border_soft))
                            .p(design::ui_px(cx, 8.0))
                            .text_size(design::t_body(cx))
                            .child(body),
                    ),
            )
            .into_any_element()
    }
}

/// One entry in the field: its text, its shade and its date on hover.
///
/// Deliberately not interactive. It carries a tooltip, but no hover style and no click: on
/// this page amber-on-hover means "this does something", and a `hover` style on every entry
/// also makes GPUI notify the view on each boundary the cursor crosses — hundreds of times
/// per sweep across a wrapped field, each one rebuilding every entry.
fn coin_chip(c: &PickCoin, p: MoonPalette) -> impl IntoElement + use<> {
    // Brightness carries the age. The theme is DARK, so "newer stands out" means BRIGHTER —
    // the opposite of the same idea on paper, where fresh ink is the darkest thing on the
    // page. Fading is by alpha over the normal text colour, so the ramp follows the theme
    // instead of hardcoding a grey.
    let alpha = OLDEST_ALPHA + (1.0 - OLDEST_ALPHA) * c.fresh;
    let tip = c.tip.clone();
    div()
        .id(c.id.clone())
        .flex_none()
        // An unsaved tick is the one entry the strategy does not hold yet, so it is marked as
        // an edit — the same amber the coin table uses for exactly that.
        .text_color(if c.pending {
            moon(p.amber)
        } else {
            moon_alpha(p.text, alpha)
        })
        .tooltip(move |_w, cx| cx.new(|_| MoonTooltipView::new(tip.clone())).into())
        .child(c.display.clone())
}

// Explicit imports, never `use super::*`: the parent re-exports `gpui::*`, whose own `test`
// shadows the built-in attribute and makes `#[test]` expand recursively.
#[cfg(test)]
mod tests;
