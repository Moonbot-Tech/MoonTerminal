#!/usr/bin/env bash
# Start the x86_64 station on an emulated CPU with no AVX and require it to stay up and stop cleanly.
#
# The station runs on the user's own server, and plenty of those are older than Haswell: no AVX2,
# no FMA, no BMI. Its Rust code targets baseline x86-64 and picks wider instructions only after
# asking the CPU, but the C inside its dependencies (SQLite, ring's C) takes whatever the runner's
# C compiler defaults to. v0.67.1 shipped C built for x86-64-v3 after `ubuntu-latest` moved to
# 26.04: on a Xeon E5-2690 v2 the station died with SIGILL 40 ms into its start and the server's
# updater rolled it back. The runner's own CPU has every extension, so nothing else in CI sees it;
# only a CPU model without them does.
#
# Two runs under `qemu-x86_64 -cpu qemu64`, which has SSE2 and nothing past it:
#   1. a control — a program executing one AVX2 instruction must die with SIGILL there, or the
#      emulator is not enforcing the CPU model and a green result below would prove nothing;
#   2. the station itself, from an empty data root with one inactive core and self-update off, so
#      it opens its logger, config and SQLite replica and enters its main loop without the
#      network. After ALIVE_SECS it gets SIGTERM and must log "stopped" and exit 0.
#
# Usage: station-baseline-smoke.sh <path to an x86_64 moon-station>
# Requires: qemu-x86_64 (Debian/Ubuntu package `qemu-user`) and musl-gcc (`musl-tools`).
# Linux only — it is never run on the macOS checkout, but stays inside the portable shell subset
# the repository's script contract enforces (no `timeout`: the waits below are bounded loops).

set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <path to an x86_64 moon-station>" >&2
    exit 2
fi
station=$1
# SSE2 and nothing more: what the x86_64 target promises, and below every server CPU in use.
cpu=qemu64
# How long the station must stay up before it is asked to stop. The server's own updater waits
# 30 s on real hardware; the fault this guards against comes within the first second.
alive_secs=15
# How long a stop may take: the station waits up to 15 s for its recorders' last writes.
stop_secs=40

for tool in qemu-x86_64 musl-gcc; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found" >&2
        exit 1
    fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 1. The control.
cat >"$work/avx2.c" <<'EOF'
int main(void) {
    __asm__ volatile("vpaddd %%ymm0, %%ymm0, %%ymm0" ::: "xmm0");
    return 0;
}
EOF
musl-gcc -static -O0 -o "$work/avx2" "$work/avx2.c"
control=0
qemu-x86_64 -cpu "$cpu" "$work/avx2" 2>/dev/null || control=$?
if [ "$control" -ne 132 ]; then
    echo "error: an AVX2 instruction under -cpu $cpu exited $control, not SIGILL (132):" >&2
    echo "the emulator does not enforce the CPU model, so this smoke cannot judge the station" >&2
    exit 1
fi
echo "control: AVX2 under -cpu $cpu raises SIGILL, as it must"

# 2. The station: the helper query first, then a full start.
qemu-x86_64 -cpu "$cpu" "$station" capabilities

mkdir -p "$work/data"
cat >"$work/station.toml" <<'EOF'
[update]
auto = false

[[core]]
uid = 1
name = "smoke"
active = false
EOF

qemu-x86_64 -cpu "$cpu" "$station" --data "$work/data" --config "$work/station.toml" \
    >"$work/out.log" 2>&1 &
pid=$!

show_output() {
    echo "--- station output ---" >&2
    cat "$work/out.log" >&2 || true
    if [ -d "$work/data/logs" ]; then
        echo "--- station logs ---" >&2
        cat "$work/data/logs"/* >&2 || true
    fi
}

waited=0
while [ "$waited" -lt "$alive_secs" ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
        status=0
        wait "$pid" || status=$?
        show_output
        if [ "$status" -eq 132 ]; then
            echo "error: the station died with SIGILL after ${waited}s on a CPU without AVX:" >&2
            echo "something in it was compiled past baseline x86-64" >&2
        else
            echo "error: the station exited $status after ${waited}s instead of staying up" >&2
        fi
        exit 1
    fi
    sleep 1
    waited=$((waited + 1))
done

# It may have died in the last second of the wait: the loop below then reaps it and reports.
kill -TERM "$pid" 2>/dev/null || true
waited=0
while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge "$stop_secs" ]; then
        kill -KILL "$pid" 2>/dev/null || true
        wait "$pid" || true
        show_output
        echo "error: the station did not stop within ${stop_secs}s of SIGTERM" >&2
        exit 1
    fi
    sleep 1
    waited=$((waited + 1))
done
status=0
wait "$pid" || status=$?
if [ "$status" -ne 0 ]; then
    show_output
    echo "error: the station exited $status on SIGTERM" >&2
    exit 1
fi
# The file log is TSV (`time level target message`); main.rs's own last line, not any writer's.
tab=$(printf '\t')
if ! grep -rqs "${tab}moon_station${tab}stopped\$" "$work/data/logs"; then
    show_output
    echo "error: the station exited without logging \"stopped\": it never reached its main loop" >&2
    exit 1
fi
echo "station: up ${alive_secs}s under -cpu $cpu, stopped cleanly on SIGTERM"
