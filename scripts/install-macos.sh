#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
binary="$project_dir/target/release/wooting-host-profile"

if [ ! -x "$binary" ]; then
  command -v cargo >/dev/null 2>&1 || {
    echo "Rust/Cargo is required to build the macOS app." >&2
    exit 1
  }
  (cd "$project_dir" && cargo build --release)
fi

app_dir="$HOME/Applications/Wooting Host Profile.app"
contents="$app_dir/Contents"
macos_dir="$contents/MacOS"
resources_dir="$contents/Resources"

mkdir -p "$macos_dir" "$resources_dir"
cp "$binary" "$resources_dir/wooting-host-profile-agent"
chmod 755 "$resources_dir/wooting-host-profile-agent"
cp "$project_dir/macos/AppIcon.icns" "$resources_dir/AppIcon.icns"

xcrun swiftc \
  "$project_dir/macos/WootingHostProfileApp.swift" \
  -o "$macos_dir/WootingHostProfile" \
  -framework SwiftUI \
  -framework AppKit

cat > "$contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Wooting Host Profile</string>
  <key>CFBundleExecutable</key>
  <string>WootingHostProfile</string>
  <key>CFBundleIdentifier</key>
  <string>io.local.wooting-host-profile</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleName</key>
  <string>Wooting Host Profile</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.4.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

open "$app_dir"
echo "Installed and opened $app_dir"
echo "Choose a profile, optionally enable startup, then select Save and run in background."
