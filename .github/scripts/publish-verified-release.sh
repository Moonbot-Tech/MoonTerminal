#!/usr/bin/env bash
# Publish a verified draft only while its remote tag is still bound to the built commit.
set -euo pipefail

release_tag="${1:?release tag is required}"
release_commit="${2:?release commit is required}"
windows_asset="${3:?local Windows asset is required}"
tag_ref="refs/tags/$release_tag"

if [[ -z "${GH_TOKEN:-}" ]]; then
    echo "[FAIL] RELEASE_ADMIN_TOKEN is required to verify release immutability" >&2
    exit 1
fi

remote_tag_commit() {
    git ls-remote origin "$tag_ref" "$tag_ref^{}" \
        | awk '$2 ~ /\^\{\}$/ { peeled=$1 } $2 !~ /\^\{\}$/ { direct=$1 } END { print peeled != "" ? peeled : direct }'
}

remote_commit="$(remote_tag_commit)"
if [[ -z "$remote_commit" || "$remote_commit" != "$release_commit" ]]; then
    echo "[FAIL] remote release tag moved after validation" >&2
    exit 1
fi

if ! gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}/immutable-releases" --silent; then
    echo "[FAIL] repository release immutability must be enabled before publishing" >&2
    exit 1
fi

# Close the ordinary workflow gap by re-verifying the exact local asset immediately in the
# publication step. Release immutability atomically locks that draft asset at publication.
bash .github/scripts/verify-release-assets.sh "$release_tag" "$windows_asset"

gh release edit "$release_tag" --draft=false --latest --verify-tag

release_json="$(gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}/releases/tags/$release_tag")"
if [[ "$(jq -r '.immutable' <<<"$release_json")" != "true" ]]; then
    echo "[FAIL] repository release immutability must be enabled before publishing" >&2
    exit 1
fi
bash .github/scripts/verify-release-assets.sh "$release_tag" "$windows_asset"
remote_commit="$(remote_tag_commit)"
if [[ "$remote_commit" != "$release_commit" ]]; then
    echo "[FAIL] immutable release tag does not match the built commit" >&2
    exit 1
fi

echo "[OK] published immutable $release_tag at $release_commit"
