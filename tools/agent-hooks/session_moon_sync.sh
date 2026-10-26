#!/usr/bin/env bash
# Shared SessionStart hook for Claude and Codex: start every session on what
# other developers have actually pushed — the repo itself AND the Moonbot-owned
# Cargo git dependencies (MoonUI, MoonProtoBeta, ...).
#
# Why the dependencies need a step of their own: Cargo.lock is COMMITTED and
# freezes every third-party version, while cargo never re-resolves a branch dep
# once locked. CI refreshes MoonUI on every run, so this checkout sits on the
# committed MoonUI pin and lags CI by design — this step tells you by how much.
#
# Safety rules, in order of importance:
#   * NEVER touch a dirty tree or a non-main branch — report only.
#   * NEVER fail the session: every path exits 0, all network work is bounded.
#   * ONLY MoonUI is ever refreshed automatically. MoonProto moves solely by a
#     deliberate human `cargo update -p moonproto` + commit, and third-party
#     forks (zed, smol-rs) stay pinned on purpose — auto-bumping those is how
#     builds break.
#
# REPORT-ONLY by default, because Cargo.lock is tracked: an automatic rewrite
# would hand you a dirty tree you did not create, one commit away from riding
# along in an unrelated PR. Set MOON_SYNC_DEPS=1 to let it run cargo update.
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

# Append one line to the SessionStart freshness report.
add() { report="${report}${1}"$'\n'; }

# Run one command with a portable wall-clock limit.
run_with_timeout() {
    local limit=$1
    shift

    if command -v timeout >/dev/null 2>&1; then
        timeout "$limit" "$@"
        return
    fi
    if command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$limit" "$@"
        return
    fi

    "$@" &
    local command_pid=$!
    (
        local remaining=$limit
        while [ "$remaining" -gt 0 ]; do
            sleep 1
            kill -0 "$command_pid" 2>/dev/null || exit 0
            remaining=$((remaining - 1))
        done
        kill -TERM "$command_pid" 2>/dev/null
    ) &
    local timer_pid=$!
    wait "$command_pid" 2>/dev/null
    local status=$?
    kill "$timer_pid" 2>/dev/null
    wait "$timer_pid" 2>/dev/null
    return "$status"
}

# Refresh the repository state used by every write guard.
refresh_worktree_state() {
    branch=$(git branch --show-current 2>/dev/null)
    dirty=$(git status --porcelain 2>/dev/null | head -c 1)
    behind=$(git rev-list --count HEAD..origin/main 2>/dev/null || echo 0)
    ahead=$(git rev-list --count origin/main..HEAD 2>/dev/null || echo 0)
}

# Acquire the atomic directory lock, recovering it only when its owner is dead.
acquire_sync_lock() {
    local lock_pid=""
    local wait_count=0

    if mkdir "$sync_lock" 2>/dev/null; then
        if printf '%s\n' "$$" > "$sync_lock/pid" 2>/dev/null; then
            return 0
        fi
        rmdir "$sync_lock" 2>/dev/null
        return 1
    fi

    while [ "$wait_count" -lt 2 ] && [ ! -s "$sync_lock/pid" ]; do
        sleep 1
        wait_count=$((wait_count + 1))
    done
    read -r lock_pid 2>/dev/null < "$sync_lock/pid" || lock_pid=""
    if [[ "$lock_pid" =~ ^[0-9]+$ ]] && kill -0 "$lock_pid" 2>/dev/null; then
        return 1
    fi

    rm -f -- "$sync_lock/pid" 2>/dev/null
    rmdir "$sync_lock" 2>/dev/null || return 1
    mkdir "$sync_lock" 2>/dev/null || return 1
    if printf '%s\n' "$$" > "$sync_lock/pid" 2>/dev/null; then
        return 0
    fi
    rmdir "$sync_lock" 2>/dev/null
    return 1
}

# Release the atomic directory lock acquired by this hook process.
release_sync_lock() {
    if [ "$lock_held" = "1" ]; then
        rm -f -- "$sync_lock/pid" 2>/dev/null
        rmdir "$sync_lock" 2>/dev/null
    fi
}

