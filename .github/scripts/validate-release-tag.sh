#!/usr/bin/env bash
# Bind a canonical stable release tag to the checked-out immutable commit on main.
set -euo pipefail

max_u64="18446744073709551615"

# Match Rust's unsigned release-component range before a tag can be published.
valid_u64_component() {
    local component="$1"
    [[ "$component" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
    (( ${#component} < ${#max_u64} )) && return 0
    (( ${#component} == ${#max_u64} )) || return 1
    [[ "$component" == "$max_u64" || "$component" < "$max_u64" ]]
}

# Accept historical two-component tags only when inspecting repository history.
valid_stable_tag() {
    local tag="$1" major minor patch extra
    [[ "$tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))?$ ]] || return 1
    IFS='.' read -r major minor patch extra <<< "${tag#v}"
    [[ -z "${extra:-}" ]] || return 1
    valid_u64_component "$major" \
        && valid_u64_component "$minor" \
        && valid_u64_component "${patch:-0}"
}

release_tag="${1:?release tag is required}"
if [[ ! "$release_tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] \
    || ! valid_stable_tag "$release_tag"; then
    echo "[FAIL] release tag must use canonical form vMAJOR.MINOR.PATCH" >&2
    exit 1
fi

tag_ref="refs/tags/$release_tag"
if ! git show-ref --verify --quiet "$tag_ref"; then
    echo "[FAIL] requested release tag does not exist" >&2
    exit 1
fi

tag_commit="$(git rev-list -n 1 "$tag_ref")"
head_commit="$(git rev-parse HEAD)"
if [[ "$tag_commit" != "$head_commit" ]]; then
    echo "[FAIL] checkout does not match the requested release tag" >&2
    exit 1
fi
if ! git merge-base --is-ancestor "$head_commit" origin/main; then
    echo "[FAIL] release tag must point to a commit on main" >&2
    exit 1
fi

accepted_tags=""
while IFS= read -r existing_tag; do
    if valid_stable_tag "$existing_tag"; then
        accepted_tags+="${accepted_tags:+$'\n'}${existing_tag}"
    fi
done < <(git tag --merged origin/main --sort=-version:refname --list 'v*')
while IFS= read -r existing_tag; do
    [[ -z "$existing_tag" || "$existing_tag" == "$release_tag" ]] && continue
    normalized_tag="$existing_tag"
    if [[ "$existing_tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
        normalized_tag="${existing_tag}.0"
    fi
    if [[ "$normalized_tag" == "$release_tag" ]]; then
        echo "[FAIL] release tag aliases an existing stable version: $existing_tag" >&2
        exit 1
    fi
done <<< "$accepted_tags"

latest_tag="$(head -n 1 <<< "$accepted_tags")"
if [[ "$release_tag" != "$latest_tag" ]]; then
    echo "[FAIL] only the greatest canonical stable tag may be published" >&2
    exit 1
fi

echo "tag=$release_tag" >> "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
echo "commit=$head_commit" >> "$GITHUB_OUTPUT"
echo "[OK] validated $release_tag at $head_commit"
