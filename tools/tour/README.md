# The user tour generator

Builds [`docs/tour/index.html`](../../docs/tour/index.html) — the interactive tour
published at <https://moonbot-tech.github.io/MoonTerminal/> — from data that already
lives in this repository.

```bash
pip install -r tools/requirements.txt   # PyYAML, once

make tour          # regenerate the page
make tour-check    # fail if the committed page is stale (this is the CI gate)
make tour-test     # the generator's own tests
make tour-theme    # refresh the palette from a MoonUI checkout (its own commit)
```

**The generated page is committed.** CI verifies it is current; it never writes it.

## What is generated and what is not

Be precise about this, because the tool will otherwise be blamed for something it
was never able to do.

| | |
|---|---|
| **Regenerated** | every visible string, the light and dark palettes, the interface metrics, the font stacks, the header figures |
| **Hand-maintained** | `template.html` — roughly 200 lines of window-replica markup, 350 lines of CSS, and all behavioural JavaScript |

So: **a locale or palette change is a re-run; a layout change is a template edit
and then a re-run.** If the toolbar gains a button, somebody edits the template.

## Where the data comes from

- **`locales/*.yml`** — the same files the terminal compiles in through `rust_i18n`.
  A content slot spelled `{locale: toolbar.live_tip}` pulls the application's own
  string, in every language it ships. That text is never retyped here, so it cannot
  drift from what a user actually sees in the terminal.
- **`theme.snapshot.toml`** — a committed copy of MoonUI's `moon-terminal.toml`,
  carrying the upstream revision it was taken from.
- **`content/*.yml`** — the prose the tour itself owns: zone explanations, the
  quick-start steps, panel and window descriptions, hotkey actions.

## Content slots — two shapes, and only two

```yaml
title:                              # authored: every configured language, required
  ru: "Live / Пауза"
  en: "Live / Pause"

body: {locale: toolbar.live_tip}    # the app's own tooltip, all languages at once
```

Anything else is an error. A one-key `locale` mapping can never be confused with a
language triple, so there is no convention to remember.

**Markup is opt-in.** Content reaches the DOM through `innerHTML`, so every slot is
HTML-escaped unless it says `html: true` — which today only the quick-start bodies
do, because they carry `<code>` and a link on purpose.

## Adding a language

Append it to `content/languages.yml`, then run `make tour`. The generator reports
every slot still missing that language, by file and by name; that report is the
worklist and finishing it is the completion criterion. No code changes.

## The MoonUI gap, stated honestly

MoonUI is a **cargo git dependency**. There is no MoonUI checkout in CI and none in
a fresh clone, so:

- generation always reads the **committed snapshot**, never a sibling checkout,
  even when one exists. Otherwise a developer who happens to have MoonUI at a
  different revision would commit a page CI could not reproduce, and the resulting
  full-palette diff would have nothing to do with their change.
- `make tour-check` therefore proves the page is **self-consistent with the
  committed snapshot**. It cannot prove the snapshot still matches MoonUI's master.
- `make tour-theme` is the deliberate refresh, and belongs in its own commit — the
  same discipline this repository already applies to `make update-moonproto`.

Nothing in this repository can close that last gap. The real fix, if it ever
becomes worth it: MoonUI publishes `moon-terminal.toml` at a stable URL and a
nightly job opens an issue on drift. Written down here so the analysis is not
re-derived from scratch next time.

## Layout

| File | Responsibility |
|---|---|
| `__main__.py` | CLI, exit codes, the Python and PyYAML guards |
| `paths.py` | every path read or written, anchored on this file rather than the CWD |
| `errors.py` | the failure types, and the collector that reports them all at once |
| `locales.py` | `locales/*.yml` into one flat table; refuses a key defined twice |
| `theme.py` | the palette and metrics; snapshot-first resolution |
| `content.py` | the slot rules and every validation |
| `render.py` | fills the template's slots; the post-render checks |
| `emit.py` | escaping and determinism — the only place that can corrupt the page |
| `template.html` | the page itself, with `{{slots}}` where data goes |
| `tests/` | run with `make tour-test`; the fixtures are deliberately broken content |

Failures accumulate rather than stopping at the first one: fixing a language pass
one message per run would mean a hundred runs.
