#!/bin/sh
# Maakt van een gebouwde binary een macOS-appbundel en zipt die.
# Gebruik: scripts/bundle-macos.sh <pad-naar-binary> <versie> <uitvoer.zip>
set -eu

BINARY="$1"
VERSION="$2"
OUTPUT="$3"

WORK="$(mktemp -d)"
APP="$WORK/Perkament.app"
mkdir -p "$APP/Contents/MacOS"
cp "$BINARY" "$APP/Contents/MacOS/perkament"
chmod +x "$APP/Contents/MacOS/perkament"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Perkament</string>
    <key>CFBundleDisplayName</key><string>Perkament</string>
    <key>CFBundleIdentifier</key><string>io.github.patrickudo.perkament</string>
    <key>CFBundleExecutable</key><string>perkament</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
</dict>
</plist>
PLIST

# Ad-hoc ondertekenen: zonder handtekening start een arm64-binary niet.
codesign --force --deep --sign - "$APP"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$OUTPUT"
rm -rf "$WORK"
echo "$OUTPUT"
