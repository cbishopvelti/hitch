#!/bin/sh
# Builds target/release/Hitch.app and target/release/Hitch-macos.zip.
#
# The app is signed ad hoc, not with a Developer ID, so it isn't notarised:
# macOS asks before opening it the first time, and the Accessibility
# permission has to be switched on again after each new build.
set -eu

cd "$(dirname "$0")/../.."

cargo build --release --locked -p hitch-app -p hitch-web

version=$(cargo pkgid -p hitch-app | sed 's/.*[#@]//')
app=target/release/Hitch.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/hitch "$app/Contents/MacOS/hitch"
# The branch page window, found next to hitch.
cp target/release/hitch-web "$app/Contents/MacOS/hitch-web"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

codesign --force --sign - --identifier io.github.cbishopvelti.Hitch "$app"
codesign --verify --verbose "$app"

# ditto keeps the executable bit and signature, which a plain zip may not.
rm -f target/release/Hitch-macos.zip
ditto -c -k --keepParent "$app" target/release/Hitch-macos.zip
echo "Built $app (version $version)"
