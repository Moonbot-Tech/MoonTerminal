//! Closed-trade history geometry: one arrow per action plus a dashed connector per trade.
//!
//! # The encoding
//!
//! Three independent channels, one meaning each — the readability of a dense chart rests on not
//! spending two of them on the same fact:
//!
//! - **colour** is the SIDE of the trade: long green, short red;
//! - **shape** is the ACTION: an arrow UP is a buy, an arrow DOWN is a sell. A long therefore
//!   opens with an up arrow and closes with a down arrow, and a short does the opposite;
//! - **position** is entry versus exit, joined by the connector so the pair reads as one trade.
//!
//! Profit and loss is deliberately NOT drawn. Colour is already spent on the side, and a second
//! colour axis on a 4 px glyph is unreadable; the figure belongs in the hover card instead.
//!
//! # Why the geometry lives here
//!
//! `moon-ui-gpui` is a binary crate with no `[lib]`, so nothing there can be reached from a test —
//! its invariants can only be grepped as text. Everything with an oracle worth asserting therefore
//! lives in this crate, exactly as [`crate::news_marks`] already does, and `chartdx` keeps only the
//! adapter that filters by core, rebases the epoch and resolves theme colours.
//!
//! # Sizing
//!
//! Every constant below is in LOGICAL pixels and is multiplied by the device scale exactly once, at
//! the point of use. Sizes are deliberately NOT derived from the view: a marker that grew with zoom
//! is what made the previous design collapse into blobs.
//!
//! The arrow size and the connector thickness ARE user-settable, through the chart toolbar's
//! graphics popup, and arrive per call in [`TradeGeometryCtx`] — a per-tab setting (with a
//! `layout.toml` default) that is still independent of zoom. It is deliberately NOT the `orders.toml` one:
//! `marker_size`/`knot_size` there belong to order lines, and making one slider move both
//! surfaces is exactly what this layer refused to do.

use moon_core::config::ChartGraphicsCfg;

use crate::layers::{
    MARKER_SHAPE_ARROW_DOWN, MARKER_SHAPE_ARROW_UP, MarkerInstance, SEG_CLAMP_NONE,
    SEG_EXTEND_NONE, SEG_PATTERN_DASH, SegInstance, rgb_with_alpha as rgba,
};

/// Half height of an arrow, in logical px. The apex sits ON the trade's price and the body hangs
/// away from it, so this is also how far the glyph reaches clear of the candle.
///
/// Sized by eye against a live chart: the first pass at 4.0 read as too faint next to a candle
/// wick. It stays well under the 7.0 the superseded cross used, because that size is what made
/// dense fills merge into blobs.
pub const ARROW_HALF_H: f32 = 6.0;
/// Half width of an arrow, in logical px. Carried in `MarkerInstance::thickness`, which is the
/// convention the news gem and the warning badge already use for a non-square glyph. Kept at three
/// quarters of the height so the triangle reads as an arrowhead rather than a squat wedge.
pub const ARROW_HALF_W: f32 = 4.5;
/// Thickness of the entry-to-exit connector, in logical px.
pub const CONNECTOR_THICKNESS: f32 = 2.0;

/// Smallest accepted arrow-size multiplier.
///
/// Floored rather than left open because the three shaders clamp the drawn half-extents with
/// `max(pos.z, 1.0)` in PHYSICAL px while [`hit_trade_marks`] has no such clamp: below this the
/// glyph would stop shrinking and the region that responds to it would not. At 0.4 the smaller
/// half-extent is `ARROW_HALF_W * 0.4 = 1.8` logical px, which stays above one physical px for
/// every device scale the terminal runs at.
pub const ARROW_SCALE_MIN: f32 = 0.4;
/// Largest accepted arrow-size multiplier. Past this a cluster of ten swallows the pane.
pub const ARROW_SCALE_MAX: f32 = 3.0;
/// Smallest accepted connector thickness, in logical px.
pub const CONNECTOR_THICKNESS_MIN: f32 = 0.5;
/// Largest accepted connector thickness, in logical px.
pub const CONNECTOR_THICKNESS_MAX: f32 = 8.0;

