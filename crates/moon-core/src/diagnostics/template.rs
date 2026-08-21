//! The documented content of `diagnostics.toml`, as data.
//!
//! Every switch is described ONCE, in [`KEYS`], and that single description is used for two jobs:
//! [`render`] builds the whole file for a first launch, and [`super::upgrade`] inserts individual
//! keys into an existing file when a new release adds one. Were the file a hand-written string and
//! the upgrade a separate list, the two would drift the first time somebody added a switch to one
//! and not the other — and the symptom would be a key documented in a file nobody has.
//!
//! The file is never round-tripped through serde. `toml::to_string` emits values only, so
//! serializing [`super::config::DiagCfg`] would produce a file with every key and not one word of
//! explanation — and the explanation is the entire reason the file exists. `storage.toml` is
//! written that way and is duly comment-free.

/// One `[section]`, with the prose that introduces it.
pub(super) struct SectionDoc {
    pub name: &'static str,
    /// Comment block placed above the `[section]` line, without leading `#`.
    pub doc: &'static str,
}

/// One switch: where it lives, what it defaults to, and why anybody would touch it.
pub(super) struct KeyDoc {
    pub section: &'static str,
    pub key: &'static str,
    /// The default rendered as TOML (`false`, `""`, `5000`). Must equal the `Default` impl in
    /// [`super::config`]; a test compares the rendered template against those defaults.
    pub default: &'static str,
    /// Comment block placed above the key, without leading `#`.
    pub doc: &'static str,
}

/// Preamble above the first section.
const HEADER: &str = "\
MoonTerminal — diagnostics

Developer switches for tracing what the terminal is doing at runtime. EVERYTHING HERE IS OFF BY
DEFAULT and nothing in this file is needed for normal use: the terminal behaves identically
whether this file exists or not. It is written once, on first launch, so the switches are
discoverable without reading the source.

HOW TO APPLY A CHANGE
  Save the file. The running terminal notices within a few seconds and applies it — no restart.
  Turning something off is the same edit in reverse. While anything here is on, the log carries a
  warning line naming the active switches, so a forgotten switch announces itself.

WHAT IT COSTS
  Every switch buys detail with work: extra log lines, extra files on disk, and on the busiest
  channels a measurable amount of CPU. Two of them can produce tens of thousands of lines per day.
  Turn on what answers your question, not everything at once — a log that records everything
  records nothing you can find.

ENVIRONMENT VARIABLES WIN
  Every SWITCH has a matching MOON_* environment variable, named in its comment below. When the
  variable is set it overrides this file and the file cannot turn that switch off — the override is
  one-way, on only. That keeps a scripted run — FireTest, CI, a headless capture — reproducible
  regardless of what this file says. The [limits] at the bottom are numbers rather than switches and
  have no variables; they always come from here.

WHEN AN UPDATE ADDS A SWITCH
  A new release appends the new key, with its comment, to the end of its section. Your existing
  settings, your edits and your own comments are left exactly as they are — nothing is reordered,
  reformatted or removed. A key you deleted on purpose comes back, since \"absent\" and \"unwanted\"
  look the same in a file; set it to its default instead if you want it gone from view.

IF THIS FILE IS BROKEN
  A syntax error is reported once in the log and every switch falls back to its default (off).
  The broken file is left exactly as you wrote it — never repaired, never overwritten, and never
  upgraded, because a file we cannot parse is one we must not edit. Fix the error, or delete the
  file to get a fresh documented copy on the next launch.";

