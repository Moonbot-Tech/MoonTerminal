//! Empty-field suggestions and popup sizing.

use super::*;

/// Nominal width of the search popup, in font-scaled units.
///
/// Shared with the header ticker, which hand-positions its own box BEHIND this popup and cannot
/// measure it: the two are drawn by different code and only line up because they start from the
/// same figure.
pub(crate) const COIN_POPUP_W: f32 = 300.0;

/// Number of top-mover rows offered while the field is empty.
///
/// The movers are the section worth scrolling: they are the only place the dropdown ANSWERS a
/// question rather than repeating a choice the user already made.
pub(crate) const COIN_SUGGEST_LIMIT: usize = 12;

/// Number of recently opened markets offered above them.
///
/// Deliberately short. Recents are a shortcut back to what is already in hand, and a long tail of
/// them pushes the movers — the part the user cannot reconstruct from memory — off the screen.
pub(crate) const COIN_RECENT_LIMIT: usize = 3;

/// How long a built suggestion list stays usable before it is rebuilt on the next popup open.
///
/// Movement rankings drift slowly and the scan is expensive, so half a minute of staleness in a
/// list of SUGGESTIONS costs nothing — the user is picking a market, not reading a quote.
const COIN_SUGGEST_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// One cached suggestion list, with everything needed to decide whether it is still valid.
pub(crate) struct CoinSuggestEntry {
    /// When the list was built.
    pub(crate) at: std::time::Instant,
    /// The market universe it was built from; a change invalidates it regardless of age.
    pub(crate) sig: CoinUniverseSig,
    /// Bare `(core, market)` pairs. Deliberately NOT resolved rows: labels and server names are
    /// live state, so they are rebuilt at read time rather than frozen into the cache.
    pub(crate) markets: Vec<(CoreId, String)>,
}

impl CoinSuggestEntry {
    /// Whether this entry is young enough to serve. The cheap half of the freshness test, for the
    /// render path.
    pub(crate) fn is_recent(&self) -> bool {
        self.at.elapsed() < COIN_SUGGEST_TTL
    }

    /// Whether this entry may still be served for `sig` — age AND the universe it was built from.
    /// The full test, for the open path, which can afford to rebuild the signature.
    pub(crate) fn is_fresh(&self, sig: &CoinUniverseSig) -> bool {
        self.is_recent() && &self.sig == sig
    }
}

/// Identity of the market universe a suggestion list was built from.
///
/// A cached list outlives the state it was derived from — a core can disconnect, a provider can
/// change — so the cache is keyed by this rather than by a timestamp alone: a stale entry must be
/// discarded because the WORLD changed, not only because time passed.
pub(crate) type CoinUniverseSig = Vec<(CoreId, Option<CoreId>)>;

/// The `(core, provider)` pairs currently feeding this field, in canonical core order.
pub(crate) fn universe_sig(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
) -> CoinUniverseSig {
    let ms = b.session.market_source();
    cores_for(b, group, bucket)
        .into_iter()
        .map(|core| (core, ms.provider_of(core)))
        .collect()
}

/// Resolves both empty-field sections for one coin field in a single pass over its core scope.
///
/// The persisted recents list is application-wide, but a coin field is not: an Add tab scoped to
/// one core, or a detached window scoped to its bucket, must never offer a market belonging to a
/// core outside that scope — picking one would drop a foreign core's chart into a bucket meant to
/// hold only its own. The history is therefore intersected with the very same `cores_for` scope the
/// typed search and the volatility ranking use, and only THEN trimmed to `limit`: trimming first
/// would let out-of-scope entries eat the visible slots.
///
/// Both sections resolve here rather than in two calls because this runs on the popup's render
/// path, and `cores_for` sorts the group's cores through `CoreOrder` on every call.
///
/// Args:
///     b: Backend holding the session, configuration and persisted history.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///     volatile: Cached top-movers markets, already ranked; see `Backend::coin_suggest_markets`.
///     limit: Maximum number of recent rows to return.
///
/// Returns:
///     `(recent, volatile)` hits, recents newest first.
pub(crate) fn suggestions(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    volatile: Vec<(CoreId, String)>,
) -> (Vec<CoinHit>, Vec<CoinHit>) {
    let scope = cores_for(b, group, bucket);
    let recent: Vec<(CoreId, String)> = b
        .recent_coins()
        .into_iter()
        .filter(|(core, _)| scope.contains(core))
        .take(COIN_RECENT_LIMIT)
        .collect();
    (hits_for(b, recent), hits_for(b, volatile))
}

