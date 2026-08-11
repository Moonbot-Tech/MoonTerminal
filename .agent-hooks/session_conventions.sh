#!/usr/bin/env bash
# Shared SessionStart hook for Claude and Codex: put this repository's conventions and its
# rules for AI agents in front of the assistant before it writes anything.
#
# Two documents, two roles, one delivery:
#   CONTRIBUTING.md      — the conventions themselves, the file humans read too. Authoritative.
#   docs/AGENT_RULES.md  — the discipline expected of an AI agent (plan first, what to run before
#                          pushing, what never to commit). It defers to CONTRIBUTING.md wherever
#                          the two touch, and outranks the agent's personal global config.
# Both are the files humans read too — one source each, so nothing can drift. This hook only
# delivers them; it never edits or interprets them. Add a third document by extending the
# `candidates` list below, and nothing else: neither .claude/settings.json nor .codex/hooks.json
# names the documents.
#
# Cost, stated plainly: both files are injected in full on every session (~2.5-3k tokens today).
# That is deliberate. A pointer ("the conventions are in CONTRIBUTING.md") only works if the
# file actually gets read, and the rules they carry — where Rust tests live, comment language,
# which changes need a PR, what a green compile does not prove — are the ones that are expensive
# to discover after the fact, in review. Revisit this trade if CONTRIBUTING.md grows past a few
# hundred lines, or if AGENT_RULES.md passes ~80: past that it has stopped being a rules list.
#
# Safety, same rule as the sibling sync hook: NEVER fail the session. Every path exits 0.

set -u

root=$(git rev-parse --show-toplevel 2>/dev/null) || root="${CLAUDE_PROJECT_DIR:-.}"

# Order is the reading order. AGENT_RULES.md goes last: it is the shorter, more directive file,
# and it is the one that must still be in view when the assistant starts working.
candidates=(
    "$root/CONTRIBUTING.md"
    "$root/docs/AGENT_RULES.md"
)

docs=()
for doc in "${candidates[@]}"; do
    if [ -f "$doc" ] && [ -r "$doc" ]; then
        docs+=("$doc")
    fi
done

# A checkout without the files (older branch, sparse checkout) is not an error — say nothing.
# One file present and the other missing still delivers the one that is there.
if [ "${#docs[@]}" -eq 0 ]; then
    exit 0
fi

python_cmd=""
if command -v python3 >/dev/null 2>&1 && python3 -c "import sys" >/dev/null 2>&1; then
    python_cmd="python3"
elif command -v python >/dev/null 2>&1 && python -c "import sys" >/dev/null 2>&1; then
    python_cmd="python"
fi

# Centralize each document's banner so the JSON and plain-text branches cannot drift apart.
banner_for() {
    case "$(basename "$1")" in
        CONTRIBUTING.md)
            printf '%s\n%s' \
                "Code conventions for this repository (CONTRIBUTING.md):" \
                "These bind every change. Full text below."
            ;;
        AGENT_RULES.md)
            printf '%s\n%s' \
                "Rules for AI coding agents (docs/AGENT_RULES.md):" \
                "These outrank your personal global configuration. Full text below."
            ;;
        *)
            printf '%s' "$(basename "$1"):"
            ;;
    esac
}

if [ -n "$python_cmd" ]; then
    banners=()
    for doc in "${docs[@]}"; do
        banners+=("$(banner_for "$doc")")
    done

    # argv is banner/path pairs: banner1 path1 banner2 path2 ...
    pairs=()
    i=0
    while [ "$i" -lt "${#docs[@]}" ]; do
        pairs+=("${banners[$i]}" "${docs[$i]}")
        i=$((i + 1))
    done

    "$python_cmd" -c "
import json
import pathlib
import sys

argv = sys.argv[1:]
sections = []
for banner, path in zip(argv[0::2], argv[1::2]):
    # One unreadable document must never cost the others. Undecodable bytes degrade to
    # replacement characters rather than raising: a mangled paragraph still delivers the
    # rules, an exception here would deliver nothing at all.
    try:
        content = pathlib.Path(path).read_text(encoding='utf-8', errors='replace')
    except OSError:
        continue
    sections.append(banner + '\n\n' + content)

if sections:
    print(json.dumps({'hookSpecificOutput': {
        'hookEventName': 'SessionStart',
        'additionalContext': '\n\n'.join(sections),
    }}))
" "${pairs[@]}" 2>/dev/null
else
    first=1
    for doc in "${docs[@]}"; do
        # Same rule as the python branch: skip a document that turned unreadable rather than
        # emitting a banner with nothing under it, so the two branches stay in agreement.
        if [ ! -r "$doc" ]; then
            continue
        fi
        if [ "$first" -eq 0 ]; then
            printf '\n\n'
        fi
        first=0
        banner_for "$doc"
        printf '\n\n'
        cat "$doc" 2>/dev/null
    done
fi

exit 0
