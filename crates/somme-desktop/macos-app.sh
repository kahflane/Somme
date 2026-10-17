#!/bin/sh
# Builds Somme.app so macOS shows the app name and icon, then prints its path.
# Usage: crates/somme-desktop/macos-app.sh [--release]
set -eu
cd "$(dirname "$0")/../.."

profile=debug
flags=""
if [ "${1:-}" = "--release" ]; then
    profile=release
    flags="--release"
fi
cargo build -p somme-desktop $flags

app="target/$profile/Somme.app"
version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "target/$profile/somme-desktop" "$app/Contents/MacOS/Somme"
cp crates/somme-desktop/assets/AppIcon.icns "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Somme</string>
    <key>CFBundleDisplayName</key><string>Somme</string>
    <key>CFBundleIdentifier</key><string>app.somme.desktop</string>
    <key>CFBundleExecutable</key><string>Somme</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
# Keychain "Always Allow" remembers the app by its signature. A quick (ad-hoc) signature
# changes on every build, so macOS asks again. A certificate keeps it the same.
identity="${SOMME_SIGN_IDENTITY:-Somme Dev}"
if security find-certificate -c "$identity" >/dev/null 2>&1; then
  codesign --force --sign "$identity" "$app" >/dev/null 2>&1 || codesign --force --sign - "$app" >/dev/null 2>&1 || true
else
  codesign --force --sign - "$app" >/dev/null 2>&1 || true
fi
echo "$app"
