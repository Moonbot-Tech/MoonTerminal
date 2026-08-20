"""Build a synthetic MoonTerminal report replica for Analytics timing measurements.

Usage: python gen_replica.py <data_dir> <cores> <rows> [span_days]

Writes <data_dir>/data/reports.sqlite and <data_dir>/data/strategies.sqlite (the Windows
data-directory layout crates/moon-core/src/config/paths.rs resolves).
Schema mirrors crates/moon-core/src/db/rep.rs (orders_rep + REP_INDEXES) and the columns
crates/moon-core/src/db/analytics reads. The supplied data directory must be disposable: the
generator recreates its report and strategy database files.
"""
import os
import random
import sqlite3
import sys

TUNER_COLS = "lev,dmark,pricebug,hvol,hvolf,dvol,vd1m,bvsvratio,d24h,d3h,da1m,d5s,btc1hdelta,exchange1hdelta,btc24hdelta,exchange24hdelta,btc5mdelta,dbtc1m,d1h,d15m,d5m,d1m,pump1h,dump1h".split(",")

REAL_COLS = [
    "profitbtc", "spentbtc", "boughtq", "buyprice", "sellprice",
]
DATE_COLS = ["closedate", "buydate"]
INT_COLS = ["isshort", "emulator", "deleted", "strategyid", "basecurrency"]
TEXT_COLS = ["coin", "sellreason", "channelname", "signaltype", "comment"]

COINS = [f"{a}USDT" for a in
         "BTC ETH SOL XRP ADA DOGE AVAX LINK DOT MATIC LTC BCH ATOM UNI ETC FIL APT ARB OP NEAR".split()]


def build(data_dir, cores, rows, span_days=400):
    """Create a disposable report and strategy replica with deterministic synthetic data.

    Args:
        data_dir: Root whose `data` child receives recreated SQLite databases.
        cores: Number of synthetic core identities to generate.
        rows: Number of synthetic report rows to generate.
        span_days: Number of days covered by the generated close times.
    """
    db_dir = os.path.join(data_dir, "data")
    os.makedirs(db_dir, exist_ok=True)
    reports = os.path.join(db_dir, "reports.sqlite")
    for suffix in ("", "-wal", "-shm"):
        try:
            os.remove(reports + suffix)
        except OSError:
            pass
    conn = sqlite3.connect(reports)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("CREATE TABLE IF NOT EXISTS app_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
    conn.execute("INSERT OR REPLACE INTO app_meta(key,value) VALUES('legacy_dropped','1')")
    cols = ["core_uid INTEGER NOT NULL", "core_name TEXT NOT NULL", "newrecid INTEGER NOT NULL"]
    cols += [f"{c} INTEGER" for c in DATE_COLS]
    cols += [f"{c} REAL" for c in REAL_COLS]
    cols += [f"{c} INTEGER" for c in INT_COLS]
    cols += [f"{c} TEXT" for c in TEXT_COLS]
    cols += [f"{c} REAL" for c in TUNER_COLS]
    conn.execute(
        "CREATE TABLE orders_rep (%s, PRIMARY KEY (core_uid, newrecid))" % ", ".join(cols)
    )
    names = ["core_uid", "core_name", "newrecid"] + DATE_COLS + REAL_COLS + INT_COLS + TEXT_COLS + TUNER_COLS
    placeholders = ",".join("?" * len(names))
    sql = "INSERT INTO orders_rep (%s) VALUES (%s)" % (",".join(names), placeholders)

    rng = random.Random(20260820)
    end = 1_780_000_000
    start = end - span_days * 86_400
    # A real replica accumulates in close-date order, so its table pages carry that locality.
    # Reproduce it: without this the index range scan pays a random page fetch per row and the
    # measurement overstates every period read.
    closes = sorted(rng.randrange(start, end) for _ in range(rows))
    strategies_per_core = 12
    batch = []
    for i in range(rows):
        core = i % cores + 1
        close = closes[i]
        buy = close - rng.randrange(60, 86_400)
        profit = rng.gauss(0.4, 25.0)
        spent = rng.uniform(20.0, 900.0)
        sid = (i // cores) % strategies_per_core + 1
        row = [
            core, "core-%03d" % core, i,
            close, buy, profit, spent,
            rng.uniform(0.01, 100.0), rng.uniform(0.5, 70000.0), rng.uniform(0.5, 70000.0),
            rng.randrange(2), 0, 0, sid, 1,
            COINS[i % len(COINS)], "TakeProfit", "", "", "",
        ]
        row += [rng.uniform(-50.0, 50.0) for _ in TUNER_COLS]
        batch.append(row)
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
        conn.execute("CREATE INDEX IF NOT EXISTS %s ON orders_rep(%s)" % (name, index_cols))
    conn.commit()
    conn.execute("ANALYZE")
    conn.commit()
    conn.close()

    strat_path = os.path.join(db_dir, "strategies.sqlite")
    try:
        os.remove(strat_path)
    except OSError:
        pass
    s = sqlite3.connect(strat_path)
    s.execute(
        "CREATE TABLE strategies (core_uid INTEGER, strategy_id INTEGER, name TEXT,"
        " deleted INTEGER DEFAULT 0, checked INTEGER DEFAULT 1,"
        " PRIMARY KEY (core_uid, strategy_id))"
    )
    s.execute(
        "CREATE TABLE strategy_versions (core_uid INTEGER, strategy_id INTEGER,"
        " raw_json TEXT, valid_to INTEGER)"
    )
    # Deliberate variety, so an equivalence A/B over the enrichment path actually exercises it:
    # sid 1 live+enabled, 2 live+disabled, 3 deleted (status but no lists), 4 NULL name (falls
    # back to the bare id), 5 no current version at all, 6 TWO current versions, 7 no head row at
    # all (traded, but the strategy database does not know it), the rest ordinary.
    heads = []
    versions = []
    for core in range(1, cores + 1):
        for sid in range(1, strategies_per_core + 1):
            if sid == 7:
                continue
            name = None if sid == 4 else "Strat_%d_%d" % (core, sid)
            deleted = 1 if sid == 3 else 0
            checked = 0 if sid == 2 else 1
            heads.append((core, sid, name, deleted, checked))
            if sid == 5:
                continue
            raw = ('{"SignalType":"MoonShot","LastEditDate":"2026-01-0%d",'
                   '"CoinsBlackList":"BTC,ETH,btc_rp","CoinsWhiteList":"SOL"}' % (sid % 9 + 1))
            versions.append((core, sid, raw, None))
            if sid == 6:
                versions.append((core, sid,
                                 '{"SignalType":"Second","LastEditDate":"2025-12-31",'
                                 '"CoinsBlackList":"XRP","CoinsWhiteList":""}', None))
    s.executemany("INSERT INTO strategies VALUES (?,?,?,?,?)", heads)
    s.executemany("INSERT INTO strategy_versions VALUES (?,?,?,?)", versions)
    s.commit()
    s.close()
    size = os.path.getsize(reports) / 1024 / 1024
    print("[OK] %s: %d rows, %d cores, %.1f MB" % (reports, rows, cores, size))


if __name__ == "__main__":
    build(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]),
          int(sys.argv[4]) if len(sys.argv) > 4 else 400)