/// Smallest accepted trade-marker size multiplier.
///
/// It is 0.2 because that is ALREADY the floor the drawing path applies — `marker_half_physical_px`
/// in `chartdx::view` multiplies by `cfg_marker_scale.max(0.2)`. Normalizing to anything higher would
/// silently rewrite a valid stored value that renders correctly today, which is the exact failure
/// this crate's one-clamp-one-authority rule exists to prevent.
pub const MARKER_SCALE_MIN: f32 = 0.2;
/// Largest accepted trade-marker size multiplier. Shares [`ARROW_SCALE_MAX`]'s ceiling: past it a
/// dense cluster of trades swallows the pane, and the two multipliers COMPOUND.
pub const MARKER_SCALE_MAX: f32 = 3.0;

/// Clamp a configured trade-marker size multiplier into the drawable range.
///
/// Distinct from [`clamp_arrow_scale`] on purpose: that one sizes the closed-trade HISTORY arrows,
/// this one the live trade crosses, and the two MULTIPLY rather than substitute.
///
/// Args:
///     scale: Configured multiplier; a non-finite value means an unusable config.
///
/// Returns:
///     The multiplier within `[MARKER_SCALE_MIN, MARKER_SCALE_MAX]`, or the shipped default when it
///     is not finite.
pub fn clamp_marker_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MARKER_SCALE_MIN, MARKER_SCALE_MAX)
    } else {
        ChartGraphicsCfg::default().marker_scale
    }
}

/// Clamp a configured opacity into `0..=1`.
///
/// One helper for both volume opacities, because an opacity has no range of its own to argue about.
/// The fallback is passed in rather than assumed: the per-trade bars and the per-candle band ship
/// with different defaults.
///
/// Args:
///     alpha: Configured opacity; a non-finite value means an unusable config.
///     fallback: The value to use when `alpha` is not finite.
///
/// Returns:
///     `alpha` within `0..=1`, or `fallback`.
pub fn clamp_volume_alpha(alpha: f32, fallback: f32) -> f32 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        fallback
    }
}

/// Clamp a configured arrow-size multiplier into the drawable range.
///
/// The ONE definition, called from BOTH the drawing path and the hit test: the setting comes from
/// a hand-editable `layout.toml`, and clamping it differently on the two paths is precisely how the
/// glyph and its hit area drift apart (see [`hit_trade_marks`]).
///
/// Args:
///     scale: Configured multiplier; a non-finite value means an unusable config.
///
/// Returns:
///     The multiplier within `[ARROW_SCALE_MIN, ARROW_SCALE_MAX]`, or `1.0` when it is not finite.
pub fn clamp_arrow_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(ARROW_SCALE_MIN, ARROW_SCALE_MAX)
    } else {
        1.0
    }
}

/// Bring a configured [`ChartGraphicsCfg`] into its drawable ranges.
///
/// The ONE normalizer, and it exists for a reason the two clamps alone do not cover: `layout.toml`
/// is hand-editable and TOML accepts `nan`, which the config's lenient reader passes through as a
/// perfectly valid `f32`. Every `PartialEq` comparison against a stored NaN then reports a CHANGE,
/// so a settings signature or an idempotent setter holding a raw value would mark the chart dirty
/// on every single frame, forever. Call this wherever the value is STORED or COMPARED; the drawing
/// and hit-test paths still clamp their own inputs, which is idempotent and therefore free.
///
/// The trade-mark and bottom-volume fields that moved here from `ChartTheme` are covered too, and
/// they raise the stakes: four of them feed the `VolumeStyleGpu` the renderer diffs every frame to
/// decide whether to rebake its CACHED base texture. One NaN there does not merely mark the chart
/// dirty — it rebakes that texture forever, at frame rate.
///
/// Args:
///     cfg: Settings as they were read from the configuration.
///
/// Returns:
///     The same settings with every numeric field finite and in range.
pub fn normalize_chart_graphics(cfg: ChartGraphicsCfg) -> ChartGraphicsCfg {
    let def = ChartGraphicsCfg::default();
    ChartGraphicsCfg {
        trade_arrow_scale: clamp_arrow_scale(cfg.trade_arrow_scale),
        connector_thickness_px: clamp_connector_thickness(cfg.connector_thickness_px),
        marker_scale: clamp_marker_scale(cfg.marker_scale),
        trade_volume_alpha: clamp_volume_alpha(cfg.trade_volume_alpha, def.trade_volume_alpha),
        // The bottom band's two clamps belong to `volume_bars`, which owns that band and its
        // range constants; this normalizer only gathers them.
        candle_volume_style: crate::volume_bars::clamp_volume_style(cfg.candle_volume_style),
        candle_volume_height: crate::volume_bars::clamp_band_fraction(cfg.candle_volume_height),
        candle_volume_alpha: clamp_volume_alpha(cfg.candle_volume_alpha, def.candle_volume_alpha),
        ..cfg
    }
}

