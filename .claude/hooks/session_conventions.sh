#!/usr/bin/env bash
# SessionStart hook: put this repo's code conventions in front of the assistant before
# it writes anything, instead of hoping it goes looking for them.
#
# The rules live in CONTRIBUTING.md, which is the file humans read too — one source, so the
# two cannot drift. This hook only delivers it; it never edits or interprets it.
#
# Cost, stated plainly: the file is injected in full on every session (~2k tokens today).
# That is deliberate. A pointer ("the conventions are in CONTRIBUTING.md") only works if the
# file actually gets read, and the rules it carries — where Rust tests live, comment language,
# which changes need a PR — are the ones that are expensive to discover after the fact, in
# review. If CONTRIBUTING.md ever grows past a few hundred lines, revisit this trade.
#
# Safety, same rule as the sibling sync hook: NEVER fail the session. Every path exits 0.

set -u

doc="${CLAUDE_PROJECT_DIR:-.}/CONTRIBUTING.md"

# A checkout without the file (older branch, sparse checkout) is not an error — say nothing.
if [ ! -f "$doc" ]; then
    exit 0
fi

printf '%s\n' "=== Code conventions for this repository (CONTRIBUTING.md) ==="
printf '%s\n' "These bind every change. Full text below."
printf '\n'
cat "$doc"

exit 0
