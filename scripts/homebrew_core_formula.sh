#!/bin/sh
# Print the formula for submitting mmore to homebrew-core, for a released
# VERSION (such as 0.5.0). The source archive is downloaded to compute its
# checksum. A different archive URL can be given, for testing.
set -eu
version=${1:?usage: $0 VERSION [ARCHIVE_URL]}
url=${2:-https://github.com/houseofmackee/mmore/archive/refs/tags/v$version.tar.gz}
sha=$(curl -fsSL "$url" | shasum -a 256 | cut -d ' ' -f 1)
template="$(dirname "$0")/../packaging/homebrew-core/mmore.rb.in"
sed -e "s|@URL@|$url|" -e "s|@SHA256@|$sha|" "$template"
