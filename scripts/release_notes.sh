#!/bin/sh
# Prints the release notes for a version: its section of CHANGELOG.md, then how
# to install it. Fails if CHANGELOG.md has no section for the version.
#
#   scripts/release_notes.sh 0.2.0
set -eu

version="$1"
changelog="$(dirname "$0")/../CHANGELOG.md"

section=$(awk -v version="$version" '
    index($0, "## [" version "]") == 1 { found = 1; next }
    found && (/^## \[/ || /^\[[^]]+\]: /) { exit }
    found {
        lines[++n] = $0
        if ($0 ~ /[^[:space:]]/) { if (!first) first = n; last = n }
    }
    END { for (i = first; i && i <= last; i++) print lines[i] }
' "$changelog")

if [ -z "$section" ]; then
    echo "CHANGELOG.md has no section for $version" >&2
    exit 1
fi

printf '%s\n\n' "$section"
cat <<'INSTALL'
### Install

`brew install houseofmackee/tap/mmore` (macOS and Linux), or download the archive
for your platform below, check it against `SHA256SUMS.txt`, and put `mmore` on
your `PATH`. The Linux builds are statically linked and run on any distribution.
INSTALL
