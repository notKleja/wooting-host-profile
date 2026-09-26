#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/wooting-switch-macos-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT HUP INT TERM

output_dir="$test_root/output"
if "$project_dir/scripts/build-macos.sh" "$test_root/not-an-app" >/dev/null 2>&1; then
    echo "build-macos.sh accepted a non-app output path" >&2
    exit 1
fi
"$project_dir/scripts/package-macos.sh" "$output_dir"

version=$(sed -n '/^version = "/ { s/^version = "//; s/"$//; p; q; }' "$project_dir/Cargo.toml")
architecture=$(uname -m)
app="$output_dir/Wooting Switch.app"
archive="$output_dir/Wooting-Switch-macOS-$architecture.zip"
checksum_file="$output_dir/SHA256SUMS-macOS.txt"

test -x "$app/Contents/MacOS/WootingHostProfile"
test -x "$app/Contents/Resources/wooting-host-profile-agent"
test -f "$app/Contents/Resources/AppIcon.icns"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Contents/Info.plist")" = "$version"
test "$("$app/Contents/Resources/wooting-host-profile-agent" --version)" = "wooting-host-profile $version"
codesign --verify --deep --strict "$app"
test -s "$archive"
test -s "$checksum_file"

extracted="$test_root/extracted"
mkdir -p "$extracted"
ditto -x -k "$archive" "$extracted"
test -x "$extracted/Wooting Switch.app/Contents/MacOS/WootingHostProfile"
test -x "$extracted/Wooting Switch.app/Contents/Resources/wooting-host-profile-agent"

expected=$(awk -v name="$(basename "$archive")" '$2 == name { print $1 }' "$checksum_file")
actual=$(shasum -a 256 "$archive" | awk '{ print $1 }')
test -n "$expected"
test "$actual" = "$expected"

printf 'macOS package smoke test passed for %s\n' "$archive"
