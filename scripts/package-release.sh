#!/bin/sh
# Build one Linux release archive (docs/releasing.md, docs/install.md).
#
# Usage: scripts/package-release.sh <rust-target> <arch>
#   e.g. scripts/package-release.sh x86_64-unknown-linux-musl x86_64
#
# Expects web/ already built (`npm ci && npm run build`), so the release
# binary embeds the interface. Writes dist/consolebook-linux-<arch>.tar.gz
# and its .sha256; the archive holds one directory,
# consolebook-<version>-linux-<arch>/, with the executable, the systemd unit,
# the installer, and the license, readme, and changelog. The build identity
# is CONSOLEBOOK_BUILD_COMMIT when set, otherwise this checkout's HEAD.
# Exit: 0 written, 1 failure, 2 usage.

set -eu

me=scripts/package-release.sh
[ $# -eq 2 ] || {
	echo "usage: $me <rust-target> <arch>" >&2
	exit 2
}
target=$1
arch=$2

top=$(git rev-parse --show-toplevel)
cd "$top"

[ -f web/build/index.html ] || {
	echo "$me: build web/ first: (cd web && npm ci && npm run build)" >&2
	exit 1
}

CONSOLEBOOK_BUILD_COMMIT=${CONSOLEBOOK_BUILD_COMMIT:-$(git rev-parse --short=12 HEAD)}
export CONSOLEBOOK_BUILD_COMMIT
cargo build --release --locked --target "$target" -p consolebook-server

binary="target/$target/release/consolebook-server"
# `--version` prints "consolebook <version> (<commit>)".
version=$("$binary" --version | cut -d' ' -f2)
name="consolebook-$version-linux-$arch"
asset="consolebook-linux-$arch.tar.gz"

rm -rf "dist/$name" "dist/$asset" "dist/$asset.sha256"
mkdir -p "dist/$name"
install -m 0755 "$binary" "dist/$name/consolebook"
install -m 0644 install/consolebook.service LICENSE README.md CHANGELOG.md "dist/$name/"
install -m 0755 install/install.sh "dist/$name/"
tar -C dist -czf "dist/$asset" "$name"
(cd dist && sha256sum "$asset" >"$asset.sha256")

echo "$me: dist/$asset ($("$binary" --version))"
cat "dist/$asset.sha256"
