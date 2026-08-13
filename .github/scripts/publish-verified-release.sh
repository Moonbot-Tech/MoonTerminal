#!/usr/bin/env bash
# Publish the exact verified draft while its remote tag still names the built commit.
set -euo pipefail

release_tag="${1:?release tag is required}"
release_commit="${2:?release commit is required}"
windows_asset="${3:?local Windows asset is required}"
repository_api="repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
tag_ref="refs/tags/$release_tag"

if [[ -z "${GH_TOKEN:-}" ]]; then
    echo "[FAIL] RELEASE_ADMIN_TOKEN is required to verify release immutability" >&2
    exit 1
fi

# Resolve annotated and lightweight remote tags to the commit they identify.
remote_tag_commit() {
    git ls-remote origin "$tag_ref" "$tag_ref^{}" \
        | awk '$2 ~ /\^\{\}$/ { peeled=$1 } $2 !~ /\^\{\}$/ { direct=$1 } END { print peeled != "" ? peeled : direct }'
}

# Require the remote tag to remain bound to the exact build at each publication boundary.
require_remote_tag_commit() {
    local failure_message="$1"
    local remote_commit
    remote_commit="$(remote_tag_commit)"
    if [[ -z "$remote_commit" || "$remote_commit" != "$release_commit" ]]; then
        echo "[FAIL] $failure_message" >&2
        exit 1
    fi
}

require_remote_tag_commit "remote release tag moved after validation"

if ! gh api "${repository_api}/immutable-releases" --silent; then
    echo "[FAIL] repository release immutability must be enabled before publishing" >&2
    exit 1
fi

# Bind verification and publication to one numeric id; a tag is not a draft lookup authority.
release_id="$(bash .github/scripts/verify-release-assets.sh "$release_tag" "$windows_asset")"
require_remote_tag_commit "remote release tag moved before publication"
gh api "${repository_api}/releases/${release_id}" \
    --method PATCH \
    -F draft=false \
    -F prerelease=false \
    -f make_latest=true \
    --silent

bash .github/scripts/verify-release-assets.sh \
    "$release_tag" "$windows_asset" published "$release_id" >/dev/null
require_remote_tag_commit "immutable release tag does not match the built commit"

echo "[OK] published immutable $release_tag at $release_commit as release $release_id"
