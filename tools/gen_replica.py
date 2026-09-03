"""Build a synthetic MoonTerminal report replica for Analytics timing measurements.

Usage: python gen_replica.py <data_dir> <cores> <rows> [span_days] [--analyze]

Writes the complete Windows data-directory layout `crates/moon-core/src/config/paths.rs`
resolves: `<data_dir>/data/reports.sqlite`, `strategies.sqlite`, `valuation.sqlite` and
`klines.sqlite`, plus a `<data_dir>/data/fixture.json` manifest. The supplied data directory
must be disposable: the generator recreates every one of those files.

`reports.sqlite`'s schema mirrors every column `crates/moon-core/src/db/report_read.rs`'s
`DISPLAY_COLUMNS` projects (see `crates/moon-core/src/db/rep.rs` for the real replica's own
create-table, which starts from three columns and grows the rest dynamically from the core's
`ReportSchema` — this generator's hand-written column set plays that same role for the fixture).
`strategies.sqlite` mirrors the full writer schema in `crates/moon-core/src/strat_db/write.rs`.
`valuation.sqlite` mirrors `crates/moon-core/src/db/valuation/mod.rs::open_store`'s DDL exactly.
`klines.sqlite` is seeded by a separate Rust helper (`crates/moon-core/examples/gen_klines.rs`)
through `KlineCache`'s own production write API — never re-implemented here, see `_build_klines`.

`--analyze` is opt-in and defaults to off: production never runs SQLite `ANALYZE` on this
database, so a fixture that always carried `sqlite_stat1` statistics the user's replica does not
have would let every measured query plan diverge from the one shipped.

After generation the fixture is checked against hard bounds (row/core/coin/group cardinality, the
`reports.sqlite` size band, and the special-case ratios below) and the process exits non-zero on a
miss instead of silently shipping an undersized or ratio-drifted replica.
"""
import json
import os
import random
import sqlite3
import subprocess
import sys

TUNER_COLS = ["lev", "dmark", "pricebug", "hvol", "hvolf", "dvol", "vd1m", "bvsvratio", "d24h", "d3h", "da1m", "d5s", "btc1hdelta", "exchange1hdelta", "btc24hdelta", "exchange24hdelta", "btc5mdelta", "dbtc1m", "d1h", "d15m", "d5m", "d1m", "pump1h", "dump1h"]

# Every DISPLAY_COLUMNS entry that is a real stored column rather than one of report_read.rs's
# four SYNTHETIC (computed) columns (profitpct, valuation_profit_usdt, valuation_rate,
# valuation_rate_source) — see report_read.rs:18-73 and :108-125.
DATE_COLS = ["closedate", "buydate", "sellsetdate"]
REAL_COLS = [
    "profitbtc", "spentbtc", "boughtq", "buyprice", "sellprice",
    "quantity", "gainedbtc", "takeprofitlag",
]
INT_COLS = [
    "isshort", "emulator", "deleted", "strategyid", "basecurrency",
    "id", "taskid", "source", "channel", "status", "last_update_at",
]
TEXT_COLS = ["coin", "sellreason", "channelname", "signaltype", "comment", "exorderid", "fname"]

STRATEGIES_PER_CORE = 120
COINS_TARGET = 400
_REAL_TICKERS = ["BTC", "ETH", "SOL", "XRP", "ADA", "DOGE", "AVAX", "LINK", "DOT", "MATIC", "LTC", "BCH", "ATOM", "UNI", "ETC", "FIL", "APT", "ARB", "OP", "NEAR"]
LEV_CHOICES = [1, 5, 10, 25]

# Special-case ratios (1b): expected fraction of rows falling into each mutually exclusive
# non-normal bucket, drawn from one categorical roll per row so the buckets never overlap.
LIQUIDATION_RATIO = 0.0005
FUNDING_RATIO = 0.003
NULLDATE_RATIO = 0.0005
EMULATOR_RATIO = 0.02
DELETED_RATIO = 0.005

LIQUIDATION_OWNER = "ExternalOwner"


