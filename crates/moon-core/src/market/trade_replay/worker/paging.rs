//! Bounded trade-page walks and interrupted coverage.

use super::*;

/// Walk every tile of one tick stage's [`TickPlan`], paginating each with the venue's own cursor,
/// until the plan is exhausted or a stop condition is reached — and, unlike a candle job, a stop
/// never discards what was already collected except when the window itself closed.
///
/// `fetch` is the injected seam — no network, no clock, no gate inside this function, which is
/// what makes it testable with a fake fetcher and a fake [`TickObserver`].
///
/// The loop rule, in order of precedence (D2-2, replacing this function's earlier design in
/// full):
/// - [`cancelled`] stops EVERYTHING, always, and discards whatever was collected — the window is
///   gone and there is no one left to serve it to.
/// - The normal deadline and page budget cannot interrupt the leading trade-only tiles. These
///   use the bounded trade deadline and [`TRADE_PAGE_BUDGET`] allowance instead. After the trade
///   completes, normal limits apply immediately, before any optional margin is requested.
///   Hard stops retain the truthful partial harvest even if the entire trade could not fit.
/// - A venue's own answer — `Transient`/`UnknownSymbol` — also stops the walk rather than the
///   whole stage, and marks the harvest [`TickHarvest::venue_refused`], so [`serve_ticks`] knows
///   not to clear a refusal the venue just gave it.
/// - The FOCUS tiles (the leading [`TickPlan::focus_len`] entries) are never truncated by the
///   tick budget: it is checked only around a NON-focus tile, before it starts and again once it
///   finishes, so a tile is walked whole or not at all — never cut mid-body.
/// - Every page is clipped to the SLICE it was fetched for before it is counted toward any budget
///   (D2-1): Binance's forward pager and OKX's backward one both routinely return a page that
///   overshoots its own slice edge, and [`Tick`] carries no exchange trade id, so an unclipped
///   overlap between two adjacent slices is undetectable once concatenated, not merely unnoticed.
///   The aggregate clip to [`TickHarvest::covered`] in [`serve_ticks`] is the OUTER bound and does
///   not replace this inner one.
/// - The verdict is one question: is the harvest empty? A non-empty one is always `Ready`,
///   whatever stopped the walk; only an empty one reaches [`TickVerdict::Abandoned`], carrying
///   whichever reason actually stopped it.
///
/// Args:
///     route: Which venue endpoint this stage answers.
///     plan: The window's own [`tick_plan`] output — tiles in fetch-priority order, with the
///         first [`TickPlan::focus_len`] of them being the trade's own focus.
///     tick_budget: Ceiling on the total ticks collected before a non-focus tile is skipped.
///     page_budget: Normal page ceiling; trade tiles get at least [`TRADE_PAGE_BUDGET`].
///     cancelled: Answers whether the requester's window has closed.
///     expired: Answers whether the deadline has passed; `true` selects the hard trade deadline.
///     observer: Records the `claim`/`pace` calls this stage makes.
///     fetch: Fetches one page for a given slice and cursor.
///
/// Returns:
///     The harvest, or the reason nothing was collected.
pub(crate) fn paginate_ticks<F, O>(
    route: TradeRoute,
    plan: &TickPlan,
    tick_budget: usize,
    page_budget: usize,
    cancelled: impl Fn() -> bool,
    expired: impl Fn(bool) -> bool,
    observer: &mut O,
    mut fetch: F,
) -> TickVerdict
where
    F: FnMut(i64, i64, Option<rest::TradeCursor>) -> Result<rest::TradePage, rest::FetchError>,
    O: TickObserver,
{
    if plan.slices.is_empty() {
        return TickVerdict::Abandoned(TickAbandon::Empty);
    }
    if observer.claim(route.host()).is_err() {
        return TickVerdict::Abandoned(TickAbandon::RateLimited);
    }
    let mut ticks: Vec<Tick> = Vec::new();
    let mut pages_fetched = 0usize;
    // Completed tiles, coalesced where they abut: one stretch while the walk stays contiguous,
    // two once it crosses to a long position's other neighbourhood.
    let mut covered = Coverage::none();
    let mut complete = true;
    let mut venue_refused = false;
    let mut stop_reason: Option<TickAbandon> = None;
    // The tile still being walked when a `break 'walk` fired mid-body — its own bounds, where its
    // rows begin in `ticks`, and the cursor most recently used for it — so its paid-for rows can
    // extend `covered` afterward rather than being reclaimed by the clip in `serve_ticks` (F2).
    // `None` whenever every stop happened BETWEEN tiles (or the walk was cancelled outright, which
    // returns before this is ever read).
    let mut interrupted: Option<(i64, i64, usize, Option<rest::TradeCursor>)> = None;

    'walk: for (index, &(slice_from, slice_to)) in plan.slices.iter().enumerate() {
        let is_focus = index < plan.focus_len;
        let is_trade = index < plan.trade_len;
        // The tick budget never truncates a focus slice — checked only around a NON-focus one, so
        // a slice is whole or absent rather than cut mid-body. See the after-check below for the
        // other half of this rule.
        if !is_focus && ticks.len() >= tick_budget {
            complete = false;
            stop_reason = Some(TickAbandon::OverTickBudget);
            break;
        }
        let start_len = ticks.len();
        let mut cursor: Option<rest::TradeCursor> = None;
        loop {
            if cancelled() {
                // The window is gone; nothing collected so far is worth keeping.
                return TickVerdict::Abandoned(TickAbandon::Cancelled);
            }
            if expired(is_trade) {
                complete = false;
                stop_reason = Some(TickAbandon::Deadline);
                interrupted = Some((slice_from, slice_to, start_len, cursor));
                break 'walk;
            }
            let page_limit = if is_trade {
                page_budget.max(TRADE_PAGE_BUDGET)
            } else {
                page_budget
            };
            if pages_fetched >= page_limit {
                complete = false;
                stop_reason = Some(TickAbandon::OverPageBudget);
                interrupted = Some((slice_from, slice_to, start_len, cursor));
                break 'walk;
            }
            // A page that already came back over the IP's weight share stopped the host.
            // The first page was claimed before the loop; every later one has to look again,
            // or the walk keeps sending into a minute the header just closed.
            if pages_fetched > 0 && observer.host_closed(route.host()) {
                complete = false;
                stop_reason = Some(TickAbandon::RateLimited);
                interrupted = Some((slice_from, slice_to, start_len, cursor));
                break 'walk;
            }
            observer.pace(route.host());
            let page = match fetch(slice_from, slice_to, cursor) {
                Ok(page) => page,
                Err(rest::FetchError::UnknownSymbol) => {
                    complete = false;
                    venue_refused = true;
                    stop_reason = Some(TickAbandon::UnknownSymbol);
                    interrupted = Some((slice_from, slice_to, start_len, cursor));
                    break 'walk;
                }
                Err(rest::FetchError::Throttled { diagnostic, .. }) => {
                    // The fetch already recorded Retry-After or the weight stop. This arm
                    // must not look like a curve refusal: serve_ticks would replace that wait.
                    log::warn!(
                        "[x] trade-replay tick page limited on {} {}..{}: {diagnostic}",
                        route.host(),
                        slice_from,
                        slice_to
                    );
                    complete = false;
                    stop_reason = Some(TickAbandon::RateLimited);
                    interrupted = Some((slice_from, slice_to, start_len, cursor));
                    break 'walk;
                }
                Err(rest::FetchError::Transient(diagnostic)) => {
                    // The one place the venue's own words survive: the verdict carries only
                    // the class, and the gate backs the host off on it — a walk refused for a
                    // reason nobody can read is a backoff nobody can question.
                    log::warn!(
                        "[x] trade-replay tick page refused on {} {}..{}: {diagnostic}",
                        route.host(),
                        slice_from,
                        slice_to
                    );
                    complete = false;
                    venue_refused = true;
                    stop_reason = Some(TickAbandon::Transient);
                    interrupted = Some((slice_from, slice_to, start_len, cursor));
                    break 'walk;
                }
            };
            pages_fetched += 1;
            let mut rows = page.ticks;
            // D2-1: clip THIS page to the slice it was fetched for, before extending or counting
            // toward the budget — see this function's own doc comment for the vendor evidence.
            rows.retain(|t| {
                t.time_ms.is_finite()
                    && (t.time_ms as i64) >= slice_from
                    && (t.time_ms as i64) <= slice_to
            });
            ticks.extend(rows);
            // The focus can require many pages. Show its already-walked span before the tile
            // completes, but do not publish non-focus tiles that a budget may later discard.
            if is_focus && page.next.is_some() {
                if let Some(span) = walked_part(
                    &covered,
                    (slice_from, slice_to),
                    &ticks[start_len..],
                    page.next,
                ) {
                    let mut so_far = covered.clone();
                    so_far.add(span);
                    observer.progress(&ticks, &so_far);
                }
            }
            match page.next {
                Some(next_cursor) => cursor = Some(next_cursor),
                None => break,
            }
        }
        // The slice's own pagination completed. For a non-focus slice only, a tick budget crossed
        // during it removes the WHOLE slice rather than leaving it half-drawn.
        if !is_focus && ticks.len() > tick_budget {
            ticks.truncate(start_len);
            complete = false;
            stop_reason = Some(TickAbandon::OverTickBudget);
            break;
        }
        covered.add((slice_from, slice_to));
        observer.progress(&ticks, &covered);
    }

    if ticks.is_empty() {
        return TickVerdict::Abandoned(stop_reason.unwrap_or(TickAbandon::Empty));
    }
    // Add the interrupted tile's own paid-for stretch — the part of it its pagination direction
    // proves walked, see `walked_part` — so those rows are served rather than reclaimed by the
    // clip in `serve_ticks`. It coalesces with the completed stretch it abuts, or stands alone.
    if let Some((slice_from, slice_to, start, cursor)) = interrupted {
        if let Some(span) = walked_part(&covered, (slice_from, slice_to), &ticks[start..], cursor) {
            covered.add(span);
        }
    }
    if let Some(
        reason
        @ (TickAbandon::Deadline | TickAbandon::OverPageBudget | TickAbandon::OverTickBudget),
    ) = stop_reason
    {
        log::info!(
            "[x] trade-replay tick stage partial on {}: {reason:?}, covered={covered} ms, pages={pages_fetched}",
            route.host()
        );
    }
    TickVerdict::Ready(TickHarvest {
        ticks,
        covered,
        complete,
        venue_refused,
        stop: stop_reason,
    })
}

