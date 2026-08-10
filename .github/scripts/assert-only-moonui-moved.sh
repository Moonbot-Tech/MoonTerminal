#!/usr/bin/env bash
# Fail unless the per-run MoonUI refresh moved MoonUI and nothing else.
#
# `cargo update -p moon-gpui -p moon-gpui-platform -p moon-ui` is documented as CONSERVATIVE, not
# surgical: cargo may change the selected packages' transitive requirements when it has to. So a
# new MoonUI master whose manifests want a newer crates.io crate silently pulls that crate into the
# graph this run compiles — a third-party version nobody committed and the audit job never sees,
# which is exactly the surprise the committed lockfile exists to prevent.
#
# This is the enforcement, not a warning: MoonUI still rolls on its own for free in the ordinary
# case, and the moment it drags a third-party version along, the run goes red and a human has to
# refresh and commit the lock deliberately.
#
# Compares the committed lock (HEAD) against the working one by `name + version + source`, ignoring
# every package that COMES FROM the MoonUI repository, since those are what the refresh is for.
#
# Note the filter is by SOURCE, not by package name: `cargo update -p moon-gpui …` names three
# crates, but that one git checkout supplies about two dozen packages (moon-collections, moon-perf,
# moon-ui-components, …) and every one of their revisions advances together. Filtering by the three
# named crates would report the other twenty as drift on the first ordinary MoonUI advance.
#
# Usage: assert-only-moonui-moved.sh
# Run from the repository root, AFTER the targeted `cargo update`. Requires the committed lock to be
# reachable in git (it reads `git show HEAD:Cargo.lock`).

set -euo pipefail

# The one git repository whose revision is allowed to move without a commit. Compared as a WHOLE
# URL after the `?branch=…` query and `#<rev>` fragment are stripped, never as a substring: a
# substring test would also accept `…/MoonUI-plugins`, so a brand-new git dependency whose name
# merely starts the same way would be filtered out of the comparison and reach the binary
# unaudited — the precise thing this script exists to catch.
MOONUI_SOURCE='git+https://github.com/Moonbot-Tech/MoonUI'

# One line per package as `name|version|source`, minus every package sourced from MoonUI.
#
# The SET of packages is the subject, deliberately, and not each block's `dependencies` edges.
# Measured on this repository rather than assumed: a targeted MoonUI refresh reports "Locking 0
# packages" and still re-unifies about fourteen `windows-sys` edges between 0.60.2 and 0.61.2 —
# and it OSCILLATES, flipping them back on the next run. Comparing edges would therefore redden
# CI on alternate runs with no dependency having changed at all, which is how a gate gets
# switched off.
#
# Excluding them costs nothing this gate exists to protect. Both endpoints of such a
# re-unification are packages already present in the committed lock, already scanned by the audit
# job, already compiled. What the gate must catch is third-party code that is NEW — a package
# added, or a version bumped — and that always changes this set.
#
# A workspace member legitimately has no `source` line; the empty field is kept, so a member
# appearing or vanishing is a difference too.
identities() {
    awk -v moonui="$MOONUI_SOURCE" '
        function repo_of(source,   bare) {
            bare = source
            sub(/[?#].*$/, "", bare)
            return bare
        }
        function emit() {
            if (name != "" && repo_of(source) != moonui)
                print name "|" version "|" source
            name = ""; version = ""; source = ""
        }
        /^name = /    { gsub(/"/, "", $3); name = $3;    next }
        /^version = / { gsub(/"/, "", $3); version = $3; next }
        /^source = /  { gsub(/"/, "", $3); source = $3;  next }
        /^$/          { emit() }
        END           { emit() }
    ' "$1" | sort
}

committed=$(mktemp)
current=$(mktemp)
trap 'rm -f "$committed" "$current"' EXIT

git show HEAD:Cargo.lock > "$committed"
identities "$committed" > "$committed.ids"
identities Cargo.lock > "$current.ids"

if diff -u "$committed.ids" "$current.ids" > /tmp/lock-drift.diff; then
    echo "[OK] the MoonUI refresh moved MoonUI only; every other dependency is as committed"
    exit 0
fi

echo "[FAIL] refreshing MoonUI moved dependencies other than MoonUI itself:"
echo
cat /tmp/lock-drift.diff
echo
echo "The committed Cargo.lock is the frozen set of third-party versions. A MoonUI commit that"
echo "needs different ones is a deliberate dependency change, not an automatic refresh."
echo
echo "To resolve, on your machine:"
echo "  cargo update -p moon-gpui -p moon-gpui-platform -p moon-ui"
echo "  git add Cargo.lock && git commit"
echo "and review the non-MoonUI lines of that diff before you push them."
exit 1
