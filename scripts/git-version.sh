#!/bin/sh
# Derives PhotoCraft's version from the nearest `v*` tag:
#
#   git describe --long --tag --abbrev=7 --match="v*" HEAD     ->  v0.90.1-0-g1a2b3c4
#
# scripts/git-version.sh            print the version: 0.90.1 on the tag, 0.90.1+3.g1a2b3c4 after it
# scripts/git-version.sh --write    also set [workspace.package] version in Cargo.toml to 0.90.1
#                                   (the numbers of the tag; Cargo.lock follows on the next build)
set -eu
write=${1:-}
cd "$(dirname "$0")/.."

describe=$(git describe --long --tag --abbrev=7 --match="v*" HEAD)
# v<major>.<minor>.<patch>-<commits since the tag>-g<hash>
parts=$(printf '%s\n' "$describe" | sed -n -E 's/^v([0-9]+\.[0-9]+\.[0-9]+)-([0-9]+)-g([0-9a-f]{7,})$/\1 \2 \3/p')
if [ -z "$parts" ]; then
    echo "git-version: cannot parse '$describe' (tags must look like v1.2.3)" >&2
    exit 1
fi
set -- $parts
base=$1; ahead=$2; hash=$3

if [ "$ahead" -eq 0 ]; then
    full=$base
else
    full="$base+$ahead.g$hash"
fi

if [ "$write" = "--write" ] || [ "${GIT_VERSION_WRITE:-}" = 1 ]; then
    sed -i -E '/^\[workspace\.package\]/,/^\[/ s/^version = "[^"]*"/version = "'"$base"'"/' Cargo.toml
fi
printf '%s\n' "$full"
