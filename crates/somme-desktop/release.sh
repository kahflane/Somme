#!/bin/sh
# Builds a release of Somme for this Mac and makes the files to upload to a
# GitHub release: the zipped app, SHA256SUMS and SHA256SUMS.sig. Prints the
# folder they are in.
#
# The signing key is the file SOMME_UPDATE_SIGNING_KEY names. Keep it out of
# the repo. Make one once with:
#   cargo run -p somme-desktop -- --update-keygen ~/somme-update.key
# and put the public key it prints in crates/somme-desktop/assets/update-key.pub.
#
# Usage: SOMME_UPDATE_SIGNING_KEY=~/somme-update.key crates/somme-desktop/release.sh
set -eu
cd "$(dirname "$0")/../.."

key="${SOMME_UPDATE_SIGNING_KEY:-}"
if [ -z "$key" ] || [ ! -f "$key" ]; then
    echo "Set SOMME_UPDATE_SIGNING_KEY to your private key file (see the top of this script)." >&2
    exit 1
fi
if ! grep -v '^[[:space:]]*#' crates/somme-desktop/assets/update-key.pub | grep -q '[^[:space:]]'; then
    echo "Put the public key in crates/somme-desktop/assets/update-key.pub first." >&2
    exit 1
fi

version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
# The machine the app is built for, named as Rust names it (aarch64 or x86_64).
arch=$(rustc -vV | sed -n 's/^host: //p' | cut -d- -f1)
app=$(crates/somme-desktop/macos-app.sh --release | tail -n 1)

out=target/release/dist
zip="Somme-$version-macos-$arch.zip"
mkdir -p "$out"
rm -f "$out/$zip" "$out/SHA256SUMS" "$out/SHA256SUMS.sig"
# Somme unpacks this with `ditto -x -k` and looks for Somme.app at the top.
ditto -c -k --keepParent "$app" "$out/$zip"
# Lists every Mac zip of this version in the folder, so a zip built on the
# other kind of Mac can be put there first and signed too.
(cd "$out" && shasum -a 256 Somme-"$version"-macos-*.zip > SHA256SUMS)
# Refuses a key that doesn't match the public key built into the app.
target/release/somme-desktop --update-sign "$key" "$out/SHA256SUMS"

echo "Upload these to a GitHub release tagged v$version:" >&2
ls -1 "$out" | grep -e "^Somme-$version-" -e '^SHA256SUMS' >&2
echo "$out"