/// Clamp a configured connector thickness into the drawable range.
///
/// Args:
///     thickness: Configured thickness in logical px; a non-finite value means an unusable config.
///
/// Returns:
///     The thickness within `[CONNECTOR_THICKNESS_MIN, CONNECTOR_THICKNESS_MAX]`, or
///     [`CONNECTOR_THICKNESS`] when it is not finite.
pub fn clamp_connector_thickness(thickness: f32) -> f32 {
    if thickness.is_finite() {
        thickness.clamp(CONNECTOR_THICKNESS_MIN, CONNECTOR_THICKNESS_MAX)
    } else {
        CONNECTOR_THICKNESS
    }
}
/// Opacity of an arrow. The markers are the data.
pub const MARKER_ALPHA: f32 = 0.95;
/// Opacity of the connector. It is context rather than data, so it stays below [`MARKER_ALPHA`];
/// the two are declared together so the ratio is visible in one read. Raised twice from an initial
/// 0.45 after live review — the dashes broke up against the candles long before the arrows did,
/// because a dash spends half its length showing the background rather than its own colour.
pub const CONNECTOR_ALPHA: f32 = 0.85;

/// How much a hovered arrow grows, so the cursor gets visible confirmation of what it is reading.
///
/// Applied on top of [`cluster_growth`], never folded into the hit area — see [`hit_trade_marks`].
pub const TRADE_HOVER_SCALE: f32 = 1.2;

/// Extra logical px around an arrow that still count as a hit.
///
/// Larger than the news gem's slack because the arrow is the smaller glyph of the two: a 4.5 px
/// half-width is below what a hand holds steady on a moving chart.
const HIT_SLACK: f32 = 4.0;

/// Growth factor an arrow gets from the number of trades it aggregates.
///
/// Capped and logarithmic: a ten-trade cluster reads as clearly bigger, a thousand-trade one stops
/// at the same ceiling rather than swallowing the pane. A cluster of one is exactly `1.0`, which is
/// what lets the clustered and unclustered cases share one code path.
///
/// **This expression is duplicated in all three shader copies** (`ARROW_CLUSTER_GROW` /
/// `ARROW_CLUSTER_GROW_CAP` in `order_lines.hlsl`, `native_marker.wgsl` and `chart_native.metal`),
/// because nothing in the repository compiles a shader and no value can be passed to one except
/// through an instance field. This copy is what the HIT AREA is measured with, so a shader edited
/// without it makes the arrows a different size than the region that responds to them.
///
/// Args:
///     count: Number of trades the arrow stands for; zero and one both mean a single glyph.
///
/// Returns:
///     Multiplier for both arrow half-extents.
pub fn cluster_growth(count: u32) -> f32 {
    1.0 + 0.25 * (count.max(1) as f32).log2().min(3.0)
}

/// One closed trade, projected into the terms this crate can reason about.
///
/// A UI-agnostic mirror of the report replica's record: this crate must not learn about `db`, and
/// the caller owns the filtering (by core, by coin) that decides which trades arrive here.
///
/// Note the field names are the replica's and are written from a LONG's point of view:
/// `buy_price`/`buy_ms` are the ENTRY and `sell_price`/`close_ms` the EXIT, whichever side the
/// trade took. For a short the entry is therefore a sell and the exit a buy — [`explode_actions`]
/// is the ONE place that reading is interpreted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeMark {
    /// Entry timestamp, absolute Unix milliseconds.
    pub buy_ms: i64,
    /// Exit timestamp, absolute Unix milliseconds.
    pub close_ms: i64,
    /// Entry price.
    pub buy_price: f64,
    /// Exit price.
    pub sell_price: f64,
    /// Traded quantity. Used as the clustering weight; a non-positive value is tolerated.
    pub qty: f64,
    /// Whether the trade was opened short.
    pub is_short: bool,
}

