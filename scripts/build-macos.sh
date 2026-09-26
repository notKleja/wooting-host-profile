#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_dir=${1:-"$project_dir/dist/macos/Wooting Switch.app"}
contents="$app_dir/Contents"
macos_dir="$contents/MacOS"
resources_dir="$contents/Resources"
target_dir=${CARGO_TARGET_DIR:-"$project_dir/target"}
binary="$target_dir/release/wooting-host-profile"
version=$(sed -n '/^version = "/ { s/^version = "//; s/"$//; p; q; }' "$project_dir/Cargo.toml")

case $app_dir in
    /*.app|./*.app|../*.app) ;;
    *)
        echo "The macOS output path must name an explicit .app bundle: $app_dir" >&2
        exit 1
        ;;
esac

case $(uname -m) in
    arm64) swift_target=arm64-apple-macos13.0 ;;
    x86_64) swift_target=x86_64-apple-macos13.0 ;;
    *)
        echo "Unsupported macOS architecture: $(uname -m)" >&2
        exit 1
        ;;
esac

command -v cargo >/dev/null 2>&1 || {
    echo "Rust/Cargo is required to build the macOS app." >&2
    exit 1
}
command -v xcrun >/dev/null 2>&1 || {
    echo "Xcode command-line tools are required to build the macOS app." >&2
    exit 1
}

(cd "$project_dir" && MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --release)

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir" "$target_dir/swift-module-cache" "$target_dir/clang-module-cache"
cp "$binary" "$resources_dir/wooting-host-profile-agent"
chmod 755 "$resources_dir/wooting-host-profile-agent"
xattr -c "$resources_dir/wooting-host-profile-agent"
cp "$project_dir/macos/AppIcon.icns" "$resources_dir/AppIcon.icns"

CLANG_MODULE_CACHE_PATH="$target_dir/clang-module-cache" \
SWIFT_MODULE_CACHE_PATH="$target_dir/swift-module-cache" \
xcrun swiftc \
    -parse-as-library \
    -O \
    -target "$swift_target" \
    "$project_dir/macos/WootingHostProfileApp.swift" \
    -o "$macos_dir/WootingHostProfile" \
    -framework SwiftUI \
    -framework AppKit

cat > "$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Wooting Switch</string>
  <key>CFBundleExecutable</key>
  <string>WootingHostProfile</string>
  <key>CFBundleIdentifier</key>
  <string>io.local.wooting-host-profile</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleName</key>
  <string>Wooting Switch</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

codesign --force --deep --sign - "$app_dir"
codesign --verify --deep --strict "$app_dir"
printf 'Built %s\n' "$app_dir"
