#!/usr/bin/env bash
# Launch the Linux terminal on a virtual X display and prove a window with a drawn frame came up.
#
# A green `cargo build` says the terminal links on Linux; it does not say the binary starts. This
# is the first place a missing runtime library, a Vulkan surface that will not configure or a
# startup panic shows, instead of a user's laptop. The run uses `--fixture`: the real application
# on a committed day of candles with a synthetic core, so it needs no network, no key and no
# saved configuration.
#
# What it sets up, because a desktop session has all of it and a container has none:
#   - a D-Bus session bus (the script re-executes itself under `dbus-run-session`);
#   - an unlocked Secret Service (gnome-keyring), the backend `servers.enc`'s machine key lives in.
#     The script proves it with its own store/lookup round trip, and that is all the proof there
#     is: a `--fixture` run configures its core in plaintext and never asks the keyring, so the
#     terminal's own keyring path is NOT exercised here;
#   - an X server (Xvfb). The GPU is whatever Vulkan driver the image carries — lavapipe, Mesa's
#     software rasterizer, on a hosted runner.
#
# It fails when the window never appears, the process exits before the frame is captured, or the
# captured frame is blank. A blank frame is measured, not assumed: a window of one flat colour has
# a handful of distinct colours, a drawn interface has thousands.
#
# The frame that is judged is the terminal's OWN window, read by its id. The whole screen is
# captured beside it for the artifact only: the window uses a 32-bit visual on a 24-bit screen,
# and what a root-window capture shows for such a child is left to the X server.
#
# Usage: linux-gui-smoke.sh <path to moonterminal> <fixtures directory> <output directory>
# Requires: Xvfb, dbus-run-session, gnome-keyring-daemon, secret-tool, xwininfo, and ImageMagick's
# `import` and `identify`. `vulkaninfo`, when present, is recorded for the artifact. Linux only — it is never run on the macOS checkout, but stays inside the
# portable shell subset the repository's script contract enforces.

set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "usage: $0 <path to moonterminal> <fixtures directory> <output directory>" >&2
    exit 2
fi

# The D-Bus session must exist before the keyring daemon starts, and both must outlive every
# command below, so the whole script moves under one session bus rather than wrapping one step.
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    exec dbus-run-session -- bash "$0" "$@"
fi

binary=$1
fixtures=$2
out=$3

# Seconds to wait for the main window, and then for the chart to load and draw its first frames.
# Measured against a software rasterizer on a hosted runner, which is the slowest case this runs on.
window_wait_secs=120
settle_secs=20
# Fewer distinct colours than this is a blank or flat frame.
min_colours=256

display=:99
mkdir -p "$out"
out=$(cd "$out" && pwd)

app_pid=""
xvfb_pid=""