/// Suggests the markets moving most over the last 24 hours across this field's cores.
///
/// Reuses the screener's own data rather than opening a feed of its own: `ScreenerRow::d_24h` is
/// the unsigned 24-hour range magnitude the Screener table already ranks by, weighed by turnover
/// through [`mover_score`] so a thin market's outsized percentage does not head the list. Ties
/// break on movement, then turnover, then the market name, so equal movers keep a stable order
/// instead of reshuffling per call.
///
/// `ScreenerRow::vol_24h` is denominated in each market's OWN quote currency, and one exchange
/// lists several — a BTC-quoted pair's turnover is numerically four orders below a USDT-quoted
/// one's at equal money. It is therefore converted through `quote_usd_rate` before comparison.
/// Rates are cached by quote during the scan; an unavailable rate uses the reference turnover so
/// missing conversion data does not classify a market as dead.
///
/// Rows are fetched ONCE PER PROVIDER but materialized against the CORES that consume that
/// provider: a provider is a market-data dedup key, not necessarily a core a chart may be opened
/// on. Each core therefore offers its own row for a shared market, exactly as a typed search does.
///
/// This walks every market of every provider — it is expensive and must never run at render; see
/// the suggestion cache in `Backend`.
///
/// Args:
///     b: Backend holding the session and market source.
///     group: Window group whose cores feed this field.
///     bucket: Chart bucket narrowing those cores, or `None` for the whole group.
///     limit: Maximum number of rows to return.
///
/// Returns:
///     Hits ordered by turnover-weighted 24-hour movement, descending, each carrying its resolved
///     label.
pub(crate) fn suggest_volatile(
    b: &Backend,
    group: &str,
    bucket: Option<&ChartBucket>,
    limit: usize,
) -> Vec<CoinHit> {
    let cores = cores_for(b, group, bucket);
    if cores.is_empty() || limit == 0 {
        return Vec::new();
    }
    let ms = b.session.market_source();
    // One fetch per distinct provider, then fan the rows back out to the cores that read it.
    // A core with no resolved provider is skipped rather than queried as its own provider, the
    // way the screener does it — asking a non-provider core for a market list answers nothing.
    let mut consumers: Vec<(CoreId, Vec<CoreId>)> = Vec::new();
    for core in &cores {
        let Some(provider) = ms.provider_of(*core) else {
            continue;
        };
        match consumers.iter_mut().find(|(p, _)| *p == provider) {
            Some((_, cs)) => cs.push(*core),
            None => consumers.push((provider, vec![*core])),
        }
    }

    // Rank each provider's markets and keep only its visible head, carrying the index of the
    // consumer list that head belongs to. Fanning every market out to its consuming cores BEFORE
    // ranking would multiply every market of every provider by its core count — thousands of cloned
    // market names on a many-core config — to produce the same handful of rows.
    let mut heads: Vec<Mover> = Vec::new();
    for (slot, (provider, _members)) in consumers.iter().enumerate() {
        // The rate depends on the QUOTE, and one exchange lists only a handful of them, while this
        // loop walks every market it has. Resolving per market would take the market-source lock
        // and a snapshot hundreds of times to answer the same few questions.
        let mut rates: HashMap<String, Option<f64>> = HashMap::new();
        // The market half only: a mover is a property of the market, and the per-core rows
        // `screener_rows` builds would repeat every market once per core sharing the exchange.
        let mut ranked: Vec<Mover> = ms
            .screener_market_rows(*provider)
            .into_iter()
            .filter(|row| row.d_24h.is_finite() && row.d_24h > 0.0)
            .map(|row| {
                // No rate is UNKNOWN turnover, not zero turnover. Substituting 1.0 would value a
                // BTC-quoted market's turnover as though one BTC were one dollar and bury a market
                // that may be among the busiest on the board; scoring it at the reference figure
                // instead lets it compete on movement, which is all that is actually known.
                let quote = moon_core::symbol::resolve_quote(&row.market).to_string();
                let rate = *rates
                    .entry(quote)
                    .or_insert_with(|| ms.quote_usd_rate(*provider, &row.market));
                let turnover = turnover_usd(row.vol_24h, rate);
                Mover {
                    score: mover_score(row.d_24h, turnover.unwrap_or(MOVER_VOL_REF)),
                    movement: row.d_24h,
                    turnover,
                    market: row.market,
                    slot,
                }
            })
            .collect();
        // A provider with no convertible turnover is rescored so it still competes; see
        // `neutralize_blind_provider`. This runs before the head is cut because rescoring may
        // change which candidates belong in that head.
        neutralize_blind_provider(&mut ranked);
        heads.extend(merge_ranked_heads(ranked, limit));
    }
    // Merge those per-provider heads into the top movers of the WHOLE scope, rather than leaving
    // the first provider's head padded out with the next provider's.
    let heads = merge_ranked_heads(heads, limit);
    // ONE row per market, offered on the FIRST core that can open it. A mover is a property of the
    // market, not of the cores watching it, and on a config where forty cores share an exchange the
    // fan-out turned a top of eight into the same coin repeated down the whole list. `consumers`
    // was built by walking `cores_for`, which is already canonical order, so its head is the core
    // the user reads first everywhere else.
    let mut out: Vec<(CoreId, String)> = heads
        .into_iter()
        .filter_map(|mover| {
            let core = *consumers[mover.slot].1.first()?;
            Some((core, mover.market))
        })
        .collect();
    out.truncate(SUGGEST_ROW_CAP);
    hits_for(b, out)
}

