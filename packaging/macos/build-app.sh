#!/bin/sh
# RDM.app and its disk image, native on Apple Silicon (M1 and every later chip), on a Mac:
#   sh packaging/macos/build-app.sh [path of an arm64 rdm, default target/release/rdm: built if missing]
# Writes RDM-<version>-macos-arm64.dmg at the repository root.
#
# Signed ad hoc (what arm64 macOS requires of any program): without an Apple Developer certificate,
# Gatekeeper asks the user to allow the first start (see "Installation" in the README).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
exe=${1:-target/release/rdm}
if [ ! -x "$exe" ]; then
    MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release -p rdm
fi
# Never an Intel binary under an "arm64" name: it would run through Rosetta, not natively.
[ "$(lipo -archs "$exe")" = arm64 ] || { echo "build-app.sh: $exe is not an arm64 program" >&2; exit 1; }

version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n 1)
work=target/macos
app=$work/RDM.app
rm -rf "$work"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 755 "$exe" "$app/Contents/MacOS/rdm"
cp packaging/macos/rdm.icns "$app/Contents/Resources/rdm.icns"
sed "s/__VERSION__/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
# Safari's extension, when built (packaging/safari/build.sh): an extension must run in a sandbox,
# which its own signature says.
appex="target/safari/RDM Extension.appex"
if [ -d "$appex" ]; then
    mkdir -p "$app/Contents/PlugIns"
    cp -R "$appex" "$app/Contents/PlugIns/"
    codesign --force --timestamp=none --sign - --entitlements packaging/safari/extension.entitlements "$app/Contents/PlugIns/RDM Extension.appex"
fi
codesign --force --timestamp=none --sign - "$app"
codesign --verify --strict "$app"

# Opens on the application and a link to /Applications: the usual drag and drop.
ln -s /Applications "$work/Applications"
dmg=RDM-$version-macos-arm64.dmg
rm -f "$dmg"
# hdiutil sometimes finds its image busy for a moment (the system scanning it).
attempt=1
until hdiutil create -quiet -volname "RDM $version" -srcfolder "$work" -format UDZO -ov "$dmg"; do
    [ "$attempt" -lt 3 ] || exit 1
    attempt=$((attempt + 1))
    sleep 5
done
echo "macOS disk image: $dmg"
