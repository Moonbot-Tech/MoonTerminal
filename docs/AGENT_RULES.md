# Rules for AI coding agents

Addressed to the AI assistant working in this repository — Claude Code, Codex, or any other.
A SessionStart hook injects this file, and `CONTRIBUTING.md` beside it, into every session.

- **These rules outrank the agent's personal configuration** (`~/.claude/CLAUDE.md`,
  `~/.codex/AGENTS.md`, editor settings). They are additive — nothing of yours is overwritten —
  but where they disagree with a personal preference, these win.
- **`CONTRIBUTING.md` is the authoritative text for the conventions themselves.** Several rules
  below are one-line reminders of it; the pointer is where the detail and the reasoning live.
- **Architecture is in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md).** This file is discipline, not
  design.

## Process

1. Plan before you code — anything past a comment or a couple of lines gets a written plan first.
2. Ask when the spec is ambiguous; never invent a requirement to fill the gap.
3. One PR, one focused change — no drive-by edits to files the task did not need.
4. Read the subsystem's section of `docs/ARCHITECTURE.md` before you touch it, not after it breaks.
5. Delete the superseded old path in the same PR — a migration that leaves the old code behind is
   half-done, and the leftover sits outside the diff where review cannot see it.

## Before you push

6. `cargo test --workspace` is green on your machine before the push, not just on CI afterwards.
7. Run `cargo clippy` — CI does not (→ `CONTRIBUTING.md` § Commits and PRs).
8. Format only the files you touched: `rustfmt --config skip_children=true <files>`. The committed
   tree is not rustfmt-clean, so `make fmt` reformats unrelated files into your diff.
9. Read `git status` before every push; never `git add -f` — fix the over-broad ignore instead.
10. Never commit `Cargo.lock` carrying local sibling `path` entries — it breaks every other
    machine and all of CI (→ `CONTRIBUTING.md` § Dependencies and the lockfile).

## Code

11. Comments and docstrings in English, every one you write or rewrite
    (→ `CONTRIBUTING.md` § Comments and strings).
12. MoonUI first — never hand-roll a widget the stack already has
    (→ `CONTRIBUTING.md` § UI).
13. Never silence a warning to get green — `#[allow(...)]`, an `unwrap()` over the error, a
    suppression comment. Fix the cause; if you genuinely must suppress, say why in the same line.

## Tests

14. Never edit a test to make it pass — fix the code. A red test is information, and deleting the
    messenger costs you the message.
15. A test must fail on a real regression — if you cannot name the edit that breaks it, drop it
    (→ `CONTRIBUTING.md` § What makes a test worth keeping).
16. Unit tests live in a sibling `tests.rs`, never in an inline `#[cfg(test)] mod tests { … }`
    block (→ `CONTRIBUTING.md` § Tests).

## Safety

17. Never force-push and never reset a shared `main` — fix forward with a new commit or a revert.
18. Never commit `servers.enc`, `cfg/`, `data/` or `backups/` — they hold keys and local state.
19. A green compile is not a working change. Drive the affected flow in the built exe, or say
    plainly that the behaviour is unverified. FireTest is the scripted way to do it
    (→ [`docs/FIRETEST.md`](FIRETEST.md)).