/// Resolves `(core, market)` pairs into labelled hits, batching label lookups per core.
///
/// Shared by the volatility ranking and the recents list: both hold bare market keys and need the
/// same server name and [`MarketLabel`] a typed search resolves. Input order is preserved, because
/// for recents that order IS the information. A market whose core is no longer a live session is
/// dropped rather than rendered with an empty server name.
pub(crate) fn hits_for(
    b: &Backend,
    pairs: impl IntoIterator<Item = (CoreId, String)>,
) -> Vec<CoinHit> {
    let pairs: Vec<(CoreId, String)> = pairs.into_iter().collect();
    if pairs.is_empty() {
        return Vec::new();
    }
    let ms = b.session.market_source();
    let venues = b.session.core_venues();
    let mut out: Vec<Option<CoinHit>> = vec![None; pairs.len()];
    // Group the positions by core so each core resolves its labels under one lock and snapshot.
    let mut cores: Vec<CoreId> = Vec::new();
    for (core, _) in &pairs {
        if !cores.contains(core) {
            cores.push(*core);
        }
    }
    for core in cores {
        let Some(server) = b
            .session
            .sessions()
            .iter()
            .find(|s| s.id == core)
            .map(|s| s.name.clone())
        else {
            continue;
        };
        // Cloned once per CORE rather than per hit: one core contributes many rows to a query, and
        // the value is three small fields.
        let venue = venues.get(&core).cloned();
        let positions: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, (c, _))| *c == core)
            .map(|(ix, _)| ix)
            .collect();
        let refs: Vec<&str> = positions.iter().map(|ix| pairs[*ix].1.as_str()).collect();
        let labels = ms.market_labels(core, &refs);
        for (ix, label) in positions.into_iter().zip(labels) {
            out[ix] = Some(CoinHit {
                core,
                market: pairs[ix].1.clone(),
                server: server.clone(),
                label,
                venue: venue.clone(),
                in_trade: false,
            });
        }
    }
    let mut hits: Vec<CoinHit> = out.into_iter().flatten().collect();
    in_trade::mark_live(b, &mut hits);
    hits
}
