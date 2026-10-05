#!/bin/sh
# Packages for the extension stores, which sign them: sh packaging/stores/build.sh [output dir, default target/stores]
# The Chrome Web Store refuses a manifest `key`; addons.mozilla.org refuses `update_url` in a listed add-on.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-"$root/target/stores"}
version=$(jq -r .version "$root/extension/manifest.json")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"
out=$(cd "$out" && pwd)

package() {
    flavour=$1 filter=$2 file=$3
    sh "$root/packaging/$flavour/build.sh" "$work/$flavour" >/dev/null
    jq "$filter" "$work/$flavour/manifest.json" >"$work/manifest.json"
    mv "$work/manifest.json" "$work/$flavour/manifest.json"
    rm -f "$out/$file"
    (cd "$work/$flavour" && zip -qrX "$out/$file" .)
}

package chromium 'del(.key, .browser_specific_settings)' "rdm-chromium-store-$version.zip"
package firefox 'del(.browser_specific_settings.gecko.update_url)' "rdm-firefox-store-$version.xpi"
echo "Store packages: $out/rdm-chromium-store-$version.zip, $out/rdm-firefox-store-$version.xpi"