python_cmd=""
if command -v python3 >/dev/null 2>&1 && python3 -c "import sys" >/dev/null 2>&1; then
    python_cmd="python3"
elif command -v python >/dev/null 2>&1 && python -c "import sys" >/dev/null 2>&1; then
    python_cmd="python"
fi

sync_lock=$(git rev-parse --git-path moonterminal-session-sync.lock 2>/dev/null)
lock_held=0
if [ -n "$sync_lock" ] && acquire_sync_lock; then
    lock_held=1
    trap release_sync_lock EXIT
else
    add "MoonTerminal: другой sync hook уже работает — эта сессия только проверяет состояние"
fi

# --- 1. the repository itself -------------------------------------------------
if [ "$lock_held" = "1" ] && ! run_with_timeout 40 git fetch origin --prune -q 2>/dev/null; then
    add "MoonTerminal: git fetch не прошёл (сеть/доступ) — состояние origin/main может быть устаревшим"
fi
refresh_worktree_state

if [ "$branch" != "main" ]; then
    add "MoonTerminal: вы на ветке '${branch:-detached HEAD}', НЕ подтянуто"
elif [ -n "$dirty" ]; then
    add "MoonTerminal: origin/main ушёл на ${behind} — в дереве есть изменения, НЕ подтянуто"
elif [ "${behind:-0}" -eq 0 ] && [ "${ahead:-0}" -eq 0 ]; then
    add "MoonTerminal: свежий (origin/main)"
elif [ "${behind:-0}" -eq 0 ]; then
    add "MoonTerminal: свежий, но на ${ahead} коммит(ов) ВПЕРЕДИ origin/main — есть незапушенное"
elif [ "${ahead:-0}" -gt 0 ]; then
    add "MoonTerminal: расхождение — ${behind} позади, ${ahead} впереди, НЕ подтянуто, нужен разбор"
elif [ "$lock_held" != "1" ]; then
    add "MoonTerminal: origin/main ушёл на ${behind} — другой sync hook работает, НЕ подтянуто"
else
    refresh_worktree_state
    if [ "$branch" != "main" ] || [ -n "$dirty" ] || [ "${ahead:-0}" -gt 0 ]; then
        add "MoonTerminal: состояние изменилось во время проверки — НЕ подтянуто"
    elif run_with_timeout 30 git merge --ff-only origin/main -q 2>/dev/null; then
        add "MoonTerminal: подтянуто ${behind} коммит(ов) из origin/main"
    else
        add "MoonTerminal: ff-merge не прошёл (${behind} позади) — разберитесь вручную"
    fi
fi

