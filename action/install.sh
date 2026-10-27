#!/usr/bin/env bash
# Puts git-torrent and git-remote-torrent on PATH: a release whose checksum and
# build provenance are verified before it runs, or a build of the action's own
# checkout (`version: source`).
set -euo pipefail

fail() {
  echo "::error::$*"
  exit 1
}

version=${VERSION:-}
if [[ -z $version ]]; then
  if [[ $ACTION_REF =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    version=$ACTION_REF
  else
    fail "set \`version:\` to a git-remote-torrent release such as v0.4.0, or use the action at a release tag"
  fi
fi

if [[ $version == source ]]; then
  cd "$GITHUB_ACTION_PATH"
  cargo build --release --locked -p grt-cli -p grt-helper
  echo "$GITHUB_ACTION_PATH/target/release" >> "$GITHUB_PATH"
  exit 0
fi

case "$RUNNER_OS-$RUNNER_ARCH" in
  Linux-X64) target=x86_64-unknown-linux-musl ;;
  Linux-ARM64) target=aarch64-unknown-linux-musl ;;
  macOS-X64) target=x86_64-apple-darwin ;;
  macOS-ARM64) target=aarch64-apple-darwin ;;
  *) fail "no git-remote-torrent release for $RUNNER_OS on $RUNNER_ARCH; try \`version: source\`" ;;
esac

dir="$RUNNER_TEMP/git-remote-torrent"
asset="git-remote-torrent-$version-$target"
rm -rf "$dir"
mkdir -p "$dir"
gh release download "$version" --repo "$SOURCE_REPOSITORY" \
  --pattern "$asset.tar.gz" --pattern SHA256SUMS --dir "$dir"

cd "$dir"
# The asset must be the one the release's checksums list...
line=$(grep -F "  $asset.tar.gz" SHA256SUMS) || fail "$asset.tar.gz is not in SHA256SUMS"
if command -v sha256sum > /dev/null; then
  sha256sum --check --strict <<< "$line"
else
  shasum --algorithm 256 --check <<< "$line"
fi
# ...and built by the release workflow from the source repository.
gh attestation verify "$asset.tar.gz" --repo "$SOURCE_REPOSITORY" \
  --signer-workflow "$SOURCE_REPOSITORY/.github/workflows/release.yml"

tar xzf "$asset.tar.gz"
echo "$dir/$asset" >> "$GITHUB_PATH"
echo "Verified and installed git-remote-torrent $version ($target)."
