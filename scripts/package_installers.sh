#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

CARGO_BIN="${CARGO:-${HOME}/.cargo/bin/cargo}"
if [[ ! -x "$CARGO_BIN" ]]; then
    CARGO_BIN="$(command -v cargo)"
fi

WINDRES_BIN="${WINDRES:-/opt/homebrew/bin/x86_64-w64-mingw32-windres}"
if [[ ! -x "$WINDRES_BIN" ]]; then
    WINDRES_BIN="$(command -v x86_64-w64-mingw32-windres)"
fi

MINGW_GCC_BIN="${MINGW_GCC:-/opt/homebrew/bin/x86_64-w64-mingw32-gcc}"
if [[ ! -x "$MINGW_GCC_BIN" ]]; then
    MINGW_GCC_BIN="$(command -v x86_64-w64-mingw32-gcc)"
fi

PKGBUILD_BIN="/usr/bin/pkgbuild"

VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$ROOT_DIR/Cargo.toml")"
VERSION="${VERSION:-0.1.0}"

DIST_DIR="$ROOT_DIR/dist"
STAGING_DIR="$ROOT_DIR/target/pkg-staging"
MAC_ROOT="$STAGING_DIR/root"
MAC_SCRIPTS="$STAGING_DIR/scripts"
APP_BUNDLE="$MAC_ROOT/SquadAir.app"
DIST_APP="$DIST_DIR/SquadAir.app"
MAC_PKG="$DIST_DIR/SquadAir-Installer-macos-arm64.pkg"
WIN_SETUP="$DIST_DIR/SquadAir-Setup-windows-x86_64.exe"

echo "==> [1/4] Building macOS release binary (v${VERSION})..."
"$CARGO_BIN" build --release --locked -p squad-client

echo "==> [2/4] Building Windows x86_64 release binary (v${VERSION})..."
"$CARGO_BIN" build --release --locked --target x86_64-pc-windows-gnu -p squad-client

rm -rf "$STAGING_DIR"
mkdir -p "$DIST_DIR" "$APP_BUNDLE/Contents/MacOS" "$APP_BUNDLE/Contents/Resources/assets/sounds" "$MAC_SCRIPTS"

echo "==> [3/4] Packaging macOS installer (${MAC_PKG})..."
cp "$ROOT_DIR/target/release/squad-client" "$APP_BUNDLE/Contents/MacOS/squad-client"
chmod 755 "$APP_BUNDLE/Contents/MacOS/squad-client"
cp "$ROOT_DIR/assets/icons/AppIcon.icns" "$APP_BUNDLE/Contents/Resources/AppIcon.icns"
cp "$ROOT_DIR"/assets/sounds/*.wav "$APP_BUNDLE/Contents/Resources/assets/sounds/" 2>/dev/null || true

cat > "$APP_BUNDLE/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>SquadAir</string>
	<key>CFBundleDisplayName</key>
	<string>SquadAir</string>
	<key>CFBundleIdentifier</key>
	<string>com.squadair.client</string>
	<key>CFBundleExecutable</key>
	<string>squad-client</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleVersion</key>
	<string>${VERSION}</string>
	<key>CFBundleShortVersionString</key>
	<string>${VERSION}</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSMicrophoneUsageDescription</key>
	<string>SquadAir needs microphone access for encrypted squad voice chat.</string>
</dict>
</plist>
EOF

cat > "$STAGING_DIR/component.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
	<dict>
		<key>BundleHasStrictIdentifier</key>
		<true/>
		<key>BundleIsRelocatable</key>
		<false/>
		<key>BundleIsVersionChecked</key>
		<false/>
		<key>BundleOverwriteAction</key>
		<string>upgrade</string>
		<key>RootRelativeBundlePath</key>
		<string>SquadAir.app</string>
	</dict>
</array>
</plist>
EOF

cat > "$MAC_SCRIPTS/preinstall" <<'EOF'
#!/bin/sh
/usr/bin/killall -9 squad-client 2>/dev/null || true
exit 0
EOF
chmod 755 "$MAC_SCRIPTS/preinstall"

cat > "$MAC_SCRIPTS/postinstall" <<'EOF'
#!/bin/sh
if [ -d "/Applications/SquadAir.app" ]; then
    /usr/bin/xattr -cr "/Applications/SquadAir.app" 2>/dev/null || true
    /bin/chmod 755 "/Applications/SquadAir.app/Contents/MacOS/squad-client" 2>/dev/null || true
fi
exit 0
EOF
chmod 755 "$MAC_SCRIPTS/postinstall"

rm -f "$MAC_PKG"
"$PKGBUILD_BIN" \
    --root "$MAC_ROOT" \
    --component-plist "$STAGING_DIR/component.plist" \
    --scripts "$MAC_SCRIPTS" \
    --identifier com.squadair.client \
    --version "$VERSION" \
    --install-location /Applications \
    "$MAC_PKG"

rm -rf "$DIST_APP"
cp -R "$APP_BUNDLE" "$DIST_APP"

echo "==> [4/4] Packaging Windows setup executable (${WIN_SETUP})..."
"$WINDRES_BIN" \
    -I "$ROOT_DIR/target/x86_64-pc-windows-gnu/release" \
    -I "$ROOT_DIR" \
    "$ROOT_DIR/scripts/windows_installer.rc" \
    -O coff \
    -o "$STAGING_DIR/win_payload.o"

rm -f "$WIN_SETUP"
"$MINGW_GCC_BIN" \
    -finput-charset=UTF-8 \
    -O2 \
    -municode \
    -mwindows \
    "$ROOT_DIR/scripts/windows_installer.c" \
    "$STAGING_DIR/win_payload.o" \
    -o "$WIN_SETUP" \
    -Wl,--nxcompat \
    -Wl,--dynamicbase \
    -Wl,--high-entropy-va \
    -lole32 \
    -luuid \
    -lshell32 \
    -luser32 \
    -lgdi32 \
    -ladvapi32 \
    -Wl,-Bstatic -lssp -Wl,-Bdynamic \
    -s

echo "==> Done! Generated installers:"
ls -lh "$MAC_PKG" "$WIN_SETUP"