/// Sections in file order.
pub(super) const SECTIONS: &[SectionDoc] = &[
    SectionDoc {
        name: "log",
        doc: "\
── Log detail ──────────────────────────────────────────────────────────────────────────────────

These admit debug-level tracing from one area into the ordinary log — the log files under `logs/`
and the Log panel in the terminal. Each area is a set of modules, so turning one on does not raise
the level for anything else.

Both areas below were deliberately silenced in August 2026: together they were 94% of a full day's
log, which left the Log panel's in-memory buffer holding under ten minutes of history. The tracing
was kept, not deleted, precisely so it could be switched back on from here.",
    },
    SectionDoc {
        name: "channels",
        doc: "\
── Dedicated channels ──────────────────────────────────────────────────────────────────────────

These do not go into the ordinary log. Each writes its own file in the `logs/` folder beside the
application's own log files, so a channel can run for a long stretch without burying the log — and
so its file can be read on its own.

These files are NOT swept by the log retention setting, which ages files by the date in their name.
They grow until you delete them, so do not leave a channel on and forget it.",
    },
    SectionDoc {
        name: "limits",
        doc: "\
── Limits ──────────────────────────────────────────────────────────────────────────────────────

Numbers that shape how much is kept and how tightly repeated lines are collapsed. These apply
whether or not anything above is on.",
    },
];