# Stop what this script started and keep the application's own logs for the artifact.
cleanup() {
    if [ -n "$app_pid" ]; then
        kill "$app_pid" 2>/dev/null || true
    fi
    if [ -n "$xvfb_pid" ]; then
        kill "$xvfb_pid" 2>/dev/null || true
    fi
    for root in "${TMPDIR:-/tmp}"/moonterminal-fixture-*; do
        if [ -d "$root/logs" ]; then
            cp -R "$root/logs" "$out/app-logs" 2>/dev/null || true
        fi
    done
    note="${TMPDIR:-/tmp}/moonterminal-fixture-error.log"
    if [ -f "$note" ]; then
        cp "$note" "$out/" 2>/dev/null || true
    fi
    # The crash report is the one file the terminal writes relative to its working directory.
    if [ -f panic.log ]; then
        cp panic.log "$out/" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Print the tail of what the application wrote, then fail with the reason.
fail() {
    echo "linux-gui-smoke: FAIL — $1" >&2
    for log in "$out/stdout.log" "$out/stderr.log"; do
        if [ -s "$log" ]; then
            echo "--- tail of $log" >&2
            tail -n 60 "$log" >&2
        fi
    done
    exit 1
}

export DISPLAY=$display
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    XDG_RUNTIME_DIR=$(mktemp -d)
    chmod 700 "$XDG_RUNTIME_DIR"
    export XDG_RUNTIME_DIR
fi

Xvfb "$display" -screen 0 1600x900x24 -nolisten tcp >"$out/xvfb.log" 2>&1 &
xvfb_pid=$!
waited=0
until xwininfo -root >/dev/null 2>&1; do
    if ! kill -0 "$xvfb_pid" 2>/dev/null; then
        fail "Xvfb exited before it accepted a connection (see xvfb.log)"
    fi
    if [ "$waited" -ge 30 ]; then
        fail "Xvfb did not accept a connection within 30 s"
    fi
    sleep 1
    waited=$((waited + 1))
done

# The same two lines docs/MAC_LINUX_BUILD.md gives for a headless session, then a store/lookup
# round trip: a Secret Service that is present but locked answers the first call with a prompt
# nobody can click.
printf '%s\n' 'smoke' | gnome-keyring-daemon --unlock --components=secrets >/dev/null
eval "$(gnome-keyring-daemon --start --components=secrets)"
printf 'ping' | secret-tool store --label=moonterminal-smoke service moon-terminal-smoke key ping
if [ "$(secret-tool lookup service moon-terminal-smoke key ping)" != "ping" ]; then
    fail "the Secret Service round trip returned a different value"
fi
echo "linux-gui-smoke: Secret Service is unlocked"

# Which X server, which capture tool and which Vulkan driver this run had: a blank frame on one
# image and a drawn one on another is otherwise two logs with nothing to tell them apart.
{
    Xvfb -version 2>&1 || true
    import -version 2>&1 | head -n 1 || true
    if command -v vulkaninfo >/dev/null 2>&1; then
        vulkaninfo --summary 2>&1 || true
    else
        echo "vulkaninfo is not installed"
    fi
} >"$out/environment.txt"
grep -E 'X.Org X Server|deviceName|driverInfo|apiVersion' "$out/environment.txt" || true

MOON_FIXTURE_DIR=$fixtures "$binary" --fixture >"$out/stdout.log" 2>"$out/stderr.log" &
app_pid=$!

waited=0
until xwininfo -root -tree 2>/dev/null | grep -qi 'moonterminal'; do
    if ! kill -0 "$app_pid" 2>/dev/null; then
        app_pid=""
        fail "the terminal exited before it opened a window"
    fi
    if [ "$waited" -ge "$window_wait_secs" ]; then
        fail "no MoonTerminal window within ${window_wait_secs} s"
    fi
    sleep 1
    waited=$((waited + 1))
done
echo "linux-gui-smoke: window appeared after ${waited} s"
xwininfo -root -tree >"$out/windows.txt" 2>&1 || true

sleep "$settle_secs"
if ! kill -0 "$app_pid" 2>/dev/null; then
    app_pid=""
    fail "the terminal exited within ${settle_secs} s of opening its window"
fi

window_id=$(xwininfo -root -tree | awk 'tolower($0) ~ /moonterminal/ { print $1; exit }')
if [ -z "$window_id" ]; then
    fail "the MoonTerminal window disappeared before the frame was captured"
fi
xwininfo -id "$window_id" >"$out/window-info.txt" 2>&1 || true

import -display "$display" -window root "$out/screen.png"
echo "linux-gui-smoke: whole screen has $(identify -format '%k' "$out/screen.png") distinct colours"
import -display "$display" -window "$window_id" "$out/window.png"
colours=$(identify -format '%k' "$out/window.png")
echo "linux-gui-smoke: the terminal's window has ${colours} distinct colours"
if [ "$colours" -lt "$min_colours" ]; then
    fail "the terminal's window is blank (${colours} distinct colours, need ${min_colours})"
fi

echo "linux-gui-smoke: PASS"
