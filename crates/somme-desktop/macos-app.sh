#!/bin/sh
# Builds Somme.app so macOS shows the app name and icon, and lists Somme under
# "Open With" for text files and folders. Prints the app's path.
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
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Folder</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array><string>public.folder</string></array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Text</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array><string>public.text</string><string>public.json</string><string>public.xml</string></array>
        </dict>
        <dict>
            <!-- Kinds macOS has no name for, by extension. Keep in step with EXTENSIONS in src/open_with.rs. -->
            <key>CFBundleTypeName</key><string>Source</string>
            <key>CFBundleTypeRole</key><string>Editor</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>CFBundleTypeExtensions</key>
            <array>
            <string>txt</string><string>text</string><string>log</string><string>md</string><string>markdown</string><string>mdx</string><string>json</string><string>jsonc</string>
            <string>jsonl</string><string>ndjson</string><string>yaml</string><string>yml</string><string>toml</string><string>ini</string><string>cfg</string><string>conf</string>
            <string>env</string><string>properties</string><string>csv</string><string>tsv</string><string>xml</string><string>plist</string><string>html</string><string>htm</string>
            <string>css</string><string>scss</string><string>less</string><string>js</string><string>mjs</string><string>cjs</string><string>jsx</string><string>ts</string>
            <string>mts</string><string>cts</string><string>tsx</string><string>vue</string><string>svelte</string><string>py</string><string>pyi</string><string>rb</string>
            <string>php</string><string>pl</string><string>lua</string><string>go</string><string>rs</string><string>c</string><string>h</string><string>cpp</string>
            <string>cc</string><string>cxx</string><string>hpp</string><string>hh</string><string>cs</string><string>java</string><string>kt</string><string>kts</string>
            <string>swift</string><string>scala</string><string>dart</string><string>zig</string><string>sh</string><string>bash</string><string>zsh</string><string>fish</string>
            <string>ps1</string><string>bat</string><string>cmd</string><string>sql</string><string>graphql</string><string>gql</string><string>proto</string><string>tf</string>
            <string>hcl</string><string>nix</string><string>dockerfile</string><string>gitignore</string><string>editorconfig</string><string>diff</string><string>patch</string><string>lock</string>
            </array>
        </dict>
    </array>
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
