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

## Release versioning

- Release tags use `vMAJOR.MINOR.PATCH` (for example, `v0.24.1`). Small fixes and maintenance
  releases increment **PATCH** only: `v0.24.1` → `v0.24.2` → `v0.24.3`.
- Increment **MINOR** only for a substantial feature or other major product change: after the
  `v0.24.x` line, such a release starts at `v0.25.0`. Never increment MINOR merely because another
  small fix is being published.
- If the user requests a version that conflicts with this policy, stop before creating or
  publishing the tag and explicitly clarify whether they intend an exception. Do not silently
  follow either the conflicting request or this default rule.

## Before you push

6. `cargo test --workspace` is green on your machine before the push, not just on CI afterwards.
7. Run `cargo clippy` — CI does not (→ `CONTRIBUTING.md` § Commits and PRs).
8. The tree is rustfmt-clean: run `cargo fmt --all` (or `make fmt`) before you push
   (→ `CONTRIBUTING.md` § Commits and PRs).
9. Read `git status` before every push; never `git add -f` — fix the over-broad ignore instead.
10. Never commit `Cargo.lock` carrying local sibling `path` entries — it breaks every other
    machine and all of CI (→ `CONTRIBUTING.md` § Dependencies and the lockfile).
11. Hand the finished work to **one or two separate agents with a clean context** for review —
    they see none of your reasoning and get only the task and the diff. One is enough for a small,
    single-file change; take two, on different angles, once the change spans several files or
    touches behaviour users depend on. If your own configuration already schedules such a pass,
    follow it; if it says nothing about one, this rule is what requires it. An agent reviewing its
    own work re-reads its own intent instead of the code, and the only mistakes it can still find
    are the ones it did not already make.
12. If you close the task by fanning out cleanup or review agents of your own — the usual
    reuse / simplification / efficiency / altitude split — one agent in that same fan-out is the
    clean-context reviewer of rule 11, launched beside the others and never instead of them. Same
    condition: if your configuration already schedules such a pass, follow it; if it says nothing,
    this rule is what adds it. A swarm that only polishes what was written never asks whether
    writing it was right.
13. Before opening the PR, check the open issues: if the change closes one, link it with a
    `Closes #N` line in the PR description (GitHub then closes the issue on merge and ties it to
    the PR) and leave a comment on the issue saying what fixed it. An issue closed by hand with
    no PR reference loses the trail from the report to the code.

## Code

14. Comments and docstrings in English, every one you write or rewrite
    (→ `CONTRIBUTING.md` § Comments and strings).
15. MoonUI first — never hand-roll a widget the stack already has
    (→ `CONTRIBUTING.md` § UI).
16. Treat UI/UX quality as a functional requirement for every new or materially changed
    interface: establish a clear visual hierarchy, use consistent spacing, alignment and existing
    design tokens, and define loading, empty, error, disabled, hover, focus and selected states.
    Preserve keyboard accessibility and readable contrast, then verify the result in the running
    app at narrow and wide sizes and in every supported theme; a compile or a happy-path screenshot
    alone is not sufficient.
17. Never silence a warning to get green — `#[allow(...)]`, an `unwrap()` over the error, a
    suppression comment. Fix the cause; if you genuinely must suppress, say why in the same line.

## Tests

18. Never edit a test to make it pass — fix the code. A red test is information, and deleting the
    messenger costs you the message.
19. A test must fail on a real regression — if you cannot name the edit that breaks it, drop it
    (→ `CONTRIBUTING.md` § What makes a test worth keeping).
20. Unit tests live in a sibling `tests.rs`, never in an inline `#[cfg(test)] mod tests { … }`
    block (→ `CONTRIBUTING.md` § Tests).

## Safety

21. Never force-push and never reset a shared `main` — fix forward with a new commit or a revert.
22. Never commit `servers.enc`, `cfg/`, `data/` or `backups/` — they hold keys and local state.
23. A green compile is not a working change. Drive the affected flow in the built exe, or say
    plainly that the behaviour is unverified. FireTest is the scripted way to do it
    (→ [`docs/FIRETEST.md`](FIRETEST.md)).
