#!/usr/bin/env bash
# Adds a swap file before the Linux terminal's release build, in linux-probe.yml and release.yml
# alike: the probe exists to prove exactly what the release ships, so both call this one copy.
#
# Why: the release profile links the terminal with fat LTO in one codegen unit, and that final
# rustc needs 17.2-17.4 GB (resident plus swapped, measured 2026-11-16 in the probe's own
# ubuntu:22.04 container). A hosted `ubuntu-latest` runner has 15.7-16.0 GB of RAM and either a
# 3 GB swap file or none at all, which one is luck of the draw: without swap the kernel's OOM
# killer ends the link with SIGKILL (cgroup `oom_kill 1`), with 3 GB it squeaks through with
# about 0.4 GB to spare. With eight more GB and no memory cap, 8 builds of 10 were green and 2
# lost their runner: the whole VM ran short of RAM and the runner agent stopped answering. With
# the container capped at 13 GB (`--memory=13g --memory-swap=-1`), the link pages inside its own
# cgroup and 5 of 5 were green.
#
# Usage: add-build-swap.sh <size in GiB>
# The job's container must be started with `--cap-add=SYS_ADMIN --security-opt
# seccomp=unconfined`: swapon is a privileged call and Docker's default seccomp profile denies it.
# Fails loudly rather than building without the swap: on a runner with none, that build is the
# coin toss this script exists to remove.
set -euo pipefail

size_gib="${1:?usage: add-build-swap.sh <size in GiB>}"
# RUNNER_TEMP is bind-mounted from the runner's ext4 disk, which swapon accepts; the container's
# own overlay root is not a valid place for a swap file.
swapfile="${RUNNER_TEMP:?RUNNER_TEMP is not set}/build-swapfile"

# /proc/meminfo rather than `free`: the bare image ships no procps, and the package list is
# pinned to the build guide.
echo "memory before:"
grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo

if ! fallocate -l "${size_gib}G" "$swapfile"; then
    # fallocate is refused on some filesystems; a written file always works, only slower.
    dd if=/dev/zero of="$swapfile" bs=1M count=$((size_gib * 1024))
fi
chmod 600 "$swapfile"
mkswap "$swapfile" > /dev/null
if ! swapon "$swapfile"; then
    echo "swapon failed: the job's container needs --cap-add=SYS_ADMIN and" \
        "--security-opt seccomp=unconfined (see this script's header)" >&2
    exit 1
fi

echo "memory after adding ${size_gib} GiB of swap:"
grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo
cat /proc/swaps