/// Pixel radius within which two actions of the SAME kind collapse into one cluster, in logical
/// px, applied on BOTH axes. An x-only threshold would merge an entry at 100 and one at 130 into a
/// single marker at 115, sitting on neither.
pub const CLUSTER_PX: f32 = 8.0;

/// One end of one trade: the atom clustering works on.
///
/// Trades are exploded into actions BEFORE clustering, so 40 scalps closing in one pixel column
/// collapse into a single down arrow while their entries stay a separate up-arrow cluster. Merging
/// whole trades instead would keep the picture just as dense at the ends, which is where the mess
/// actually is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeAction {
    pub t_ms: i64,
    pub price: f64,
    pub qty: f64,
    /// Whether this action BUYS. A long's entry and a short's exit both do.
    pub buy: bool,
    pub is_short: bool,
    /// Index into the `marks` slice this action came from.
    pub mark: usize,
}

/// A group of actions drawn as one arrow.
///
/// A cluster of one is the degenerate case of the same type, which is what makes "zoom in until it
/// dissolves" free: nothing switches modes, the threshold simply stops matching.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeCluster {
    /// Absolute milliseconds, volume-weighted across positive quantities or averaged otherwise.
    pub t_ms: f64,
    /// Price, volume-weighted across positive quantities or averaged otherwise.
    pub price: f64,
    pub buy: bool,
    pub is_short: bool,
    /// Indices into `marks`, in the order the members were swept (chronological).
    pub members: Vec<usize>,
}

/// Explode trades into their two actions each.
///
/// A long enters by buying and exits by selling; a short does the opposite. The replica's field
/// names are written from a long's point of view, so this is the ONE place that reading is
/// interpreted — see [`TradeMark`].
///
/// Args:
///     marks: Closed trades to split into entry and exit actions.
///
/// Returns:
///     Two actions per trade, in each trade's entry-then-exit order.
pub fn explode_actions(marks: &[TradeMark]) -> Vec<TradeAction> {
    let mut actions = Vec::with_capacity(marks.len().saturating_mul(2));
    for (idx, mark) in marks.iter().enumerate() {
        actions.push(TradeAction {
            t_ms: mark.buy_ms,
            price: mark.buy_price,
            qty: mark.qty,
            buy: !mark.is_short,
            is_short: mark.is_short,
            mark: idx,
        });
        actions.push(TradeAction {
            t_ms: mark.close_ms,
            price: mark.sell_price,
            qty: mark.qty,
            buy: mark.is_short,
            is_short: mark.is_short,
            mark: idx,
        });
    }
    actions
}

