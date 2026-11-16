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
# a handful of distinct colours, a drawn interface has thousands. Under a window manager (below)
# it also fails when the terminal does not survive a resize, does not apply the new size, or
# shows a blank frame after it.
#
# `SMOKE_BLANK_FRAME=report` turns the blank-frame failure into a logged note, for an image whose
# software Vulkan driver is known not to show the window at all. Measured on 2026-11-10 with one
# binary: Mesa 22.3.6 (debian:12) and 23.2.1 (ubuntu:22.04) leave the terminal's 32-bit window one
# flat colour while presenting an ordinary 24-bit window (`vkcube`) correctly; Mesa 25.0.7 draws it
# — on debian:13, on ubuntu:24.04's own Mesa, and on ubuntu:22.04 with nothing but Mesa replaced.
# So that blank frame is the old rasterizer's presentation path, not the terminal, and on such an
# image the run still proves the binary loads, opens its window and stays up.
#
# The frame that is judged is the terminal's OWN window, read by its id. The whole screen is
# captured beside it for the artifact only: the window uses a 32-bit visual on a 24-bit screen,
# and what a root-window capture shows for such a child is left to the X server.
#
# `SMOKE_WINDOW_MANAGER=<command line>` runs the same check under a window manager and then
# resizes the window through it. A bare X server never configures a window after it was created,
# so that resize is the only place the terminal's handling of a size from the window manager
# runs at all. The proof that it ran is the terminal's own account: its X11 backend logs one
# `X11 resize, window N: <n> sizes applied, …; <m> frames, …` line per second of resizing, and the
# script waits for one that reports an applied size and a frame. The window's geometry cannot
# serve: the window manager sets it whether or not the terminal reacts. After that line the
# process and the window manager must still be alive and, if the first frame was drawn, the
# second (`window-resized.png`) must be drawn too.
#
# Usage: linux-gui-smoke.sh <path to moonterminal> <fixtures directory> <output directory>
# Requires: Xvfb, dbus-run-session, gnome-keyring-daemon, secret-tool, xwininfo, and ImageMagick's
# `import` and `identify`; with `SMOKE_WINDOW_MANAGER`, also the window manager, `xprop` and
# `xdotool`. `vulkaninfo`, when present, is recorded for the artifact. Linux only — it is never run on the macOS checkout, but stays inside the
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
# What a blank frame means on this image: `fail` (the default) or `report`.
blank_frame=${SMOKE_BLANK_FRAME:-fail}
if [ "$blank_frame" != "fail" ] && [ "$blank_frame" != "report" ]; then
    echo "SMOKE_BLANK_FRAME must be 'fail' or 'report', not '$blank_frame'" >&2
    exit 2
fi

# A window manager's command line, or empty for a bare X server. A desktop session has one, and
# the terminal's windows are treated differently under it: the window manager reparents, places
# and configures a window after the terminal created it. With this set, the manager is started
# first, and the terminal's window is captured a second time after being resized through it.
window_manager=${SMOKE_WINDOW_MANAGER:-}
# Seconds to wait for the window manager to announce itself, and for the terminal to report a
# resize as applied and drawn.
window_manager_wait_secs=30
resize_wait_secs=60
# The terminal's report of a resize it handled: at least one size applied and one frame after it.
# The wording is MoonUI's (`moon-gpui-linux/src/linux/x11/resize_diag.rs`); if it changes there,
# this check fails by timing out and names the line it waited for.
resize_report='X11 resize, window [0-9]+: [1-9][0-9]* sizes applied, .*; [1-9][0-9]* frames, '

display=:99
mkdir -p "$out"
out=$(cd "$out" && pwd)

app_pid=""
xvfb_pid=""
wm_pid=""

