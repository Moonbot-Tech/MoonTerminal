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
  anything from it and can only execute the built binary. That is why `tests/theme_contract/`
  checks its invariants by grepping the sources: a workaround for that limitation, not a style
  choice.
- **An integration test that outgrows one file becomes a directory, not several targets.** Cargo
  takes `tests/<name>/main.rs` as ONE test target called `<name>`, with its submodules beside it;
  loose `tests/*.rs` files would each become a target of their own and could not share helpers.
  `tests/theme_contract/` is the worked example — a thin `main.rs` of `mod` declarations, one
  module per subject, and shared helpers in `support.rs`.
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
- **CI runs neither `fmt` nor `clippy`.** Run `cargo clippy` yourself before pushing, and format
  **only the files you touched** — `rustfmt --config skip_children=true <files>`. The committed
  tree is not rustfmt-clean, so `make fmt` (`cargo fmt --all`) silently reformats files your
  change never touched and buries the real diff. Read the formatter's own diff too: with two
  editions in the workspace it also reorders `use` blocks it had no business touching — revert
  those hunks.
- Three CI gates, all on every PR and all meant to be green before you merge: the Windows
  `.exe` job (~15 min), `Tests (x86_64-msvc)` running `cargo test --workspace`, and
  `Dependency audit (cargo-deny)`. They run in parallel. The macOS job is diagnostic
  (`continue-on-error`) — read its log, but it does not block. "Gate" is a convention here, not
  enforcement: with no branch protection nothing stops a red merge except you reading the checks.
- Never force-push or reset a shared `main` — fix forward with a new commit or a revert.
- New releases use canonical three-component stable tags such as `v0.24.0` and `v0.24.1`.
  Historical two-component tags such as `v0.21` remain readable as patch zero, but are not valid
  inputs for a new release and block a semantically equivalent alias such as `v0.21.0`. Start a
  new minor line at `vMAJOR.MINOR.0`, then increment PATCH for fixes. Tag an already-green `main`
  commit and either push that exact tag or dispatch the workflow with that existing tag; never
  create a new `vMAJOR.MINOR` release. The release workflow checks out that exact tag with full
  tag history, requires its commit on `main`, and verifies GitHub's published
  SHA-256 digest for the exact
  `MoonTerminal.exe` asset while the release is a draft. Repository release immutability must be
  enabled: publication then locks the verified tag and assets. The `RELEASE_ADMIN_TOKEN` Actions
  secret must grant repository Administration read and Contents write for the immutable-release
  preflight and final publication. Bare numeric and prerelease tags are not release inputs.

## Dependencies and the lockfile

`Cargo.lock` is committed, and it is the freeze: a third-party version can change only in a
deliberate commit, so a compromised or merely surprising upstream release cannot enter a build
unnoticed.

- **Touch a `Cargo.toml` → commit the lock in the same commit.** Forget it and every COMPILING CI
  job fails at its first step (`cargo fetch --locked`) before anything builds. The audit job has no
  such step and stays green — it reads the lock as data.
- **MoonUI stays rolling and needs nothing from you.** CI refreshes its three crates on every run;
  locally that is `make update-moon-ui`. If a new MoonUI master needs different third-party
  versions, CI goes red on purpose — refresh and commit the lock, reviewing the non-MoonUI lines.
- **MoonProto moves only by hand**: `make update-moonproto`, as its own commit. Nothing automatic
  ever touches it.
- **`make update-all`** moves everything including the pinned forks. That defeats the freeze; use
  it deliberately and read the whole diff.
- **A lock conflict on a rebase — do not hand-merge it.** Take the other side whole and let cargo
  add back only what your branch needs:

  ```
  git checkout origin/main -- Cargo.lock
  cargo check          # minimal update: adds your new dependency, moves nothing else
  git add Cargo.lock
  ```

  Do not reach for `cargo generate-lockfile` here — it re-resolves the whole graph and can move
  MoonProto and unrelated crates inside what should be a trivial conflict fix. The repo also ships
  no merge driver for this file on purpose: a `merge=ours` driver silently drops a coworker's newly
  added dependency.

  **Never do this while a sibling MoonUI override is active.** With `.cargo/config.toml` patching
  MoonUI to a local checkout, any cargo command rewrites the lock with local `path` entries, and
  `git add` would stage exactly that — a lock that breaks every other machine and all of CI. Remove
  the override and restore the lock first. `lockfile_contract.rs` skips itself while an override is
  present, so it will not catch this locally; CI, which never has one, will.
- Conflicts are rarer than they sound: the lock changes only when a manifest changes or a human
  refreshes a Moonbot dep. CI's per-run MoonUI refresh is never committed.
- New git dependency? Its URL must be approved in TWO places, in the same commit: `allow-git` in
  `deny.toml`, and the allow-list in `crates/moon-core/tests/lockfile_contract.rs`. Both are
  deliberate — one fails the audit gate, the other fails the test gate, and approving a new source
  of code should cost a conscious edit. Miss either and the red check names the file.
- An advisory you have to live with for now goes in `deny.toml`'s `ignore` list with its RUSTSEC
  id, the date, and one sentence of reason. Nothing enforces that convention; it works only if you
  keep it, and an entry with no reason becomes permanent by accident.

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
  `cargo test -p <crate> <name> -- --exact --nocapture`. CI runs the whole workspace, so a
  crate you did not build locally still has to compile and pass.
- The release-script tests shell out to **`jq`** (the scripts parse GitHub's JSON with it). A local
  box without it prints `[skip]` and runs the rest; under `CI` the same tests fail hard instead,
  because every hosted runner ships `jq`. Install it if you touch `.github/scripts`.
- Every `.sh` in the repo must run on **bash 3.2 with a BSD userland** — that is what a macOS
  checkout executes them under, while CI only ever sees Ubuntu, so nothing else catches the
  difference. `ci_gate_contract.rs` denies the constructs that have actually bitten (`${v,,}`, an
  awk ternary, GNU `\|` alternation, bare `sha256sum`); it is a denylist, not a substitute for
  running them on a Mac.
- Live behaviour check is FireTest: `moonterminal --debug-script chart-smoke`
  (see [`docs/FIRETEST.md`](docs/FIRETEST.md)).
- Unit tests inside a `moon-ui-gpui` panel module need **explicit imports**, never `use super::*`:
  the parent re-exports `gpui::*`, whose own `test` shadows the built-in attribute and makes
  `#[test]` expand recursively ("recursion limit reached").

## Files

LF, UTF-8, 4-space indent, trailing newline (`.gitattributes` + `.editorconfig`).
A CRLF write shows up as a whole-file diff.
