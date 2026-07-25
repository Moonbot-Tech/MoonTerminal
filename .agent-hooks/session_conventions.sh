#!/usr/bin/env bash
# Shared SessionStart hook for Claude and Codex: put this repository's code conventions
# in front of the assistant before it writes anything.
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

root=$(git rev-parse --show-toplevel 2>/dev/null) || root="${CLAUDE_PROJECT_DIR:-.}"
doc="$root/CONTRIBUTING.md"

# A checkout without the file (older branch, sparse checkout) is not an error — say nothing.
if [ ! -f "$doc" ]; then
    exit 0
fi

python_cmd=""
if command -v python3 >/dev/null 2>&1 && python3 -c "import sys" >/dev/null 2>&1; then
    python_cmd="python3"
elif command -v python >/dev/null 2>&1 && python -c "import sys" >/dev/null 2>&1; then
    python_cmd="python"
fi

if [ -n "$python_cmd" ]; then
    "$python_cmd" -c "
import json
import pathlib
import sys

content = pathlib.Path(sys.argv[1]).read_text(encoding='utf-8')
print(json.dumps({'hookSpecificOutput': {
    'hookEventName': 'SessionStart',
    'additionalContext': 'Code conventions for this repository (CONTRIBUTING.md):\n'
                         'These bind every change. Full text below.\n\n'
                         + content,
}}))
" "$doc" 2>/dev/null
else
    printf '%s\n' "Code conventions for this repository (CONTRIBUTING.md):"
    printf '%s\n\n' "These bind every change. Full text below."
    cat "$doc" 2>/dev/null
fi

exit 0