# Stop what this script started and keep the application's own logs for the artifact.
cleanup() {
    if [ -n "$app_pid" ]; then
        kill "$app_pid" 2>/dev/null || true
    fi
    if [ -n "$wm_pid" ]; then
        kill "$wm_pid" 2>/dev/null || true
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
    for log in "$out/stdout.log" "$out/stderr.log" "$out/window-manager.log"; do
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

if [ -n "$window_manager" ]; then
    # Started up to three times. In run 43960917934 xfwm4 exited at once with "cannot open
    # display" on a server that had just answered `xwininfo`, and started normally in the run
    # before it on the same script; why is not established, so an early exit is retried and
    # every attempt's output is kept.
    : >"$out/window-manager.log"
    wm_attempts=0
    wm_up=""
    while [ -z "$wm_up" ] && [ "$wm_attempts" -lt 3 ]; do
        wm_attempts=$((wm_attempts + 1))
        # A window manager that died leaves its announcement on the root window; without this
        # the next attempt would read the dead one's and pass before it had taken the display.
        xprop -root -remove _NET_SUPPORTING_WM_CHECK 2>/dev/null || true
        # Unquoted on purpose: the variable is a command line, split into the command and its
        # options.
        # shellcheck disable=SC2086
        $window_manager >>"$out/window-manager.log" 2>&1 &
        wm_pid=$!
        waited=0
        while kill -0 "$wm_pid" 2>/dev/null; do
            # A running window manager names its check window in this root property (EWMH).
            if xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q 'window id'; then
                wm_up=1
                break
            fi
            if [ "$waited" -ge "$window_manager_wait_secs" ]; then
                fail "the window manager did not take the display within ${window_manager_wait_secs} s"
            fi
            sleep 1
            waited=$((waited + 1))
        done
        if [ -z "$wm_up" ]; then
            wm_pid=""
            echo "linux-gui-smoke: window manager attempt ${wm_attempts} exited early"
            sleep 2
        fi
    done
    if [ -z "$wm_up" ]; then
        fail "the window manager exited before it took the display, three times (see window-manager.log)"
    fi
    echo "linux-gui-smoke: window manager is up after ${waited} s, attempt ${wm_attempts}: $window_manager"
fi

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

# Under a window manager, resize the window through it and look again. This is the first time
# since the window opened that the window manager hands the terminal a new size, which a bare X
# server never does: a panic or a hang on that path, a size the terminal never applies or never
# draws, and a window left blank by it show here and nowhere else in CI. A frame that is drawn
# but wrong is not judged; `window-resized.png` is kept for the eye.
if [ -n "$window_manager" ]; then
    xprop -id "$window_id" >"$out/window-props.txt" 2>&1 || true
    width=$(awk '/Width:/ { print $2; exit }' "$out/window-info.txt")
    height=$(awk '/Height:/ { print $2; exit }' "$out/window-info.txt")
    if [ -z "$width" ] || [ -z "$height" ]; then
        fail "the window's size could not be read before the resize (see window-info.txt)"
    fi
    if ! kill -0 "$wm_pid" 2>/dev/null; then
        wm_pid=""
        fail "the window manager exited before the resize (see window-manager.log)"
    fi
    new_width=$((width - 120))
    new_height=$((height - 80))
    # Only what the terminal logs from here on counts: the window manager also configures the
    # window while it opens.
    logged_before=$(wc -l <"$out/stderr.log")
    xdotool windowsize "$window_id" "$new_width" "$new_height" ||
        fail "xdotool could not resize the window to ${new_width}x${new_height}"
    waited=0
    # `grep` reads to the end on purpose: with `-q` it would close the pipe early and `pipefail`
    # would turn a match into a failure.
    until tail -n "+$((logged_before + 1))" "$out/stderr.log" | grep -E "$resize_report" >/dev/null; do
        if ! kill -0 "$app_pid" 2>/dev/null; then
            app_pid=""
            fail "the terminal exited after its window was resized to ${new_width}x${new_height}"
        fi
        if [ "$waited" -ge "$resize_wait_secs" ]; then
            fail "the terminal did not report the resize to ${new_width}x${new_height} as applied and drawn within ${resize_wait_secs} s (no line matching '${resize_report}' in stderr.log)"
        fi
        sleep 1
        waited=$((waited + 1))
    done
    echo "linux-gui-smoke: the terminal reported the resize after ${waited} s:"
    tail -n "+$((logged_before + 1))" "$out/stderr.log" | grep -E "$resize_report" | tail -n 1
    if ! kill -0 "$app_pid" 2>/dev/null; then
        app_pid=""
        fail "the terminal exited after its window was resized to ${new_width}x${new_height}"
    fi
    # Without the manager `xdotool` resizes the window on the bare server, and the run would
    # prove nothing about the path it exists for.
    if ! kill -0 "$wm_pid" 2>/dev/null; then
        wm_pid=""
        fail "the window manager exited during the resize (see window-manager.log)"
    fi
    # A precondition of the frame check below, not a check of the terminal: this is the size the
    # window manager gave the window.
    xwininfo -id "$window_id" >"$out/window-info-resized.txt" 2>&1 || true
    resized_width=$(awk '/Width:/ { print $2; exit }' "$out/window-info-resized.txt")
    resized_height=$(awk '/Height:/ { print $2; exit }' "$out/window-info-resized.txt")
    if [ "$resized_width" != "$new_width" ] || [ "$resized_height" != "$new_height" ]; then
        fail "the window is ${resized_width:-?}x${resized_height:-?} after a resize to ${new_width}x${new_height}"
    fi
    import -display "$display" -window "$window_id" "$out/window-resized.png" ||
        fail "the resized window could not be captured"
    resized_colours=$(identify -format '%k' "$out/window-resized.png") ||
        fail "the capture of the resized window could not be read"
    echo "linux-gui-smoke: after a resize to ${new_width}x${new_height} the window has" \
        "${resized_colours} distinct colours"
    # Judged only against a first frame that was drawn: on an image whose driver shows nothing,
    # both frames are blank and neither says anything about the resize.
    if [ "$colours" -ge "$min_colours" ] && [ "$resized_colours" -lt "$min_colours" ]; then
        fail "the window went blank after the resize (${resized_colours} distinct colours, need ${min_colours})"
    fi
fi
if [ "$colours" -lt "$min_colours" ]; then
    if [ "$blank_frame" = "report" ]; then
        echo "linux-gui-smoke: the terminal's window is blank (${colours} distinct colours);" \
            "this image is known to show nothing through its software Vulkan driver"
        echo "linux-gui-smoke: PASS (started and stayed up; frame not judged)"
        exit 0
    fi
    fail "the terminal's window is blank (${colours} distinct colours, need ${min_colours})"
fi

echo "linux-gui-smoke: PASS"
