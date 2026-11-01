#!/usr/bin/env bash
# Publishes a draft release, but only once its notes record a smoke run:
# refuses while the body still says UNVERIFIED, or while GitHub's immutable
# releases setting is off (a release published then stays mutable).
# Usage: scripts/publish-release.sh vX.Y.Z
set -euo pipefail
tag="${1:?usage: scripts/publish-release.sh vX.Y.Z}"
repo=jfms7s/opendeck-utilities
body="$(gh release view "$tag" -R "$repo" --json body --jq .body)"
if grep -q "UNVERIFIED" <<<"$body"; then
	echo "refusing: $tag's notes still say UNVERIFIED; record the smoke run first" >&2
	exit 1
fi
if ! gh api "repos/$repo/immutable-releases" --jq .enabled | grep -qx true; then
	echo "refusing: immutable releases are off for $repo; turn them on first" >&2
	exit 1
fi
gh release edit "$tag" -R "$repo" --draft=false --latest