/// Every switch, in file order. Adding one here is the whole of adding it to the file: a first
/// launch renders it and an existing installation has it appended on the next start.
pub(super) const KEYS: &[KeyDoc] = &[
    KeyDoc {
        section: "log",
        key: "balances",
        default: "false",
        doc: "\
Balance repair on the live feed: each refresh request and the snapshot that answers it.
Answers \"did the core ever answer our request for this coin's balance\". Repeated requests inside
`balance_repeat_window_sec` collapse into one line.
COST: high — on a busy account this was ~271000 lines a day, 84% of the entire log.
ENV: MOON_DIAG_BALANCES=1",
    },
    KeyDoc {
        section: "log",
        key: "kline_cache",
        default: "false",
        doc: "\
Candle cache writes: which market/kind/day chunk was merged into `klines.sqlite`, first time each.
Answers \"is the chart empty because nothing was cached, or because the cache was not read\".
COST: moderate — ~33000 lines a day, in bursts of about five thousand as markets are swept.
ENV: MOON_DIAG_KLINE_CACHE=1",
    },
    KeyDoc {
        section: "log",
        key: "market_sources",
        default: "false",
        doc: "\
Market data sources: which source served a request, the cache prefix it read, the one-time
native backfill for a coarse timeframe, and the ARBITRAGE read — the core's own watch mask, the
venue codes that actually reported, and the Hyperliquid deployer names.
Answers \"why is this chart showing old prices\", \"did anything come out of the cache at all\" and
\"which platform byte is the venue the reference terminal calls OkxF\".
COST: moderate — rises with cursor movement, since a hovering cursor re-reads sources.
ENV: MOON_DIAG_MARKET_SOURCES=1",
    },
    KeyDoc {
        section: "log",
        key: "filter",
        default: "\"\"",
        doc: "\
Escape hatch for an area with no switch of its own: a filter in `RUST_LOG` syntax, appended after
everything above. `moon_core::db=debug` raises one module; `moon_core=debug` raises a whole crate
and will be very loud. Empty means no extra filter.

This field is validated ON ITS OWN and ignored WHOLE, with the reason in the log, if it either
fails to parse or contains a `/`. Both would otherwise take everything down with them: the parser
abandons the entire filter on any error — the base and every area switch above — and leaves the
terminal at errors only. A slash is refused even though it parses, because in this syntax it
introduces a match on the message TEXT applying to the whole filter rather than to the directive it
follows, which would make every record in the application conditional on your text.

ENV: RUST_LOG replaces the BASE filter (the default `warn,…=info` set). The area switches above are
still appended after it, so a variable cannot turn an area off.",
    },
    KeyDoc {
        section: "channels",
        key: "render",
        default: "false",
        doc: "\
Render counters -> `logs/render_diag.log`, one line per second: how often each panel, chart layer
and window actually repainted, next to process CPU and the number of open windows and charts.
This is the only honest answer to \"where are the frames going\" — render frequency cannot be read
out of the source, and reasoning about it without these counters has been wrong every time.
COST: low. The counters are plain atomic increments and the line is written once a second.
ENV: MOON_RENDER_DIAG=1",
    },
    KeyDoc {
        section: "channels",
        key: "detect",
        default: "false",
        doc: "\
Detect pipeline -> `logs/detect_diag.log`: a detect traced from the core's feed through the store
to the chart tab that shows it, including the sound and the detached window.
Answers \"the core fired a detect and nothing appeared\".
COST: low, and proportional to how many detects actually arrive.
ENV: MOON_DETECT_DIAG=1",
    },
    KeyDoc {
        section: "channels",
        key: "orders",
        default: "\"\"",
        doc: "\
Order pipeline -> `logs/order_diag.log`: every order the core reports, with its status and whether
the market it belongs to is known yet. Answers \"the core says the order exists, so where is it in
the interface\" — the protocol client silently parks an order whose market it has not learned, and
this channel is the only place that becomes visible.

This one is a SELECTOR, not a flag, because a busy account churns hundreds of orders and the one
being watched would be buried:
  \"\"            off
  \"1\"           every order on every core
  \"BTC\"         every core, markets whose name contains BTC
  \"GateF/BTC\"   only markets containing BTC on cores whose name contains GateF
Both halves match as case-insensitive substrings, so \"BTC\" finds BTCUSDT.
COST: low when narrowed to one market; high at \"1\" on a busy account.
ENV: MOON_ORDER_DIAG=GateF/BTC",
    },
    KeyDoc {
        section: "channels",
        key: "assets",
        default: "false",
        doc: "\
Assets: the raw balance rows each core reports, before the dust filter hides any of them.
Answers \"this coin is in the exchange's interface but not in the Assets panel\" — it separates a
row that was filtered out from a row that never arrived.
COST: moderate — one block per core per rebuild of the panel.
ENV: MOON_ASSETS_DIAG=1",
    },
    KeyDoc {
        section: "channels",
        key: "markets",
        default: "false",
        doc: "\
Markets: which market is followed by which core, the order-book pull, and archive reach.
Answers \"why does this market have no data\" and \"which core is serving it\".
COST: moderate, throttled by `market_trace_min_interval_ms` below.
ENV: MOON_MARKET_DIAG=1",
    },
    KeyDoc {
        section: "limits",
        key: "log_ring_lines",
        default: "5000",
        doc: "\
Lines held in the in-memory ring the Log panel reads from. Older lines are dropped; this does not
affect the log FILES, which keep everything for `log_retention_days` in the main settings.

This is a BUFFER, not the panel's window. The panel itself shows at most 5000 rows while it follows
the tail (20000 while scrolling is paused), so raising this past those numbers does not put more on
screen. What it buys is lines not being LOST: with a noisy switch on, a burst can push more entries
between two panel reads than the ring holds, and whatever overflowed is gone before it is ever
shown. That is the reason to raise it.

Roughly 150 bytes per line, so 20000 lines is about 3 MB. Clamped to 100..=1000000.",
    },
    KeyDoc {
        section: "limits",
        key: "balance_repeat_window_sec",
        default: "5",
        doc: "\
Balance-repair requests for the same core inside this many seconds collapse into a single line.
Only meaningful while `balances` is on. Set to 0 to log every request, which is genuinely loud:
on a busy account the requests arrive several times a second.",
    },
    KeyDoc {
        section: "limits",
        key: "market_trace_min_interval_ms",
        default: "1000",
        doc: "\
Minimum gap between two market-trace lines about the same subject, in milliseconds. Only
meaningful while `markets` is on. The order-book pull runs five times a second, so without this
floor the channel would emit at that rate per market.",
    },
];

/// Prefix every line of `block` with `# `, leaving blank lines as a bare `#`.
pub(super) fn comment(block: &str) -> String {
    block
        .lines()
        .map(|l| {
            if l.is_empty() {
                "#".to_string()
            } else {
                format!("# {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The complete file, for a first launch.
pub(super) fn render() -> String {
    let mut out = comment(HEADER);
    out.push('\n');
    for section in SECTIONS {
        out.push_str("\n\n");
        out.push_str(&comment(section.doc));
        out.push_str(&format!("\n[{}]\n", section.name));
        for key in KEYS.iter().filter(|k| k.section == section.name) {
            out.push('\n');
            out.push_str(&comment(key.doc));
            out.push_str(&format!("\n{} = {}\n", key.key, key.default));
        }
    }
    out
}