/// Collapse actions that land within [`CLUSTER_PX`] of each other into single markers.
///
/// Actions are PARTITIONED by `(buy, is_short)` before the sweep, never merely skipped during it: a
/// green up arrow and a red down arrow in the same pixel are opposite facts, and a single pass over
/// the mixed list would let one incompatible action in the middle split two neighbours that should
/// have merged.
///
/// Each cluster is anchored to its SEED rather than to its running mean, so its on-screen extent
/// stays bounded by `2 * CLUSTER_PX`. A mean-chasing sweep would let a chain of near-neighbours
/// grow a cluster without limit, and the marker would end up far from most of its own members.
///
/// Args:
///     actions: Actions to group, in any order.
///     px_per_ms: Horizontal scale, physical px per millisecond, from the pane's own view.
///     px_per_price: Vertical scale, physical px per price unit, from the same view.
///     scale: Device pixel scale, applied to [`CLUSTER_PX`].
///
/// Returns:
///     One cluster per drawn marker, ordered by kind and then chronologically.
pub fn cluster_actions(
    actions: &[TradeAction],
    px_per_ms: f32,
    px_per_price: f32,
    scale: f32,
) -> Vec<TradeCluster> {
    let reach = (CLUSTER_PX * scale.max(0.1)) as f64;
    // A degenerate view (a pane mid-layout, a collapsed price range) must not merge the whole
    // history into one marker: with no usable scale, nothing is close enough to anything.
    let (ppm, ppp) = (px_per_ms as f64, px_per_price as f64);
    let usable = ppm.is_finite() && ppp.is_finite() && ppm > 0.0 && ppp > 0.0;

    let mut out = Vec::new();
    for key in [(false, false), (true, false), (false, true), (true, true)] {
        let mut part = actions
            .iter()
            .filter(|a| (a.buy, a.is_short) == key)
            .collect::<Vec<_>>();
        if part.is_empty() {
            continue;
        }
        // A TOTAL order: `t_ms` alone is not unique, and an unstable tie would vary which member
        // seeds a cluster and therefore where the marker lands.
        part.sort_by(|a, b| {
            a.t_ms
                .cmp(&b.t_ms)
                .then(a.price.total_cmp(&b.price))
                .then(a.mark.cmp(&b.mark))
        });

        let mut group: Vec<&TradeAction> = Vec::new();
        for action in part {
            let joins = match group.first() {
                Some(seed) if usable => {
                    let dx = (action.t_ms - seed.t_ms) as f64 * ppm;
                    let dy = (action.price - seed.price) * ppp;
                    dx.abs() <= reach && dy.abs() <= reach
                }
                Some(_) => false,
                None => true,
            };
            if !joins {
                out.push(fold_cluster(&group, key));
                group.clear();
            }
            group.push(action);
        }
        if !group.is_empty() {
            out.push(fold_cluster(&group, key));
        }
    }
    out
}

/// The weight one action carries in its cluster's aggregate position.
///
/// The ONE definition, used for both the numerators and the denominator. They were once written
/// separately — the denominator summing raw quantities while the numerators clamped them — and a
/// group holding both a positive and a negative quantity then divided one weight set by another,
/// which can place the marker outside the range of its own members. The replica preserves any
/// finite quantity, negative ones included, so that group is reachable.
///
/// Args:
///     qty: Quantity reported for one action.
///
/// Returns:
///     The positive finite quantity, or zero when it cannot safely weight the aggregate.
fn cluster_weight(qty: f64) -> f64 {
    if qty.is_finite() && qty > 0.0 {
        qty
    } else {
        0.0
    }
}

/// Fold one swept group into its aggregate marker.
///
/// The aggregate is VOLUME-WEIGHTED, so the marker sits where the money actually went. A total
/// weight of zero falls back to the plain mean: the replica maps an absent or malformed quantity
/// to zero, and a zero denominator would put the marker at a non-finite coordinate — which the GPU
/// draws as nothing at all, silently losing the trades.
///
/// Args:
///     group: Non-empty swept group to aggregate.
///     key: Shared `(buy, is_short)` identity of every action in the group.
///
/// Returns:
///     One cluster containing the group's aggregate position and source-mark indices.
fn fold_cluster(group: &[&TradeAction], key: (bool, bool)) -> TradeCluster {
    // One pass, not three: the weight is the same value in all three sums, and computing it once
    // per action is also what keeps the numerators and the denominator provably in step — they
    // were once written as separate passes, and drifted (see [`cluster_weight`]).
    let mut total = 0.0_f64;
    let mut t_weighted = 0.0_f64;
    let mut price_weighted = 0.0_f64;
    for action in group {
        let weight = cluster_weight(action.qty);
        total += weight;
        t_weighted += action.t_ms as f64 * weight;
        price_weighted += action.price * weight;
    }
    let (t_ms, price) = if total > 0.0 {
        (t_weighted / total, price_weighted / total)
    } else {
        let n = group.len().max(1) as f64;
        (
            group.iter().map(|a| a.t_ms as f64).sum::<f64>() / n,
            group.iter().map(|a| a.price).sum::<f64>() / n,
        )
    };
    TradeCluster {
        t_ms,
        price,
        buy: key.0,
        is_short: key.1,
        members: group.iter().map(|a| a.mark).collect(),
    }
}

