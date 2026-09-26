#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$project_dir/dist/macos"}
architecture=$(uname -m)
version=$(sed -n '/^version = "/ { s/^version = "//; s/"$//; p; q; }' "$project_dir/Cargo.toml")
app="$output_dir/Wooting Switch.app"
archive="$output_dir/Wooting-Switch-macOS-$architecture.zip"
installer="$output_dir/Wooting-Switch-Setup-macOS-$architecture.pkg"
checksum_file="$output_dir/SHA256SUMS-macOS.txt"

mkdir -p "$output_dir"
"$project_dir/scripts/build-macos.sh" "$app"

rm -f "$archive" "$installer" "$checksum_file"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
pkgbuild \
    --component "$app" \
    --install-location /Applications \
    --identifier io.local.wooting-host-profile \
    --version "$version" \
    "$installer"

for artifact in "$archive" "$installer"; do
    checksum=$(shasum -a 256 "$artifact" | awk '{ print $1 }')
    printf '%s  %s\n' "$checksum" "$(basename "$artifact")" >> "$checksum_file"
done

test -s "$archive"
test -s "$installer"
codesign --verify --deep --strict "$app"
printf 'Packaged %s\n' "$archive"
printf 'Packaged %s\n' "$installer"
printf 'Checksums: %s\n' "$checksum_file"
