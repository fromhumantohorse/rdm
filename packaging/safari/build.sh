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
# 16.4: the first Safari with `storage.session`.
jq 'del(.key, .minimum_chrome_version)
    | .browser_specific_settings = {safari: {strict_min_version: "16.4"}}
    | .permissions -= ["downloads", "nativeMessaging"]' \
    "$root/extension/manifest.json" >"$work/extension/manifest.json"

xcrun safari-web-extension-converter "$work/extension" --project-location "$work/project" --app-name RDM \
    --bundle-identifier org.rdm.rdm --macos-only --swift --no-open --no-prompt --force --copy-resources
project=$(find "$work/project" -maxdepth 3 -name '*.xcodeproj' | head -n 1)
[ -n "$project" ] || { echo "build.sh: the converter made no Xcode project" >&2; exit 1; }
# Signed by build-app.sh, with the rest of RDM.app.
xcodebuild -quiet -project "$project" -alltargets -configuration Release \
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