/// Quantize the view scale into the bucket the rebuild signature carries.
///
/// Clustering happens when the userdata layer is rebuilt, so a scale change MUST invalidate that
/// layer — and hashing the RAW scale would make every marker rebuild on every frame of a smooth
/// zoom. Eighth-octaves mean a cluster instead dissolves in discrete steps of roughly 9% zoom.
///
/// What this bucket does NOT buy, stated plainly so the next reader does not credit it with more:
/// the chart panel's render path calls `sync_orders_if_visible(.., force = true)` whenever the view
/// is dirty, and `force` short-circuits every per-surface signature before this one is compared, so
/// the WHEEL-ZOOM path rebuilds regardless — as it already did for orders, figures and news long
/// before trade history existed. This bucket governs the non-forced path, where it is what makes a
/// clustering-changing zoom rebuild AT ALL. Making the forced path selective would change shared
/// chart plumbing and belongs to its own task.
///
/// Args:
///     px_per_ms: Horizontal view scale in physical pixels per millisecond.
///     px_per_price: Vertical view scale in physical pixels per price unit.
///
/// Returns:
///     A stable signature component for the two quantized view scales.
pub fn scale_bucket(px_per_ms: f32, px_per_price: f32) -> u64 {
    let step = |v: f32| -> i64 {
        let v = v as f64;
        if !v.is_finite() || v <= 0.0 {
            i64::MIN
        } else {
            (v.log2() * 8.0).round() as i64
        }
    };
    (step(px_per_ms) as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(step(px_per_price) as u64)
}

/// Shortest on-screen connector worth drawing, in logical px.
///
/// Below this the entry and the exit are within a few pixels of each other, so the "line" is a
/// smudge under its own arrows and traces nothing the eye can follow — and those are exactly the
/// trades the clustering has already merged into one marker.
///
/// This replaced a cap on the trade COUNT. The count could not express the rule it was written
/// for: the loader keeps up to a thousand records regardless of the view, so a pane holding 401
/// historical trades lost every connector even when zoomed into two of them. Length is measured in
/// the view the user is actually looking at, so it thins exactly when the picture is dense and
/// restores every connector the moment they zoom in.
pub const CONNECTOR_MIN_PX: f32 = 6.0;

/// Everything the geometry needs from the pane besides the trades themselves.
///
/// Bundled rather than passed loose because the view scales, the device scale and the two side
/// colours all travel together to every call, and phase-by-phase this list only grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeGeometryCtx {
    /// Absolute chart epoch subtracted from every timestamp.
    pub epoch_ms: f64,
    /// Colour of a long trade.
    pub long_rgb: [u8; 3],
    /// Colour of a short trade.
    pub short_rgb: [u8; 3],
    /// Device pixel scale; every logical constant is multiplied by it here and nowhere else.
    pub scale: f32,
    /// Horizontal view scale, physical px per millisecond.
    pub px_per_ms: f32,
    /// Vertical view scale, physical px per price unit.
    pub px_per_price: f32,
    /// User multiplier on the arrow half-extents, clamped by [`clamp_arrow_scale`] at use.
    ///
    /// The hit test takes the SAME number through the same clamp; see [`hit_trade_marks`].
    pub arrow_scale: f32,
    /// User connector thickness in logical px, clamped by [`clamp_connector_thickness`] at use.
    pub connector_thickness: f32,
    /// The ACTION under the cursor as `(marks index, buy)`; its cluster grows and goes fully opaque.
    ///
    /// Deliberately not a cluster index. The caller takes this from the snapshot of a PREVIOUS
    /// build, and this call re-clusters at the view scale live right now — which is not the scale
    /// that snapshot was built at, because the rebuild signature quantizes scale into buckets and
    /// the view keeps moving inside one. A positional cluster index taken from the old clustering
    /// can therefore name a different arrow in the new one, growing a marker the hover card is not
    /// describing. An action survives re-clustering; only its grouping changes.
    ///
    /// The `buy` half is load-bearing, not decoration: a trade explodes into TWO actions that carry
    /// the SAME marks index — its entry and its exit — so a bare mark identifies a TRADE and would
    /// grow both of its arrows at once. Clusters never mix the two, so pairing the mark with the
    /// direction names exactly one cluster per build.
    pub hovered: Option<(usize, bool)>,
}

