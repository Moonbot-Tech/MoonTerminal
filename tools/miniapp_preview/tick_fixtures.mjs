// Synthetic, seeded payloads for the tick-chart endpoints (/api/trade/ticks, /api/order/ticks),
// shaped as the frozen contract: ticks are [t_ms, price, qty, side] with side 1 a buy.
// Every name and figure is invented (CORE_A, XALPHA); nothing comes from a real core.

/** Linear congruential generator: the same seed always draws the same sequence in [0, 1). */
function rng(seed) {
    let state = seed >>> 0;
    return () => {
        state = (state * 1664525 + 1013904223) >>> 0;
        return state / 4294967296;
    };
}

// Closed trade: XALPHA long, entry 12:38:14.420 local-agnostic UTC, exit 48.5 s later on the take.
const T0 = Date.UTC(2026, 9, 16, 9, 38, 14, 420);
const T1 = Date.UTC(2026, 9, 16, 9, 39, 2, 910);
const ENTRY = 0.07124;
const TAKE = 0.07181;
const STOP = 0.07038;
const WINDOW_MS = 300000;
const VOLUME_BUCKET_MS = 2000;

export const CHART_TRADE = {
    core: 1001,
    core_name: "CORE_A",
    exchange: "Binance Futures",
    rec_id: 990001,
    new_rec_id: 770001,
    coin: "XALPHA",
    side: "buy",
    profit: 2.4,
    profit_text: "+2.40$",
    profit_pct: 0.8,
    profit_pct_text: "+0.80%",
    closed_at: Math.floor(T1 / 1000),
    closed_text: "16.10.2026 12:39",
    closed_short_text: "12:39",
    entry_text: "0.071240",
    exit_text: "0.071810",
    qty_text: "4210",
    duration_secs: 48,
    strategy: "DROPS_01",
    manual: false,
    volume_text: "300.0$",
};

// A slide into the buy order, a dip under it, a climb into the take, then drift after the exit.
function tradePath(t) {
    if (t < T0 - 45000) return 0.07158 + Math.sin((t - T0) / 37000) * 0.00016;
    if (t < T0 - 6000) return 0.07158 - (t - (T0 - 45000)) / 39000 * 0.0003;
    if (t < T0) return 0.07128 - (t - (T0 - 6000)) / 6000 * 0.0001;
    if (t < T0 + 4000) return ENTRY - 0.00008 + (t - T0) / 4000 * 0.00005;
    if (t < T1) return ENTRY - 0.00003 + (t - T0 - 4000) / (T1 - T0 - 4000) * (TAKE - ENTRY + 0.00003);
    if (t < T1 + 40000) return TAKE - (t - T1) / 40000 * 0.00012;
    return TAKE - 0.00012 + Math.sin((t - T1) / 29000) * 0.00014;
}

function buildTradeTicks() {
    const r = rng(7);
    const out = [];
    let t = T0 - WINDOW_MS;
    while (t < T1 + WINDOW_MS) {
        t += 40 + r() * 420;
        let p = tradePath(t) + (r() - 0.5) * 0.00007;
        if (t >= T0 && t < T1 && p > TAKE - 0.000005) p = TAKE - 0.000005 - r() * 0.00002;
        if (Math.abs(t - T1) < 250) p = TAKE;
        const rising = t >= T0 && t < T1;
        out.push([Math.round(t), +p.toFixed(6), Math.round(200 + Math.pow(r(), 3) * 9000), r() < (rising ? 0.62 : 0.4) ? 1 : 0]);
    }
    return out;
}

// Raw side volume per bucket, as the server computes it before any thinning.
function volumeOf(ticks, width) {
    const map = new Map();
    for (const [t, , q, side] of ticks) {
        const start = Math.floor(t / width) * width;
        const cell = map.get(start) || [start, 0, 0];
        if (side === 1) cell[1] += q;
        else cell[2] += q;
        map.set(start, cell);
    }
    return [...map.values()].sort((a, b) => a[0] - b[0]);
}

const TRADE_TICKS = buildTradeTicks();

/** Body of a /api/trade/ticks reply in `state` (ok, loading, no_tape, archive_pending). */
export function tradeTicks(state) {
    const base = { state, open_ms: T0, close_ms: T1 };
    if (state === "loading") return { ...base, retry_in_ms: 1500 };
    if (state === "catching_up") return { ...base, retry_in_ms: 10000 };
    if (state === "archive_pending") return { ...base, retry_at_ms: Date.UTC(2026, 9, 17, 20, 0, 0) };
    const levels = [
        { kind: "buy", price: ENTRY },
        { kind: "take", price: TAKE },
        { kind: "stop", price: STOP },
    ];
    if (state === "no_tape") return { ...base, levels };
    return {
        ...base,
        from_ms: T0 - WINDOW_MS,
        to_ms: T1 + WINDOW_MS,
        entry_price: ENTRY,
        exit_price: TAKE,
        levels,
        ticks: TRADE_TICKS,
        thinned_ms: 0,
        source: "recorder",
        exchange: "Binance Futures",
        price_decimals: 6,
        volume_bucket_ms: VOLUME_BUCKET_MS,
        volume: volumeOf(TRADE_TICKS, VOLUME_BUCKET_MS),
        exit_reason: "Sell Price",
    };
}

export const TRADE_TICK_COUNT = TRADE_TICKS.length;

// Live position: XBETA long; the take sits above, the stop below.
const LIVE_ENTRY = 2.4185;
const LIVE_TAKE = 2.439;
const LIVE_STOP = 2.392;
const LIVE_SPAN_MS = 900000;
const MAX_LIVE_TICKS = 4000;