def _coin_universe(n=COINS_TARGET):
    """Build a deterministic coin ticker universe wide enough for cardinality targets.

    Args:
        n: Total distinct coin identities to produce.

    Returns:
        The 20 original real tickers (unchanged, for anything keying off them) followed by
        synthetic fillers up to `n`.
    """
    coins = [f"{a}USDT" for a in _REAL_TICKERS]
    i = 0
    while len(coins) < n:
        coins.append(f"ALT{i:03d}USDT")
        i += 1
    return coins[:n]


def _core_name(core):
    """Format:
        core -> f"core-{core:03d}", the same label the real replica writes per core.
    """
    return f"core-{core:03d}"


def _btc_cores(cores):
    """Cores whose reports are BTC-quoted (`basecurrency = 0`); every other core is USDT (`1`).

    Args:
        cores: Total synthetic core count.

    Returns:
        Up to the first 3 core ids, verified against `db/quote.rs`'s `QuoteCurrency::btc()`
        ordinal (0) and `QuoteCurrency::usdt()` ordinal (1).
    """
    return {c for c in range(1, cores + 1) if c <= 3}


def _margin_cores(cores):
    """Cores that write posted MARGIN (`notional / lev`) into `spentbtc` instead of full notional.

    Args:
        cores: Total synthetic core count.

    Returns:
        Up to 2 core ids distinct from `_btc_cores`, matching `db/analytics/basis.rs`'s
        per-core margin-vs-notional probe.
    """
    return {c for c in range(1, cores + 1) if 4 <= c <= 5}