# --- 2. Moonbot-owned cargo git deps -----------------------------------------
if [ -f Cargo.lock ] && [ -n "$python_cmd" ]; then
    # '|'-separated so empty fields survive `read`; \r stripped for the compare.
    deps=$("$python_cmd" -c "
import sys
from urllib.parse import urlsplit

seen, name = {}, None
try:
    fh = open('Cargo.lock', encoding='utf-8')
except OSError:
    sys.exit(0)
for line in fh:
    line = line.strip()
    if line.startswith('name = '):
        name = line.split('\"')[1]
    elif line.startswith('source = \"git+'):
        src = line.split('\"')[1][4:]
        url, _, rev = src.partition('#')
        url, _, query = url.partition('?')
        parsed = urlsplit(url)
        parts = parsed.path.strip('/').split('/')
        if parsed.hostname != 'github.com' or len(parts) < 2 or parts[0] != 'Moonbot-Tech':
            continue
        branch = query.split('branch=')[1].split('&')[0] if 'branch=' in query else ''
        prev = seen.get(url)
        # Any package from the source moves the whole source, but prefer a
        # recognisable name so the printed 'cargo update -p X' reads sensibly.
        if prev is None or (name in ('moon-gpui', 'moon-ui', 'moonproto') and prev[0] not in ('moon-gpui', 'moon-ui', 'moonproto')):
            seen[url] = (name, branch, rev)
for url, (name, branch, rev) in seen.items():
    print('|'.join((name, url, branch, rev)))
" 2>/dev/null | tr -d '\r')

    cargo_args=()
    manual_command="cargo update"
    stale_names=""
    while IFS='|' read -r pkg url branch rev; do
        [ -z "$pkg" ] && continue
        short=${url##*/}
        if [[ ! "$pkg" =~ ^[A-Za-z0-9][A-Za-z0-9_-]*$ ]]; then
            add "${short}: некорректное имя пакета в Cargo.lock — пропущено"
            continue
        fi
        if [ -n "$branch" ]; then
            head=$(run_with_timeout 25 git ls-remote "$url" "refs/heads/$branch" 2>/dev/null | cut -f1)
        else
            head=$(run_with_timeout 25 git ls-remote "$url" HEAD 2>/dev/null | cut -f1)
        fi
        if [ -z "$head" ]; then
            add "${short}: проверить не удалось (сеть/доступ) — пин ${rev:0:8}"
        elif [ "$head" = "$rev" ]; then
            add "${short}: свежий (${rev:0:8})"
        elif [ "$pkg" = "moonproto" ]; then
            # Deliberately outside the automatic path: MoonProto is frozen until a human moves it
            # and commits the tracked lock. CI never bumps it either.
            add "${short}: ОТСТАЁТ — пин ${rev:0:8}, сейчас ${head:0:8}; двигается только вручную: cargo update -p moonproto + коммит"
        else
            cargo_args+=("-p" "$pkg")
            manual_command="${manual_command} -p ${pkg}"
            stale_names="${stale_names}${short} "
            add "${short}: ОТСТАЁТ — пин ${rev:0:8}, сейчас ${head:0:8}"
        fi
    done <<< "$deps"

    if [ "${#cargo_args[@]}" -gt 0 ]; then
        refresh_worktree_state
        update_blocker=""
        if [ "$lock_held" != "1" ]; then
            update_blocker="другой sync hook уже работает"
        elif [ "$branch" != "main" ] && [ -n "$dirty" ]; then
            update_blocker="ветка '${branch:-detached HEAD}' не main и в дереве есть изменения"
        elif [ "$branch" != "main" ]; then
            update_blocker="ветка '${branch:-detached HEAD}' не main"
        elif [ -n "$dirty" ]; then
            update_blocker="в дереве есть изменения"
        elif [ "${behind:-0}" -gt 0 ] && [ "${ahead:-0}" -gt 0 ]; then
            update_blocker="main расходится с origin/main"
        elif [ "${behind:-0}" -gt 0 ]; then
            update_blocker="main отстаёт от origin/main"
        elif [ "${ahead:-0}" -gt 0 ]; then
            update_blocker="main впереди origin/main"
        fi

        if [ -n "$update_blocker" ]; then
            add "-> автоматически НЕ обновлено (${update_blocker}); после синхронизации main: ${manual_command}"
        elif [ "${MOON_SYNC_DEPS:-0}" != "1" ] || ! command -v cargo >/dev/null 2>&1; then
            add "-> обновить вручную: ${manual_command}"
        elif run_with_timeout 180 cargo update "${cargo_args[@]}" >/dev/null 2>&1; then
            add "-> обновлено: ${stale_names}(Cargo.lock ОТСЛЕЖИВАЕТСЯ и теперь изменён: закоммитьте осознанно или git checkout -- Cargo.lock)"
        else
            add "-> cargo update НЕ прошёл, обновите вручную: ${manual_command}"
        fi
    fi
elif [ -f Cargo.lock ]; then
    add "Moonbot-зависимости: проверить не удалось — нет python3/python"
fi

if [ -n "$python_cmd" ]; then
    REPORT="$report" "$python_cmd" -c "
import json, os
print(json.dumps({'hookSpecificOutput': {
    'hookEventName': 'SessionStart',
    'additionalContext': 'Свежесть MoonTerminal и Moonbot-зависимостей (хук на старте сессии):\n'
                         + os.environ['REPORT'].strip(),
}}))" 2>/dev/null
else
    printf '%s\n' "MoonTerminal and Moonbot freshness (SessionStart hook):"
    printf '%s' "$report"
fi
exit 0
