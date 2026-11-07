#!/bin/sh
# Safari's flavour of the extension, on a Mac with Xcode: sh packaging/safari/build.sh [output dir, default target/safari]
#
# Safari only loads an extension as an app extension of an installed application, which only Xcode
# builds: Apple's converter makes a project of the extension's folder, and "RDM Extension.appex"
# lands in the output folder, for packaging/macos/build-app.sh to put in RDM.app.
#
# Safari has no `downloads` API (content.js takes over the clicks on links) and starts no connector
# of another program, so the extension talks to RDM's bridge itself.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-"$root/target/safari"}
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
. "$root/packaging/output-path.sh"
prepare_output
work="$out/work"
mkdir -p "$work"
cp -R "$root/extension" "$work/extension"
rm -rf "$work/extension/test"
# The converter says Safari does not take `background.type`, and a worker starting with `import`
# would not even load: a plain script here, with shared.js inside.
{ sed 's/^export //' "$root/extension/shared.js"; sed '/^import /d' "$root/extension/background.js"; } >"$work/extension/background.js"
rm "$work/extension/shared.js"
if command -v node >/dev/null; then node --check "$work/extension/background.js"; fi
# 16.4: the first Safari with `storage.session`.
jq 'del(.key, .minimum_chrome_version, .background.type)
    | .browser_specific_settings = {safari: {strict_min_version: "16.4"}}
    | .permissions -= ["downloads", "nativeMessaging"]' \
    "$root/extension/manifest.json" >"$work/extension/manifest.json"

xcrun safari-web-extension-converter "$work/extension" --project-location "$work/project" --app-name RDM \
    --bundle-identifier org.rdm.rdm --macos-only --swift --no-open --no-prompt --force --copy-resources
project=$(find "$work/project" -maxdepth 3 -name '*.xcodeproj' | head -n 1)
[ -n "$project" ] || { echo "build.sh: the converter made no Xcode project" >&2; exit 1; }
# The extension alone: RDM.app is ours, and Xcode refuses the application the converter writes around
# it (org.rdm.RDM, whatever --bundle-identifier says: not a prefix of org.rdm.rdm.Extension).
target=$(xcodebuild -list -project "$project" | sed -n 's/^ *\(.*Extension\)$/\1/p' | head -n 1)
[ -n "$target" ] || { echo "build.sh: no extension target in the Xcode project" >&2; exit 1; }
# Signed by build-app.sh, with the rest of RDM.app.
xcodebuild -quiet -project "$project" -target "$target" -configuration Release \
    SYMROOT="$work/build" ARCHS=arm64 MACOSX_DEPLOYMENT_TARGET=11.0 \
    MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$version" CODE_SIGNING_ALLOWED=NO

appex=$(find "$work/build" -type d -name '*.appex' | head -n 1)
[ -n "$appex" ] || { echo "build.sh: Xcode built no app extension" >&2; exit 1; }
cp -R "$appex" "$out/RDM Extension.appex"
# RDM opens Safari's settings on this identifier (`SAFARI_EXTENSION`, extension/browser.rs).
id=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$out/RDM Extension.appex/Contents/Info.plist")
[ "$id" = org.rdm.rdm.Extension ] || { echo "build.sh: unexpected extension identifier $id" >&2; exit 1; }
rm -rf "$work"
echo "Safari extension: $out/RDM Extension.appex"