def _rate_usdt_for_minute(minute_utc):
    """Deterministic synthetic BTC/USDT spot rate for one closed minute.

    A pure function of `minute_utc` rather than an RNG draw, so the same minute always resolves
    to the same rate across cores and reruns without needing a shared random-state thread through
    the row loop.

    Args:
        minute_utc: UTC minute start, in Unix seconds.

    Returns:
        A plausible BTC/USDT rate that varies slowly with time.
    """
    return 20_000.0 + float((minute_utc // 60) % 50_000)


def _build_reports(conn, cores, rows, span_days, rng, btc_cores, margin_cores):
    """Populate `orders_rep` and collect everything the valuation pass and the shape gate need.

    Args:
        conn: Open connection to the recreated `reports.sqlite`.
        cores: Synthetic core count.
        rows: Synthetic row count.
        span_days: Days covered by generated close times.
        rng: Seeded RNG shared with every other builder in this run.
        btc_cores: Core ids whose rows are BTC-quoted.
        margin_cores: Core ids that write margin instead of notional into `spentbtc`.

    Returns:
        Stats dict consumed by `_build_valuation` and the shape gate: `cores_seen`, `coins_seen`,
        `groups_seen`, per-special-case row counts, and the BTC-quoted closed rows to value.
    """
    cols = ["core_uid INTEGER NOT NULL", "core_name TEXT NOT NULL", "newrecid INTEGER NOT NULL"]
    cols += [f"{c} INTEGER" for c in DATE_COLS]
    cols += [f"{c} REAL" for c in REAL_COLS]
    cols += [f"{c} INTEGER" for c in INT_COLS]
    cols += [f"{c} TEXT" for c in TEXT_COLS]
    cols += [f"{c} REAL" for c in TUNER_COLS]
    conn.execute("CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
    conn.execute("INSERT OR REPLACE INTO app_meta(key,value) VALUES('legacy_dropped','1')")
    conn.execute(f"CREATE TABLE orders_rep ({', '.join(cols)}, PRIMARY KEY (core_uid, newrecid))")
    names = ["core_uid", "core_name", "newrecid"] + DATE_COLS + REAL_COLS + INT_COLS + TEXT_COLS + TUNER_COLS
    placeholders = ",".join("?" * len(names))
    sql = f"INSERT INTO orders_rep ({','.join(names)}) VALUES ({placeholders})"

    coins = _coin_universe()
    end = 1_780_000_000
    start = end - span_days * 86_400
    # A real replica accumulates in close-date order, so its table pages carry that locality.
    # Reproduce it: without this the index range scan pays a random page fetch per row and the
    # measurement overstates every period read.
    closes = sorted(rng.randrange(start, end) for _ in range(rows))

    stats = {
        "cores_seen": set(),
        "coins_seen": set(),
        "groups_seen": set(),
        "lev_seen": set(),
        "n_emulator": 0,
        "n_deleted": 0,
        "n_funding": 0,
        "n_liquidation": 0,
        "n_nulldate": 0,
    }
    to_value = []  # (core_uid, newrecid, closedate, profitbtc, spentbtc, last_update_at)

    batch = []
    for i in range(rows):
        core = i % cores + 1
        stats["cores_seen"].add(core)
        raw_close = closes[i]

        kind_roll = rng.random()
        if kind_roll < LIQUIDATION_RATIO:
            kind = "liquidation"
        elif kind_roll < LIQUIDATION_RATIO + FUNDING_RATIO:
            kind = "funding"
        elif kind_roll < LIQUIDATION_RATIO + FUNDING_RATIO + NULLDATE_RATIO:
            kind = "nulldate"
        else:
            kind = "normal"

        if kind == "funding":
            buy = raw_close
            sellreason = "Funding"
            stats["n_funding"] += 1
        else:
            buy = raw_close - rng.randrange(60, 86_400)
            sellreason = "TakeProfit"
        closedate_val = None if kind == "nulldate" else raw_close
        if kind == "nulldate":
            stats["n_nulldate"] += 1
        sellsetdate_val = raw_close + rng.randrange(0, 300)

        profit = rng.gauss(0.4, 25.0)
        notional = rng.uniform(20.0, 900.0)
        lev = rng.choice(LEV_CHOICES)
        stats["lev_seen"].add(lev)
        spent = notional / lev if core in margin_cores else notional
        boughtq = rng.uniform(0.01, 100.0)
        buyprice = rng.uniform(0.5, 70000.0)
        sellprice = rng.uniform(0.5, 70000.0)
        quantity = boughtq
        gainedbtc = spent + profit
        takeprofitlag = rng.uniform(0.0, 300.0)

        isshort = rng.randrange(2)
        emulator = 1 if rng.random() < EMULATOR_RATIO else 0
        stats["n_emulator"] += emulator
        deleted = 1 if rng.random() < DELETED_RATIO else 0
        stats["n_deleted"] += deleted
        sid = (i // cores) % STRATEGIES_PER_CORE + 1
        channelname = "Core"
        signaltype = "AutoSignal"
        if kind == "liquidation":
            sid = 0
            channelname = "LIQUIDATION"
            signaltype = LIQUIDATION_OWNER
            stats["n_liquidation"] += 1
        stats["groups_seen"].add((core, sid))
        basecurrency = 0 if core in btc_cores else 1
        taskid = rng.randrange(1, 999_999)
        source_val = rng.randrange(0, 3)
        channel_val = rng.randrange(0, 3)
        status_val = rng.randrange(0, 4)
        last_update_at = raw_close * 1000 + rng.randrange(0, 1000)
        coin = coins[i % len(coins)]
        stats["coins_seen"].add(coin)
        # `exorderid`/`fname`/`comment` are sized to match the byte width real exchange order ids
        # and order filenames carry (see the size-band gate below): a real replica's rows are not
        # this narrow, and the row width is what the index range-scan measurement is timing.
        exorderid = f"{(i + 1) * 137 + core:018d}"
        fname = f"core{core:03d}_order_{i:08d}_synthetic_fixture_row_reference"
        comment = (
            f"synthetic-fixture-note row={i} core={core} strategy={sid} "
            f"closeref={raw_close} padding-{'x' * 96}"
        )

        tuner_vals = [
            float(lev) if col == "lev" else rng.uniform(-50.0, 50.0) for col in TUNER_COLS
        ]

        row = (
            [core, _core_name(core), i]
            + [closedate_val, buy, sellsetdate_val]
            + [profit, spent, boughtq, buyprice, sellprice, quantity, gainedbtc, takeprofitlag]
            + [
                isshort, emulator, deleted, sid, basecurrency,
                i, taskid, source_val, channel_val, status_val, last_update_at,
            ]
            + [coin, sellreason, channelname, signaltype, comment, exorderid, fname]
            + tuner_vals
        )
        batch.append(row)
        if basecurrency == 0 and closedate_val is not None:
            to_value.append((core, i, closedate_val, profit, spent, last_update_at))
        if len(batch) >= 20000:
            conn.executemany(sql, batch)
            batch = []
    if batch:
        conn.executemany(sql, batch)
    conn.commit()
    for name, index_cols in [
        ("idx_rep_closedate", "closedate"),
        ("idx_rep_core_close", "core_uid, closedate"),
        ("idx_rep_strat", "core_uid, strategyid, buydate"),
        ("idx_rep_strategy_close", "core_uid, strategyid, closedate"),
    ]:
        conn.execute(f"CREATE INDEX IF NOT EXISTS {name} ON orders_rep({index_cols})")
    conn.commit()

    stats["to_value"] = to_value
    return stats


def _build_strategies(strat_path, cores, rng):
    """Recreate `strategies.sqlite` with the full production schema (`strat_db/write.rs`).

    Args:
        strat_path: Destination `strategies.sqlite` path.
        cores: Synthetic core count.
        rng: Shared seeded RNG.

    Returns:
        None. Keeps the 7 deliberate per-core sid special cases the enrichment path depends on:
        1 live+enabled, 2 live+disabled, 3 deleted, 4 nameless, 5 no current version, 6 two
        versions, 7 no head row at all.
    """
    try:
        os.remove(strat_path)
    except OSError:
        pass
    s = sqlite3.connect(strat_path)
    s.execute(
        "CREATE TABLE strategies (\n"
        "    core_uid     INTEGER NOT NULL,\n"
        "    strategy_id  INTEGER NOT NULL,\n"
        "    core_name    TEXT NOT NULL DEFAULT '',\n"
        "    name         TEXT NOT NULL DEFAULT '',\n"
        "    kind         TEXT NOT NULL DEFAULT '',\n"
        "    kind_ordinal INTEGER NOT NULL DEFAULT 0,\n"
        "    folder_path  TEXT NOT NULL DEFAULT '',\n"
        "    is_short     INTEGER NOT NULL DEFAULT 0,\n"
        "    checked      INTEGER NOT NULL DEFAULT 0,\n"
        "    server_ver   INTEGER NOT NULL DEFAULT 0,\n"
        "    server_ms    INTEGER NOT NULL DEFAULT 0,\n"
        "    deleted      INTEGER NOT NULL DEFAULT 0,\n"
        "    content_hash INTEGER NOT NULL DEFAULT 0,\n"
        "    head_hash    INTEGER NOT NULL DEFAULT 0,\n"
        "    updated_ms   INTEGER NOT NULL DEFAULT 0,\n"
        "    PRIMARY KEY (core_uid, strategy_id))"
    )
    s.execute("CREATE INDEX IF NOT EXISTS idx_strat_name ON strategies(core_uid, name)")
    s.execute("CREATE INDEX IF NOT EXISTS idx_strat_sid ON strategies(strategy_id)")
    s.execute(
        "CREATE TABLE strategy_versions (\n"
        "    id           INTEGER PRIMARY KEY,\n"
        "    core_uid     INTEGER NOT NULL,\n"
        "    strategy_id  INTEGER NOT NULL,\n"
        "    valid_from   INTEGER NOT NULL,\n"
        "    valid_to     INTEGER,\n"
        "    change_kind  TEXT NOT NULL,\n"
        "    origin       TEXT,\n"
        "    n_changed    INTEGER NOT NULL DEFAULT 0,\n"
        "    ver_gap      INTEGER NOT NULL DEFAULT 0,\n"
        "    server_ver   INTEGER NOT NULL DEFAULT 0,\n"
        "    server_ms    INTEGER NOT NULL DEFAULT 0,\n"
        "    checked_at   INTEGER NOT NULL DEFAULT 0,\n"
        "    raw_json     TEXT NOT NULL,\n"
        "    changed_json TEXT,\n"
        "    UNIQUE (core_uid, strategy_id, valid_from))"
    )
    s.execute("CREATE TABLE app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")

    heads = []
    versions = []
    base_valid_from = 1_700_000_000_000
    for core in range(1, cores + 1):
        core_name = _core_name(core)
        for sid in range(1, STRATEGIES_PER_CORE + 1):
            if sid == 7:
                continue  # deliberate: traded, but the strategy database never learned of it
            # Deliberate variety exercised by the enrichment path (kept from the earlier fixture):
            # sid 1 live+enabled, 2 live+disabled, 3 deleted, 4 nameless (NOT NULL DEFAULT '' rules
            # out a literal SQL NULL here; see the generator report for the resulting fallback
            # caveat), 5 no current version, 6 two versions, the rest ordinary.
            name = "" if sid == 4 else f"Strat_{core}_{sid}"
            deleted = 1 if sid == 3 else 0
            checked = 0 if sid == 2 else 1
            kind = "MoonShot" if sid % 2 else "Standard"
            heads.append(
                (core, sid, core_name, name, kind, 0, "", 0, checked,
                 sid, 0, deleted, 0, 0, 0)
            )
            if sid == 5:
                continue
            valid_from = base_valid_from + core * 1_000_000 + sid * 1_000
            raw = (
                f'{{"SignalType":"MoonShot","LastEditDate":"2026-01-0{sid % 9 + 1}",'
                f'"CoinsBlackList":"BTC,ETH,btc_rp","CoinsWhiteList":"SOL"}}'
            )
            if sid == 6:
                second_valid_from = valid_from + 500
                versions.append(
                    (core, sid, valid_from, second_valid_from, "created", None, 0, 0,
                     1, 0, 0, raw, None)
                )
                second_raw = ('{"SignalType":"Second","LastEditDate":"2025-12-31",'
                               '"CoinsBlackList":"XRP","CoinsWhiteList":""}')
                changed = '{"CoinsBlackList":{"old":"BTC,ETH,btc_rp","new":"XRP"}}'
                versions.append(
                    (core, sid, second_valid_from, None, "params", "local", 1, 0,
                     2, 0, 0, second_raw, changed)
                )
            else:
                versions.append(
                    (core, sid, valid_from, None, "created", None, 0, 0, 1, 0, 0, raw, None)
                )
    s.executemany(
        "INSERT INTO strategies (core_uid, strategy_id, core_name, name, kind, kind_ordinal,"
        " folder_path, is_short, checked, server_ver, server_ms, deleted, content_hash,"
        " head_hash, updated_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        heads,
    )
    s.executemany(
        "INSERT INTO strategy_versions (core_uid, strategy_id, valid_from, valid_to,"
        " change_kind, origin, n_changed, ver_gap, server_ver, server_ms, checked_at,"
        " raw_json, changed_json) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        versions,
    )
    s.commit()
    s.close()


def _build_valuation(valuation_path, to_value):
    """Recreate `valuation.sqlite` with the exact reader-facing DDL from `valuation/mod.rs::open_store`.

    Args:
        valuation_path: Destination `valuation.sqlite` path.
        to_value: `(core_uid, newrecid, closedate, profitbtc, spentbtc, last_update_at)` rows for
            every closed BTC-quoted report row, collected while `_build_reports` ran.

    Returns:
        None. Writes one `trade_values` row per input row, keyed `(0, core_uid, newrecid)`
        (`TradeSource::Typed.code() == 0`), plus the `rates` row each one resolves against.
    """
    for suffix in ("", "-wal", "-shm"):
        try:
            os.remove(valuation_path + suffix)
        except OSError:
            pass
    v = sqlite3.connect(valuation_path)
    v.execute("PRAGMA journal_mode=WAL")
    v.executescript(
        "CREATE TABLE IF NOT EXISTS rates (\n"
        "     algorithm_version INTEGER NOT NULL,\n"
        "     quote_ordinal INTEGER NOT NULL,\n"
        "     minute_utc INTEGER NOT NULL,\n"
        "     resolved_minute_utc INTEGER NOT NULL,\n"
        "     rate_usdt REAL NOT NULL,\n"
        "     price_basis INTEGER NOT NULL,\n"
        "     provider TEXT NOT NULL,\n"
        "     symbol TEXT NOT NULL,\n"
        "     orientation INTEGER NOT NULL,\n"
        "     candle_open_ms INTEGER NOT NULL,\n"
        "     candle_close_ms INTEGER NOT NULL,\n"
        "     leg1_rate REAL NOT NULL,\n"
        "     leg2_provider TEXT,\n"
        "     leg2_symbol TEXT,\n"
        "     leg2_orientation INTEGER,\n"
        "     leg2_rate REAL,\n"
        "     fetched_at_ms INTEGER NOT NULL,\n"
        "     PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)\n"
        " );\n"
        " CREATE TABLE IF NOT EXISTS rate_searches (\n"
        "     algorithm_version INTEGER NOT NULL,\n"
        "     quote_ordinal INTEGER NOT NULL,\n"
        "     minute_utc INTEGER NOT NULL,\n"
        "     searched_through_minute INTEGER NOT NULL,\n"
        "     next_retry_at_ms INTEGER NOT NULL,\n"
        "     attempts INTEGER NOT NULL,\n"
        "     updated_at_ms INTEGER NOT NULL,\n"
        "     PRIMARY KEY (algorithm_version, quote_ordinal, minute_utc)\n"
        " );\n"
        " CREATE INDEX IF NOT EXISTS idx_rate_searches_retry\n"
        "     ON rate_searches (algorithm_version, next_retry_at_ms);\n"
        " CREATE TABLE IF NOT EXISTS trade_values (\n"
        "     source_kind INTEGER NOT NULL,\n"
        "     core_uid INTEGER NOT NULL,\n"
        "     row_id INTEGER NOT NULL,\n"
        "     algorithm_version INTEGER NOT NULL,\n"
        "     closedate INTEGER NOT NULL,\n"
        "     quote_ordinal INTEGER NOT NULL,\n"
        "     profit_quote REAL NOT NULL,\n"
        "     spent_quote REAL,\n"
        "     rate_minute_utc INTEGER NOT NULL,\n"
        "     rate_usdt REAL NOT NULL,\n"
        "     profit_usdt REAL NOT NULL,\n"
        "     spent_usdt REAL,\n"
        "     valued_at_ms INTEGER NOT NULL,\n"
        "     PRIMARY KEY (source_kind, core_uid, row_id)\n"
        " );\n"
        " CREATE INDEX IF NOT EXISTS idx_trade_values_inputs\n"
        "     ON trade_values (algorithm_version, quote_ordinal, rate_minute_utc);"
    )

    ALGORITHM_VERSION = 2
    QUOTE_ORDINAL_BTC = 0
    SOURCE_KIND_TYPED = 0

    rate_rows = []
    trade_rows = []
    for core_uid, newrecid, closedate, profit, spent, last_update_at in to_value:
        minute_utc = (closedate // 60) * 60
        rate_usdt = _rate_usdt_for_minute(minute_utc)
        candle_open_ms = minute_utc * 1000
        rate_rows.append((
            ALGORITHM_VERSION, QUOTE_ORDINAL_BTC, minute_utc, minute_utc, rate_usdt,
            0, "synthetic", "BTCUSDT", 0, candle_open_ms, candle_open_ms + 59_999,
            rate_usdt, None, None, None, None, last_update_at,
        ))
        profit_usdt = profit * rate_usdt
        spent_usdt = spent * rate_usdt
        trade_rows.append((
            SOURCE_KIND_TYPED, core_uid, newrecid, ALGORITHM_VERSION, closedate,
            QUOTE_ORDINAL_BTC, profit, spent, minute_utc, rate_usdt, profit_usdt,
            spent_usdt, last_update_at,
        ))

    v.executemany(
        "INSERT OR IGNORE INTO rates (algorithm_version, quote_ordinal, minute_utc,"
        " resolved_minute_utc, rate_usdt, price_basis, provider, symbol, orientation,"
        " candle_open_ms, candle_close_ms, leg1_rate, leg2_provider, leg2_symbol,"
        " leg2_orientation, leg2_rate, fetched_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        rate_rows,
    )
    v.executemany(
        "INSERT INTO trade_values (source_kind, core_uid, row_id, algorithm_version,"
        " closedate, quote_ordinal, profit_quote, spent_quote, rate_minute_utc, rate_usdt,"
        " profit_usdt, spent_usdt, valued_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
        trade_rows,
    )
    v.commit()
    v.close()


def _build_klines(data_dir, span_days):
    """Seed `data/klines.sqlite` through `KlineCache`'s own production write API.

    Never re-implemented in Python: `chunks_v2`'s packed row codec (`pack_rows_v2`) is private to
    `market/kline_cache.rs`, and a second Python encoder of that binary format would be a second
    authority no drift check could prove equivalent to the first (rejected in review). Instead
    this shells out to a small Rust example that opens the cache and calls
    `KlineCache::merge_batch_blocking` directly.

    Args:
        data_dir: Root whose `data` child receives the recreated kline cache.
        span_days: Days the synthetic candle series should cover.

    Returns:
        None. Raises `subprocess.CalledProcessError` if the helper build or run fails.
    """
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    cmd = [
        "cargo", "run", "--release", "-p", "moon-core", "--example", "gen_klines",
        "--", os.path.abspath(data_dir), str(span_days),
    ]
    subprocess.run(cmd, cwd=repo_root, check=True)


def _check_shape(rows, cores, span_days, analyze, reports_bytes, stats):
    """Hard shape gate (1g): fail loudly rather than shipping an undersized or ratio-drifted
    fixture that would still produce a ranking and a cut line.

    Bounds are derived from the CLI arguments actually supplied, not hardcoded to one invocation,
    so a small development run does not spuriously fail; every check below — cardinality,
    size-band and ratio — scales to `rows` and applies at every size, so a tiny sparse fixture
    cannot pass the gate by virtue of being tiny.

    Args:
        rows, cores, span_days, analyze: The generator's own resolved arguments.
        reports_bytes: Size of the produced `reports.sqlite` in bytes.
        stats: The dict `_build_reports` returned.

    Returns:
        None on success.

    Raises:
        SystemExit(1) with a description of the first bound the fixture missed.
    """
    failures = []

    if len(stats["cores_seen"]) != cores:
        failures.append(
            f"core cardinality: expected {cores}, got {len(stats['cores_seen'])}"
        )
    expected_coins = min(COINS_TARGET, rows)
    if len(stats["coins_seen"]) != expected_coins:
        failures.append(
            f"coin cardinality: expected {expected_coins}, got {len(stats['coins_seen'])}"
        )
    expected_groups = cores * STRATEGIES_PER_CORE
    # A liquidation row's forced strategyid=0 can add one extra (core, 0) group per core beyond
    # the ordinary 1..STRATEGIES_PER_CORE groups; both the plain and +cores counts are accepted.
    if not (expected_groups <= len(stats["groups_seen"]) <= expected_groups + cores):
        failures.append(
            f"strategy-group cardinality: expected ~{expected_groups}, "
            f"got {len(stats['groups_seen'])}"
        )
    if not set(stats["lev_seen"]) <= set(LEV_CHOICES):
        failures.append(f"lev values outside {LEV_CHOICES}: {stats['lev_seen']}")

    min_bytes_per_row, max_bytes_per_row = 650, 1000
    low, high = rows * min_bytes_per_row, rows * max_bytes_per_row
    if not (low <= reports_bytes <= high):
        failures.append(
            f"reports.sqlite size band: expected {low / 1e6:.0f}-{high / 1e6:.0f} MB "
            f"at {rows} rows, got {reports_bytes / 1e6:.1f} MB"
        )

    def ratio_ok(count, ratio, label):
        """Append a failure when one observed special-case count escapes its scaled band."""
        expected = rows * ratio
        # Wide multiplicative tolerance: this gate exists to catch a broken generator (an
        # off-by-a-lot bug), not to police RNG sampling noise at the target row count. The
        # scale-independent "+5" floor keeps a small fixture's near-zero expected count from
        # rejecting on ordinary RNG noise.
        if not (expected * 0.3 <= count <= expected * 3.0 + 5):
            failures.append(
                f"{label} ratio: expected ~{expected:.0f} rows ({ratio:.3%}), got {count}"
            )

    ratio_ok(stats["n_emulator"], EMULATOR_RATIO, "emulator")
    ratio_ok(stats["n_deleted"], DELETED_RATIO, "deleted")
    ratio_ok(stats["n_funding"], FUNDING_RATIO, "Funding")
    ratio_ok(stats["n_liquidation"], LIQUIDATION_RATIO, "LIQUIDATION")
    ratio_ok(stats["n_nulldate"], NULLDATE_RATIO, "NULL closedate")

    if failures:
        sys.exit("[FAIL] fixture shape gate:\n  " + "\n  ".join(failures))


def build(data_dir, cores, rows, span_days=400, analyze=False):
    """Create a disposable report, strategy, valuation and kline replica with deterministic data.

    Args:
        data_dir: Root whose `data` child receives recreated SQLite databases.
        cores: Number of synthetic core identities to generate.
        rows: Number of synthetic report rows to generate.
        span_days: Number of days covered by the generated close times.
        analyze: Whether to run SQLite `ANALYZE` on the finished `reports.sqlite` (opt-in; off by
            default, since production never runs it — see the module docstring).

    Returns:
        None. Writes the fixture databases and manifest below `data_dir`.

    Raises:
        subprocess.CalledProcessError: If the production-API kline helper fails.
        SystemExit: If the generated fixture violates a required shape bound.
    """
    db_dir = os.path.join(data_dir, "data")
    os.makedirs(db_dir, exist_ok=True)
    reports = os.path.join(db_dir, "reports.sqlite")
    for suffix in ("", "-wal", "-shm"):
        try:
            os.remove(reports + suffix)
        except OSError:
            pass

    seed = 20260820
    rng = random.Random(seed)
    conn = sqlite3.connect(reports)
    conn.execute("PRAGMA journal_mode=WAL")
    btc_cores = _btc_cores(cores)
    margin_cores = _margin_cores(cores)
    stats = _build_reports(conn, cores, rows, span_days, rng, btc_cores, margin_cores)
    if analyze:
        conn.execute("ANALYZE")
        conn.commit()
    conn.close()

    strat_path = os.path.join(db_dir, "strategies.sqlite")
    _build_strategies(strat_path, cores, rng)

    valuation_path = os.path.join(db_dir, "valuation.sqlite")
    _build_valuation(valuation_path, stats["to_value"])

    _build_klines(data_dir, span_days)

    reports_bytes = os.path.getsize(reports)
    manifest = {
        "rows": rows,
        "cores": cores,
        "strategy_groups": len(stats["groups_seen"]),
        "coins": len(stats["coins_seen"]),
        "span_days": span_days,
        "reports_bytes": reports_bytes,
        "analyzed": analyze,
        "seed": seed,
        "btc_cores": sorted(btc_cores),
        "margin_cores": sorted(margin_cores),
        "n_emulator": stats["n_emulator"],
        "n_deleted": stats["n_deleted"],
        "n_funding": stats["n_funding"],
        "n_liquidation": stats["n_liquidation"],
        "n_nulldate": stats["n_nulldate"],
    }
    with open(os.path.join(db_dir, "fixture.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)

    _check_shape(rows, cores, span_days, analyze, reports_bytes, stats)

    print(
        f"[OK] {reports}: {rows} rows, {cores} cores, "
        f"{reports_bytes / 1024 / 1024:.1f} MB, "
        f"{manifest['strategy_groups']} strategy groups, {manifest['coins']} coins"
    )


if __name__ == "__main__":
    argv = sys.argv[1:]
    analyze_flag = "--analyze" in argv
    argv = [a for a in argv if a != "--analyze"]
    if len(argv) < 3:
        sys.exit("usage: gen_replica.py <data_dir> <cores> <rows> [span_days] [--analyze]")
    build(
        argv[0], int(argv[1]), int(argv[2]),
        int(argv[3]) if len(argv) > 3 else 400,
        analyze=analyze_flag,
    )