/// Append arrows and connectors for the visible trades, clustering dense ones.
///
/// Args:
///     marks: Trades to draw, already filtered to the pane's own core and coin.
///     ctx: The pane's epoch, colours and scales.
///     markers: Marker union to extend, shared with orders, figures, news and warnings.
///     segs: Segment union to extend, shared with order ladders and figures.
///
/// Returns:
///     The cluster snapshot the markers were built FROM, in the same order as the appended
///     markers. Hit-testing must read this rather than re-clustering from the live view: the
///     rebuild signature quantizes the scale (see [`scale_bucket`]), so a recomputation inside the
///     same bucket can split or merge a cluster the GPU is still displaying whole.
pub fn build_trade_geometry(
    marks: &[TradeMark],
    ctx: &TradeGeometryCtx,
    markers: &mut Vec<MarkerInstance>,
    segs: &mut Vec<SegInstance>,
) -> Vec<TradeCluster> {
    let TradeGeometryCtx {
        epoch_ms,
        long_rgb,
        short_rgb,
        scale,
        px_per_ms,
        px_per_price,
        arrow_scale,
        connector_thickness,
        hovered,
    } = *ctx;
    let clusters = cluster_actions(&explode_actions(marks), px_per_ms, px_per_price, scale);

    let arrow_scale = clamp_arrow_scale(arrow_scale);
    let half_h = ARROW_HALF_H * scale * arrow_scale;
    let half_w = ARROW_HALF_W * scale * arrow_scale;
    markers.reserve(clusters.len());
    for cluster in &clusters {
        let rgb = if cluster.is_short {
            short_rgb
        } else {
            long_rgb
        };
        // The hovered arrow grows and goes fully opaque. The growth is applied HERE and not through
        // the cluster count, so it stays out of `cluster_growth` and therefore out of the hit area.
        let hot = hovered
            .is_some_and(|(mark, buy)| cluster.buy == buy && cluster.members.contains(&mark));
        let grow = if hot { TRADE_HOVER_SCALE } else { 1.0 };
        let alpha = if hot { 1.0 } else { MARKER_ALPHA };
        // The arrow points the way the ACTION went: an up arrow is a buy whichever side opened it.
        let shape = if cluster.buy {
            MARKER_SHAPE_ARROW_UP
        } else {
            MARKER_SHAPE_ARROW_DOWN
        };
        markers.push(MarkerInstance::arrow(
            (cluster.t_ms - epoch_ms) as f32,
            cluster.price as f32,
            half_h * grow,
            half_w * grow,
            shape,
            cluster.members.len() as u32,
            rgba(rgb, alpha),
        ));
    }

    // Connectors stay PER TRADE and are never clustered: a cluster's two ends belong to different
    // trades, so a line between aggregates would join a synthetic pair that never traded together.
    {
        let thickness = clamp_connector_thickness(connector_thickness) * scale;
        let min_px = (CONNECTOR_MIN_PX * scale.max(0.1)) as f64;
        segs.reserve(marks.len());
        for mark in marks {
            // Skip a connector too short to read. Measured in the CURRENT view, so the same trade
            // gains its line back the moment the user zooms in on it.
            let dx = (mark.close_ms - mark.buy_ms) as f64 * px_per_ms as f64;
            let dy = (mark.sell_price - mark.buy_price) * px_per_price as f64;
            let len_px = dx.hypot(dy);
            if len_px.is_finite() && len_px < min_px {
                continue;
            }
            let rgb = if mark.is_short { short_rgb } else { long_rgb };
            // The connector must NOT extend: `SEG_EXTEND_EDGE` moves the far end in time while
            // keeping its price, which flattens any sloped segment — and a trade that moved is
            // exactly the case this line exists to show.
            segs.push(SegInstance {
                t0_rel: (mark.buy_ms as f64 - epoch_ms) as f32,
                p0: mark.buy_price as f32,
                t1_rel: (mark.close_ms as f64 - epoch_ms) as f32,
                p1: mark.sell_price as f32,
                thickness,
                pattern: SEG_PATTERN_DASH,
                extend: SEG_EXTEND_NONE,
                clamp: SEG_CLAMP_NONE,
                color: rgba(rgb, CONNECTOR_ALPHA),
            });
        }
    }

    clusters
}

