# Code conventions

Short and to the point. Architecture lives in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md);
this file is only the rules that apply to every PR.

## Tests

Three kinds of test, three homes. The toolchain dictates this, not taste:

| kind | sees | lives in |
|---|---|---|
| unit | private items | the module's **sibling file** |
| integration | public API only | `crates/<crate>/tests/*.rs` |
| doc-test | public API | inline in the `///` example |

- **Do not write `#[cfg(test)] mod tests { ... }` inside a file that carries logic** — at any
  size. The logic file keeps only the declaration; the bodies move to the sibling file.
  Otherwise a 300-line module becomes a 1–2k-line file you cannot navigate.

  ```rust
  // src/config/backup.rs — logic only
  fn is_snapshot_name(name: &str) -> bool { /* ... */ }

  #[cfg(test)]
  mod tests;
  ```
  ```rust
  // src/config/backup/tests.rs — reaches private items through super
  use super::*;

  #[test]
  fn rejects_a_user_folder() { assert!(!is_snapshot_name("2026-07-21")); }
  ```

- Where the sibling file lands: a declaration in `src/parser.rs` **or** in `src/parser/mod.rs`
  both resolve to `src/parser/tests.rs`.
- **Crate roots (`main.rs`, `lib.rs`, `src/bin/*.rs`) carry no unit tests** — `mod tests;` there
  resolves to a shared `src/tests.rs`, one file compiled into two crates. Keep roots thin and put
  logic in modules.
- **`moon-ui-gpui` is a binary crate** — it has no `[lib]`. An integration test cannot import
  anything from it and can only execute the built binary. That is why `tests/theme_contract.rs`
  checks its invariants by grepping the sources: a workaround for that limitation, not a style
  choice.
- Test files **are committed** — both `src/**/tests.rs` and `crates/*/tests/*.rs`.

### What makes a test worth keeping

- A test must **fail on a real regression**. If you cannot name the edit that would break it,
  it does not belong.
- Do not test the obvious. A three-arm `match` asserted to return its three arms catches nothing
  and costs a read on every review.
- The oracle must be **independent of the code under test**. A constant compared against its own
  literal, a field read back after the test set it, `> 0` on a tuned threshold — none of those
  are checks.

## Comments and strings

- **Comments and docstrings are in English.** Every one you write or rewrite: `//!`, `///`, `//`.
  Existing Russian comments are left alone — translate one only if you are already rewriting
  that block.
- **UI strings go through `t!("key")`**, with the keys in `locales/*.yml`. No literals in panels.
- A key is added in **all three languages at once** — ru/en/es. [`locales/README.md`](locales/README.md)
  is binding, not background: it holds the deliberately-untranslated list and the rule that glyphs
  never live in dictionary values.
- Every new or changed function, struct and module gets a docstring — public and private alike.
  For anything non-obvious, say **why**, not just what.

## Paths and data

- Every filesystem path lives in `moon_core::config::paths`. No `data/...` literals scattered
  through the code.
- Never commit `servers.enc`, `cfg/`, `data/` or `backups/` — they hold keys and local state.

## UI

- **MoonUI first.** Do not hand-roll a widget the stack already has; extend MoonUI upstream and
  consume it through `moon_ui::*`.
- Popups and menus go through `window.open_moon_context_menu(...)`, never as a child of a panel:
  z-order and dismissal belong to MoonUI's `Root`.
- Panels ship as a dock tab, a detached window and a group-window host, at widths from a narrow
  side dock to full screen. Give each panel a defined narrow behaviour; a horizontal scrollbar is
  not one.

## Commits and PRs

- Commit format: `<type>(<scope>): <subject>` — `feat`, `fix`, `refactor`, `docs`, `test`,
  `chore`. Example: `fix(report): flatten embedded line breaks in table cells`.
- **Anything that is not prose goes through a PR.** Direct-to-`main` is limited to `README.md`,
  `README.en.md` and `docs/**` (except `docs/FIRETEST.md` — a test asserts on its text).
  Everything else can change the binary, the build, the tests or CI.
- `main` has no branch protection and no required checks: a direct push is public and untested
  the instant it lands, with CI reporting only afterwards. Branch from fresh `main`, open a PR,
  squash-merge — history stays linear.
- **CI runs neither `fmt` nor `clippy`.** Run `make fmt` yourself before pushing.
- The CI gate is the Windows `.exe` job (~15 min). The macOS job is diagnostic
  (`continue-on-error`) — read its log, but it does not block.
- Never force-push or reset a shared `main` — fix forward with a new commit or a revert.

## Build and checks

```
make build | run | release | check | fmt
```

- Windows needs **VS 2022 Build Tools** (`vcvars64`). Resolve it with
  `vswhere -latest -find '**\vcvars64.bat'` — machines here carry both BuildTools and Community.
- **Never wrap a build in `2>&1`.** `vcvars64.bat` writes to stderr on a healthy run, and
  PowerShell 5.1 turns that into a terminating error — it reads as "the code does not compile"
  when nothing is wrong. Judge by the exit code.
- Tests: `cargo test -p moon-core` / `-p moon-ui-gpui`. A single test:
  `cargo test -p <crate> <name> -- --exact --nocapture`.
- Live behaviour check is FireTest: `moonterminal --debug-script chart-smoke`
  (see [`docs/FIRETEST.md`](docs/FIRETEST.md)).
- Unit tests inside a `moon-ui-gpui` panel module need **explicit imports**, never `use super::*`:
  the parent re-exports `gpui::*`, whose own `test` shadows the built-in attribute and makes
  `#[test]` expand recursively ("recursion limit reached").

## Files

LF, UTF-8, 4-space indent, trailing newline (`.gitattributes` + `.editorconfig`).
A CRLF write shows up as a whole-file diff.
