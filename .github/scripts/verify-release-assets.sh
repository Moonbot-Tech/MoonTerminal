#!/usr/bin/env bash
# Verify one exact draft or published release and print its numeric GitHub release id.
set -euo pipefail

max_u64="18446744073709551615"
max_release_pages=10
releases_per_page=100

# Match Rust's unsigned release-component range before verifying release metadata.
valid_u64_component() {
    local component="$1"
    [[ "$component" =~ ^(0|[1-9][0-9]*)$ ]] || return 1
    (( ${#component} < ${#max_u64} )) && return 0
    (( ${#component} == ${#max_u64} )) || return 1
    [[ "$component" == "$max_u64" || "$component" < "$max_u64" ]]
}

# Stop release publication with one ASCII diagnostic.
fail() {
    echo "[FAIL] $1" >&2
    exit 1
}

# Find exactly one draft by scanning a bounded authenticated release list.
find_draft_release() {
    local page page_json page_record page_size page_match_count page_match_id
    local match_count=0
    local match_id=""

    for ((page = 1; page <= max_release_pages; page++)); do
        if ! page_json="$(gh api "${repository_api}/releases?per_page=${releases_per_page}&page=${page}")"; then
            fail "failed to read GitHub release-list metadata"
        fi
        if ! page_record="$(
            jq -er --arg tag "$release_tag" '
                if type == "array" then
                    [.[] | select(.tag_name == $tag)] as $matches
                    | [length, ($matches | length), ($matches[0].id // "")]
                    | @tsv
                else
                    error("release list must be an array")
                end
            ' <<<"$page_json"
        )"; then
            fail "GitHub returned malformed release-list metadata"
        fi
        IFS=$'\t' read -r page_size page_match_count page_match_id <<<"$page_record" \
            || fail "GitHub returned malformed release-list metadata"
        if [[ ! "$page_size" =~ ^(0|[1-9][0-9]*)$ \
            || ! "$page_match_count" =~ ^(0|[1-9][0-9]*)$ \
            || "$page_match_count" -gt "$page_size" ]]; then
            fail "GitHub returned malformed release-list metadata"
        fi
        if (( page_size > releases_per_page )); then
            fail "GitHub returned an oversized release-list page"
        fi
        if (( page_match_count == 1 )); then
            [[ "$page_match_id" =~ ^[1-9][0-9]*$ ]] \
                || fail "GitHub release is missing a numeric id"
            if (( match_count == 0 )); then
                match_id="$page_match_id"
            fi
        fi
        match_count=$((match_count + page_match_count))
        if (( page_size < releases_per_page )); then
            break
        fi
        if (( page == max_release_pages )); then
            fail "release-list pagination exceeded the safety bound"
        fi
    done

    if [[ "$match_count" != "1" ]]; then
        fail "release list must contain exactly one release tagged $release_tag"
    fi
    printf '%s\n' "$match_id"
}

# Read one release by its already-verified numeric id.
release_by_id() {
    local release_id="$1"
    [[ "$release_id" =~ ^[1-9][0-9]*$ ]] || fail "invalid numeric release id"
    gh api "${repository_api}/releases/${release_id}"
}

# Verify release identity, lifecycle state, exact assets, and the local Windows bytes.
verify_release() {
    local release_json="$1"
    local expected_state="$2"
    local expected_release_id="${3:-}"
    local actual_id actual_tag actual_target actual_draft actual_prerelease actual_immutable
    local asset_names remote_digest remote_size tag_json tag_release_id

    jq -e 'type == "object" and (.assets | type == "array")' <<<"$release_json" >/dev/null \
        || fail "GitHub returned malformed release metadata"
    actual_id="$(jq -r '.id // empty' <<<"$release_json")"
    actual_tag="$(jq -r '.tag_name // empty' <<<"$release_json")"
    actual_target="$(jq -r '.target_commitish // empty' <<<"$release_json")"
    actual_draft="$(jq -r '.draft' <<<"$release_json")"
    actual_prerelease="$(jq -r '.prerelease' <<<"$release_json")"
    actual_immutable="$(jq -r '.immutable' <<<"$release_json")"

    [[ "$actual_id" =~ ^[1-9][0-9]*$ ]] || fail "GitHub release is missing a numeric id"
    [[ -z "$expected_release_id" || "$actual_id" == "$expected_release_id" ]] \
        || fail "GitHub returned a different release id"
    [[ "$actual_tag" == "$release_tag" ]] || fail "GitHub release tag does not match $release_tag"
    [[ "$actual_target" == "$expected_commit" ]] \
        || fail "GitHub release target does not match the tagged commit"
    [[ "$actual_prerelease" == "false" ]] || fail "GitHub release must not be a prerelease"

    case "$expected_state" in
        draft)
            [[ "$actual_draft" == "true" && "$actual_immutable" == "false" ]] \
                || fail "GitHub release must be a mutable draft before publication"
            ;;
        published)
            [[ "$actual_draft" == "false" && "$actual_immutable" == "true" ]] \
                || fail "GitHub release must be public and immutable after publication"
            ;;
        *)
            fail "unsupported release verification state: $expected_state"
            ;;
    esac

    asset_names="$(jq -c '[.assets[].name] | sort' <<<"$release_json")"
    if [[ "$asset_names" != '["MoonTerminal.dmg","MoonTerminal.exe"]' ]]; then
        fail "release must contain exactly MoonTerminal.dmg and MoonTerminal.exe"
    fi
    remote_digest="$(jq -r --arg name "$asset_name" '.assets[] | select(.name == $name) | .digest // empty' <<<"$release_json")"
    remote_size="$(jq -r --arg name "$asset_name" '.assets[] | select(.name == $name) | .size' <<<"$release_json")"
    [[ "$remote_digest" =~ ^sha256:[0-9a-fA-F]{64}$ ]] \
        || fail "GitHub did not publish a valid SHA-256 digest for $asset_name"
    [[ "${remote_digest,,}" == "$local_digest" ]] \
        || fail "published digest does not match the local Windows asset"
    [[ "$remote_size" == "$local_size" ]] \
        || fail "published size does not match the local Windows asset"

    if [[ "$expected_state" == "published" ]]; then
        tag_json="$(gh api "${repository_api}/releases/tags/$release_tag")"
        tag_release_id="$(jq -r '.id // empty' <<<"$tag_json")"
        [[ "$tag_release_id" == "$actual_id" ]] \
            || fail "published tag resolves to a different release id"
    fi

    printf '%s\n' "$actual_id"
}

release_tag="${1:?release tag is required}"
asset_path="${2:?local asset path is required}"
verification_state="${3:-draft}"
expected_release_id="${4:-}"
asset_name="MoonTerminal.exe"
repository_api="repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

if [[ ! "$release_tag" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    fail "non-canonical release tag: $release_tag"
fi
IFS='.' read -r release_major release_minor release_patch <<< "${release_tag#v}"
if ! valid_u64_component "$release_major" \
    || ! valid_u64_component "$release_minor" \
    || ! valid_u64_component "$release_patch"; then
    fail "release tag exceeds the supported numeric range: $release_tag"
fi
[[ -f "$asset_path" ]] || fail "local Windows asset is missing: $asset_path"
expected_commit="$(git rev-parse "${release_tag}^{commit}")"
local_digest="sha256:$(sha256sum "$asset_path" | awk '{print $1}')"
local_size="$(wc -c < "$asset_path" | tr -d '[:space:]')"

case "$verification_state" in
    draft)
        [[ -z "$expected_release_id" ]] || fail "draft mode does not accept a release id"
        if ! listed_release_id="$(find_draft_release)"; then
            exit 1
        fi
        release_json="$(release_by_id "$listed_release_id")"
        expected_release_id="$listed_release_id"
        ;;
    published)
        [[ -n "$expected_release_id" ]] || fail "published mode requires a release id"
        release_json="$(release_by_id "$expected_release_id")"
        ;;
    *)
        fail "unsupported release verification state: $verification_state"
        ;;
esac

verified_release_id="$(verify_release "$release_json" "$verification_state" "$expected_release_id")"
echo "[OK] verified $release_tag $asset_name ($local_size bytes, $local_digest) as release $verified_release_id" >&2
printf '%s\n' "$verified_release_id"
