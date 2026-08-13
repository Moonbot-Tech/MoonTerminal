#!/usr/bin/env bash
# Verify that one published Windows release asset has GitHub's digest for the local bytes.
set -euo pipefail

max_u64="18446744073709551615"

# Match Rust's unsigned release-component range before verifying release metadata.
valid_u64_component() {
    local component="$1"
    [[ "$component" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
    (( ${#component} < ${#max_u64} )) && return 0
    (( ${#component} == ${#max_u64} )) || return 1
    [[ "$component" == "$max_u64" || "$component" < "$max_u64" ]]
}

release_tag="${1:?release tag is required}"
asset_path="${2:?local asset path is required}"
asset_name="MoonTerminal.exe"

if [[ ! "$release_tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "[FAIL] non-canonical release tag: $release_tag" >&2
    exit 1
fi
IFS='.' read -r release_major release_minor release_patch <<< "${release_tag#v}"
if ! valid_u64_component "$release_major" \
    || ! valid_u64_component "$release_minor" \
    || ! valid_u64_component "$release_patch"; then
    echo "[FAIL] release tag exceeds the supported numeric range: $release_tag" >&2
    exit 1
fi
if [[ ! -f "$asset_path" ]]; then
    echo "[FAIL] local Windows asset is missing: $asset_path" >&2
    exit 1
fi

release_json="$(gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}/releases/tags/$release_tag")"
asset_count="$(jq --arg name "$asset_name" '[.assets[] | select(.name == $name)] | length' <<<"$release_json")"
if [[ "$asset_count" != "1" ]]; then
    echo "[FAIL] release must contain exactly one $asset_name asset" >&2
    exit 1
fi

remote_digest="$(jq -r --arg name "$asset_name" '.assets[] | select(.name == $name) | .digest // empty' <<<"$release_json")"
remote_size="$(jq -r --arg name "$asset_name" '.assets[] | select(.name == $name) | .size' <<<"$release_json")"
if [[ ! "$remote_digest" =~ ^sha256:[0-9a-fA-F]{64}$ ]]; then
    echo "[FAIL] GitHub did not publish a valid SHA-256 digest for $asset_name" >&2
    exit 1
fi

local_digest="sha256:$(sha256sum "$asset_path" | awk '{print $1}')"
local_size="$(wc -c < "$asset_path" | tr -d '[:space:]')"
if [[ "${remote_digest,,}" != "$local_digest" ]]; then
    echo "[FAIL] published digest does not match the local Windows asset" >&2
    exit 1
fi
if [[ "$remote_size" != "$local_size" ]]; then
    echo "[FAIL] published size does not match the local Windows asset" >&2
    exit 1
fi

echo "[OK] verified $release_tag $asset_name ($local_size bytes, $local_digest)"
