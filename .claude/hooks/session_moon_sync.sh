#!/usr/bin/env bash
# SessionStart hook (LOCAL to this checkout): start every session on what the
# other developers have actually pushed — the repo itself AND the Moonbot-owned
# Cargo git dependencies (MoonUI, MoonProtoBeta, ...).
#
# Why the dependencies need a step of their own: they are git deps pinned by a
# git-ignored, LOCAL-ONLY Cargo.lock. Cargo never re-resolves a branch dep once
# locked, so this checkout can sit on a months-old MoonUI while CI — which has
# no Cargo.lock — compiles the current one, and the two silently disagree.
#
# Safety rules, in order of importance:
#   * NEVER touch a dirty tree or a non-main branch — report only.
#   * NEVER fail the session: every path exits 0, all network work is bounded.
#   * ONLY Moonbot-owned sources are refreshed. Third-party forks (zed, smol-rs)
#     stay pinned on purpose — auto-bumping those is how builds break.
#
# Set MOON_SYNC_DEPS=0 to report dependency drift without running cargo update.
#
# Two Windows traps this script exists to sidestep, both found by testing:
#   * python print() emits CRLF here, so a naive compare against a git hash
#     never matches — the \r is stripped explicitly below.
#   * tab is IFS whitespace, so `read` COLLAPSES consecutive tabs and an empty
#     field (a dep with no explicit branch) silently shifts every later field.
#     Hence '|' as the separator, which is not IFS whitespace.

git rev-parse --git-dir >/dev/null 2>&1 || exit 0
root=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
origin=$(git -C "$root" remote get-url origin 2>/dev/null) || exit 0
case "$origin" in
    *Moonbot-Tech/MoonTerminal*) ;;
    *) exit 0 ;;
esac
cd "$root" 2>/dev/null || exit 0

report=""
add() { report="${report}${1}"$'\n'; }

# --- 1. the repository itself -------------------------------------------------
timeout 40 git fetch origin --prune -q 2>/dev/null
branch=$(git branch --show-current 2>/dev/null)
dirty=$(git status --porcelain 2>/dev/null | head -c 1)
behind=$(git rev-list --count HEAD..origin/main 2>/dev/null || echo 0)
ahead=$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 0)

if [ "${behind:-0}" -eq 0 ] && [ "${ahead:-0}" -eq 0 ]; then
    add "MoonTerminal: свежий (origin/main)"
elif [ "${behind:-0}" -eq 0 ]; then
    add "MoonTerminal: свежий, но на ${ahead} коммит(ов) ВПЕРЕДИ origin/main — есть незапушенное"
elif [ "$branch" != "main" ]; then
    add "MoonTerminal: origin/main ушёл на ${behind} — вы на ветке '${branch}', НЕ подтянуто"
elif [ -n "$dirty" ]; then
    add "MoonTerminal: origin/main ушёл на ${behind} — в дереве есть изменения, НЕ подтянуто"
elif [ "${ahead:-0}" -gt 0 ]; then
    add "MoonTerminal: расхождение — ${behind} позади, ${ahead} впереди, НЕ подтянуто, нужен разбор"
elif timeout 30 git merge --ff-only origin/main -q 2>/dev/null; then
    add "MoonTerminal: подтянуто ${behind} коммит(ов) из origin/main"
else
    add "MoonTerminal: ff-merge не прошёл (${behind} позади) — разберитесь вручную"
fi

# --- 2. Moonbot-owned cargo git deps -----------------------------------------
if [ -f Cargo.lock ]; then
    # '|'-separated so empty fields survive `read`; \r stripped for the compare.
    deps=$(python -c "
import sys
seen, name = {}, None
try:
    fh = open('Cargo.lock', encoding='utf-8')
except OSError:
    sys.exit(0)
for line in fh:
    line = line.strip()
    if line.startswith('name = '):
        name = line.split('\"')[1]
    elif line.startswith('source = \"git+') and 'Moonbot-Tech' in line:
        src = line.split('\"')[1][4:]
        url, _, rev = src.partition('#')
        url, _, query = url.partition('?')
        branch = query.split('branch=')[1].split('&')[0] if 'branch=' in query else ''
        prev = seen.get(url)
        # Any package from the source moves the whole source, but prefer a
        # recognisable name so the printed 'cargo update -p X' reads sensibly.
        if prev is None or (name in ('moon-gpui', 'moon-ui', 'moonproto') and prev[0] not in ('moon-gpui', 'moon-ui', 'moonproto')):
            seen[url] = (name, branch, rev)
for url, (name, branch, rev) in seen.items():
    print('|'.join((name, url, branch, rev)))
" 2>/dev/null | tr -d '\r')

    stale_flags=""
    stale_names=""
    while IFS='|' read -r pkg url branch rev; do
        [ -z "$pkg" ] && continue
        short=${url##*/}
        if [ -n "$branch" ]; then
            head=$(timeout 25 git ls-remote "$url" "refs/heads/$branch" 2>/dev/null | cut -f1)
        else
            head=$(timeout 25 git ls-remote "$url" HEAD 2>/dev/null | cut -f1)
        fi
        if [ -z "$head" ]; then
            add "${short}: проверить не удалось (сеть/доступ) — пин ${rev:0:8}"
        elif [ "$head" = "$rev" ]; then
            add "${short}: свежий (${rev:0:8})"
        else
            stale_flags="${stale_flags} -p ${pkg}"
            stale_names="${stale_names}${short} "
            add "${short}: ОТСТАЁТ — пин ${rev:0:8}, сейчас ${head:0:8}"
        fi
    done <<< "$deps"

    if [ -n "$stale_flags" ]; then
        if [ "${MOON_SYNC_DEPS:-1}" = "0" ] || ! command -v cargo >/dev/null 2>&1; then
            add "-> обновить вручную: cargo update${stale_flags}"
        elif timeout 180 cargo update $stale_flags >/dev/null 2>&1; then
            add "-> обновлено: ${stale_names}(Cargo.lock переписан — следующая сборка будет длиннее)"
        else
            add "-> cargo update НЕ прошёл, обновите вручную: cargo update${stale_flags}"
        fi
    fi
fi

REPORT="$report" python -c "
import json, os
print(json.dumps({'hookSpecificOutput': {
    'hookEventName': 'SessionStart',
    'additionalContext': 'Свежесть MoonTerminal и Moonbot-зависимостей (хук на старте сессии):\n'
                         + os.environ['REPORT'].strip(),
}}))" 2>/dev/null
exit 0
