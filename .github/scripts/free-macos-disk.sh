#!/usr/bin/env bash
# Reclaim disk on a GitHub-hosted macOS runner before a release build.
#
# The image ships several Xcode versions, every simulator runtime and an Android SDK, which is why
# a `macos-14` runner offers only ~14 GB free. Tagging v0.28.1 died at packaging with
# "hdiutil: create failed - No space left on device" AFTER both slices had compiled — the job spent
# its whole budget and shipped nothing.
#
# Only things a Rust build cannot use are removed, and the SELECTED Xcode is kept: the linker
# resolves the macOS SDK through it. Removals are best-effort, because image contents move between
# runner releases and a path that has been renamed is not a reason to fail a release — but the
# script ends by proving the toolchain still answers, so a cleanup that went too far fails HERE,
# loudly, instead of surfacing forty minutes later as an unrelated-looking link error.
#
# Deliberately NOT `set -e`: a single failed removal must not abort the job. The final check is the
# gate instead.
set -uo pipefail

# Available kibibytes on the volume the build runs from, for the before/after comparison below.
# `df -k` is POSIX; the BSD df here has no --output, so the field is picked positionally.
available_kib() {
    kib="$(df -k / | awk 'NR == 2 { print $4 }')"
    # A df that prints something unexpected must not take the script down before the SDK gate
    # below, which is the check that actually matters.
    case "$kib" in
    '' | *[!0-9]*) echo 0 ;;
    *) echo "$kib" ;;
    esac
}

echo "=== disk before cleanup ==="
df -h /
BEFORE_KIB="$(available_kib)"

# Resolve a directory through any symlinks, printing nothing if it does not exist. `readlink -f` is
# GNU-only and the BSD readlink on this runner has no -f, so the shell does it.
resolve_dir() {
    [ -d "$1" ] || return 1
    (cd -- "$1" 2>/dev/null && pwd -P)
}

# Simulator runtimes are the biggest single win — several GB per iOS/watchOS/tvOS version, none of
# which a terminal build touches. Devices go first: deleting the runtimes out from under a running
# CoreSimulatorService is what makes `simctl` hang rather than answer.
xcrun simctl delete all >/dev/null 2>&1
sudo rm -rf /Library/Developer/CoreSimulator/Profiles/Runtimes
sudo rm -rf "$HOME/Library/Developer/CoreSimulator"
sudo rm -rf "$HOME/Library/Developer/Xcode/iOS DeviceSupport"

# Every Xcode except the selected one, compared by RESOLVED path. The image ships
# `/Applications/Xcode.app` as a symlink to a versioned bundle, and either of the two may be what
# `xcode-select` points at — comparing the strings would keep the link and delete its target (or
# the reverse), which destroys the SDK this cleanup exists to make room for.
#
# A developer dir that is not an Xcode bundle at all (Command Line Tools, or a path this parser
# does not recognise) is the give-up case: keep every Xcode rather than guess which one is load
# bearing.
ACTIVE_DEVELOPER_DIR="$(xcode-select -p 2>/dev/null)"
case "$ACTIVE_DEVELOPER_DIR" in
*/Contents/Developer)
    ACTIVE_XCODE="$(resolve_dir "${ACTIVE_DEVELOPER_DIR%/Contents/Developer}")"
    ;;
*)
    ACTIVE_XCODE=""
    ;;
esac

if [ -n "$ACTIVE_XCODE" ]; then
    echo "keeping the selected toolchain: $ACTIVE_XCODE"
    for app in /Applications/Xcode*.app; do
        [ -e "$app" ] || continue
        app_real="$(resolve_dir "$app")" || continue
        [ "$app_real" = "$ACTIVE_XCODE" ] && continue
        if sudo rm -rf "$app"; then
            echo "removed $app"
        else
            echo "could not remove $app; continuing" >&2
        fi
    done
else
    echo "no Xcode bundle is selected ($ACTIVE_DEVELOPER_DIR); leaving every Xcode in place" >&2
fi

# Toolchains for platforms this repo does not target. Kept narrow on purpose: the wins above are
# the ones measured in tens of gigabytes, and every extra removal is another way to break a step
# that has nothing to do with disk.
sudo rm -rf "$HOME/Library/Android"
sudo rm -rf /usr/local/share/dotnet

echo "=== disk after cleanup ==="
df -h /
AFTER_KIB="$(available_kib)"

# Say what the cleanup was worth. Freeing nothing is not fatal on its own — the build may still
# fit — but it must not be silent: without this line a give-up path (Command Line Tools selected,
# bundles moved by a new image) reads exactly like a successful sweep, and the ENOSPC this script
# exists to prevent comes back forty minutes later looking unrelated.
FREED_MIB=$(((AFTER_KIB - BEFORE_KIB) / 1024))
echo "reclaimed ${FREED_MIB} MiB, $((AFTER_KIB / 1024 / 1024)) GiB now free"
if [ "$FREED_MIB" -lt 1024 ]; then
    echo "::warning::cleanup freed under 1 GiB; the image layout probably changed. If this build fails with 'No space left on device', start here."
fi

# The gate. Everything above is best-effort, so prove the one thing the build cannot do without:
# a macOS SDK the selected toolchain can still name.
if ! SDK_PATH="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null)" || [ ! -d "$SDK_PATH" ]; then
    echo "[FAIL] cleanup left no usable macOS SDK; xcrun reports:" >&2
    xcrun --sdk macosx --show-sdk-path >&2 2>&1
    exit 1
fi
echo "macOS SDK still resolves: $SDK_PATH"