/// One arrow as the hit test sees it: where it is drawn and how big it was drawn.
///
/// Deliberately NOT the cluster itself. The cluster carries time and price; turning those into
/// pixels needs the pane's mapping, which lives in the UI crate — so the caller does that
/// conversion and this crate stays free of any view type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TradeMarkAt {
    /// Arrow centre on the x axis, in the panel's device pixels.
    pub x: f32,
    /// The arrow's APEX on the y axis, in device pixels — the point sitting on the trade's price.
    /// The body hangs away from it, downward for a buy and upward for a sell.
    pub apex_y: f32,
    /// Whether this arrow points up, i.e. whether the action was a buy.
    pub buy: bool,
    /// How many trades the arrow stands for, which is what decides how big it was drawn.
    pub count: u32,
}

/// The clusters the cursor is over: the nearest one plus every other one it touches.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TradeHit {
    /// Index of the cluster nearest the cursor — the one that grows.
    pub nearest: usize,
    /// Every touched cluster, in the order the marks came in. A buy cluster and a sell cluster can
    /// overlap at the same spot, and hiding one of them would silently drop trades from the card.
    pub stack: Vec<usize>,
}

/// Hit-test the drawn arrows against a cursor position, all in device pixels.
///
/// The reach is measured against the arrow's RESTING size — its base size times [`cluster_growth`],
/// but never times [`TRADE_HOVER_SCALE`]. Both halves of that matter and for opposite reasons: the
/// cluster growth is a property of the data and is already on screen, so leaving it out would make
/// the biggest arrows the hardest to hit; the hover growth is a property of BEING hovered, so
/// folding it in would let a mark grow itself under a cursor that had only just reached its edge,
/// then shrink back out from under it — a marker flickering on its own growth.
///
/// Args:
///     marks: Drawn arrows, in cluster order; the returned indices point back into this sequence.
///     cursor: Cursor position in the same device pixels.
///     scale: Device pixel scale, applied to the logical size constants.
///     arrow_scale: The SAME user multiplier the arrows were drawn with; it goes through
///         [`clamp_arrow_scale`] here exactly as it did in [`build_trade_geometry`]. `HIT_SLACK`
///         is deliberately left out of it — the slack is how steadily a hand holds a cursor, not
///         a property of the glyph, so it must not shrink with a smaller arrow.
///
/// Returns:
///     The touched clusters, or `None` when the cursor is over none of them.
pub fn hit_trade_marks(
    marks: impl IntoIterator<Item = TradeMarkAt>,
    cursor: (f32, f32),
    scale: f32,
    arrow_scale: f32,
) -> Option<TradeHit> {
    let scale = scale.max(0.1);
    let slack = HIT_SLACK * scale;
    // Hoisted: this loop runs on the POINTER PATH, once per mark on every mouse move that clears
    // the movement threshold, and only `grow` varies between iterations.
    let arrow_scale = clamp_arrow_scale(arrow_scale);
    let half_w = ARROW_HALF_W * scale * arrow_scale;
    let body_full = 2.0 * ARROW_HALF_H * scale * arrow_scale;
    let mut stack: Vec<usize> = Vec::new();
    let mut nearest = 0usize;
    let mut best = f32::INFINITY;
    for (index, mark) in marks.into_iter().enumerate() {
        let grow = cluster_growth(mark.count);
        let reach_x = half_w * grow + slack;
        let dx = cursor.0 - mark.x;
        if dx.abs() > reach_x {
            continue;
        }
        // The body runs from the apex AWAY from the price: down for an up arrow, up for a down one.
        let body = body_full * grow;
        let (top, bottom) = if mark.buy {
            (mark.apex_y - slack, mark.apex_y + body + slack)
        } else {
            (mark.apex_y - body - slack, mark.apex_y + slack)
        };
        if cursor.1 < top || cursor.1 > bottom {
            continue;
        }
        // Rank by distance to the glyph's CENTRE rather than to its apex, so two arrows the cursor
        // sits between resolve to the one it is actually inside more of.
        let center_y = (top + bottom) * 0.5;
        let dy = cursor.1 - center_y;
        let dist = dx * dx + dy * dy;
        if dist < best {
            best = dist;
            nearest = index;
        }
        stack.push(index);
    }
    (!stack.is_empty()).then_some(TradeHit { nearest, stack })
}

#[cfg(test)]
mod tests;