export const CHART_ORDER = {
    core: 1002,
    core_name: "CORE_B",
    exchange: "Bybit Futures",
    uid: "18446744073709551001",
    coin: "XBETA",
    market: "XBETAUSDT",
    side: "buy",
    entry_text: "2.4185",
    mark_text: "2.4210",
    pnl: 0.31,
    pnl_text: "+0.31$",
    pnl_sign: 1,
    pnl_usd: 0.31,
    emulator: false,
    change_pct: 0.1,
    change_text: "+0.10%",
    to_entry_pct: null,
    to_entry_text: null,
    panic_armed: false,
    volume_text: "300.0$",
};

function closedTrade(closeMs) {
    return {
        core: CHART_ORDER.core,
        core_name: CHART_ORDER.core_name,
        exchange: CHART_ORDER.exchange,
        rec_id: 990002,
        coin: CHART_ORDER.coin,
        side: "buy",
        profit: 2.55,
        profit_text: "+2.55$",
        profit_pct: 0.85,
        profit_pct_text: "+0.85%",
        closed_at: Math.floor(closeMs / 1000),
        closed_text: "16.10.2026 12:43",
        closed_short_text: "12:43",
        entry_text: "2.4185",
        exit_text: "2.4390",
        qty_text: "124",
        duration_secs: 130,
        strategy: "DROPS_01",
        manual: false,
        volume_text: "300.0$",
    };
}

/**
 * A stateful /api/order/ticks handler: the first call seeds 15 minutes of prints, every later
 * call adds `perPoll` new ones and answers only prints after `since_ms`.
 * `mode`: live | stale (the newest print 18 s old, nothing new) | no_tape | closing | closed |
 *   quiet (the feed is live but has not printed this market yet: no ticks).
 * `closeAfter`: after that many live calls, two closing replies, then closed with the trade.
 * `seedCount`: seed exactly this many prints instead of the random-walk cadence.
 * `delayFirst`: milliseconds the first reply is held back.
 */
export function liveFeed({ mode = "live", perPoll = 3, seedCount = null, closeAfter = null, delayFirst = 0 } = {}) {
    const r = rng(11);
    const ring = [];
    let price = 2.414;
    let calls = 0;
    let openedAt = 0;
    const step = () => {
        price += (r() - 0.5) * 0.0011 + (2.4215 - price) * 0.02;
        price = Math.min(LIVE_TAKE - 0.0025, Math.max(LIVE_STOP + 0.009, price));
        return +price.toFixed(4);
    };
    const print = (t, buyBias) => [Math.round(t), step(), Math.round(5 + Math.pow(r(), 3) * 900), r() < buyBias ? 1 : 0];
    function seed(now, until) {
        openedAt = now - 130000;
        if (seedCount != null) {
            for (let i = 0; i < seedCount; i++) ring.push(print(until - LIVE_SPAN_MS + 1000 + (i + 1) * ((LIVE_SPAN_MS - 2000) / seedCount), 0.52));
            return;
        }
        let t = now - LIVE_SPAN_MS;
        while (t < until) {
            t += 80 + r() * 700;
            if (t < until) ring.push(print(t, 0.52));
        }
    }
    return (raw) => {
        calls += 1;
        const now = Date.now();
        let body = {};
        try { body = JSON.parse(raw || "{}"); } catch { body = {}; }
        const stale = mode === "stale";
        const silent = mode === "no_tape" || mode === "quiet";
        if (!ring.length && !silent) seed(now, stale ? now - 18000 : now);
        else if (!stale && !silent) {
            const last = ring.length ? ring[ring.length - 1][0] : now - 1000;
            for (let i = 1; i <= perPoll; i++) ring.push(print(last + (now - last) * i / perPoll, 0.55));
        }
        const since = typeof body.since_ms === "number" ? body.since_ms : now - LIVE_SPAN_MS;
        let ticks = ring.filter((t) => t[0] > since);
        const truncated = ticks.length > MAX_LIVE_TICKS;
        if (truncated) ticks = ticks.slice(ticks.length - MAX_LIVE_TICKS);
        let state = mode === "closed" || mode === "closing" || mode === "no_tape" ? mode : (stale ? "stale" : "live");
        let trade;
        if (closeAfter != null && calls > closeAfter) state = calls > closeAfter + 2 ? "closed" : "closing";
        if (state === "closed") trade = closedTrade(now - 20000);
        const mark = ring.length ? ring[ring.length - 1][1] : LIVE_ENTRY;
        const pnl = (mark - LIVE_ENTRY) * 124;
        const reply = {
            state,
            now_ms: now,
            last_tick_ms: ring.length ? ring[ring.length - 1][0] : mode === "quiet" ? now - 1000 : null,
            entry_ms: openedAt || now - 130000,
            entry_price: LIVE_ENTRY,
            mark_price: mark,
            levels: [
                { kind: "take", price: LIVE_TAKE },
                { kind: "entry", price: LIVE_ENTRY },
                { kind: "stop", price: LIVE_STOP },
            ],
            ticks,
            truncated,
            pnl_text: `${pnl >= 0 ? "+" : "-"}${Math.abs(pnl).toFixed(2)}$`,
            pnl_sign: pnl > 0 ? 1 : pnl < 0 ? -1 : 0,
            change_text: `${pnl >= 0 ? "+" : "-"}${Math.abs((mark / LIVE_ENTRY - 1) * 100).toFixed(2)}%`,
            take_dist_text: `+${((LIVE_TAKE / mark - 1) * 100).toFixed(2)}%`,
            stop_dist_text: `-${((1 - LIVE_STOP / mark) * 100).toFixed(2)}%`,
            price_decimals: 4,
        };
        if (trade) reply.trade = trade;
        return { body: reply, delay: calls === 1 ? delayFirst : 0 };
    };
}