/// The stretch of a partly walked tile its rows prove exhaustive, or `None` when nothing can be
/// said.
///
/// Within one slice a paginated run is contiguous, but its direction is per-venue: Binance's
/// `FromId` cursor walks FORWARD from the tile's own `slice_from`, so the rows so far are every
/// print from that edge to the last one seen; Bitget/OKX's `LessThanId` and Gate futures'
/// `Before`/`Within` walk BACKWARD from `slice_to`, so they are every print from the first one
/// seen to that edge. Gate spot's `Page` cursor carries an UNDOCUMENTED order (`venue_caps.rs`), so its
/// rows prove nothing while a completed stretch exists to keep honest — and only when NO slice
/// completed at all does the observed extent of the rows stand in, which can only UNDER-state
/// true coverage, never claim more than was walked (D2-2). `AfterMs` is treated as undocumented:
/// no current route emits it, so there is no evidence for which edge it walks from.
///
/// Args:
///     covered: The stretches completed so far.
///     slice: The interrupted tile's own bounds.
///     rows: The rows fetched for it so far.
///     cursor: The cursor most recently used for it.
///
/// Returns:
///     The proven stretch, to be added to `covered` — it coalesces with the stretch it abuts
///     or stands alone, so no unwalked ground is ever claimed either way.
pub(super) fn walked_part(
    covered: &Coverage,
    slice: (i64, i64),
    rows: &[Tick],
    cursor: Option<rest::TradeCursor>,
) -> Option<(i64, i64)> {
    let first = rows.first()?;
    let (lo, hi) = rows.iter().fold(
        (first.time_ms as i64, first.time_ms as i64),
        |(lo, hi), t| (lo.min(t.time_ms as i64), hi.max(t.time_ms as i64)),
    );
    match cursor {
        Some(rest::TradeCursor::FromId(_)) => Some((slice.0, hi)),
        Some(
            rest::TradeCursor::LessThanId(_)
            | rest::TradeCursor::Before { .. }
            | rest::TradeCursor::Within { .. },
        ) => Some((lo, slice.1)),
        _ if covered.is_empty() => Some((lo, hi)),
        _ => None,
    }
}
