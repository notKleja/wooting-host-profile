#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
output_dir=${1:-"$project_dir/dist/macos"}
architecture=$(uname -m)
app="$output_dir/Wooting Switch.app"
archive="$output_dir/Wooting-Switch-macOS-$architecture.zip"
checksum_file="$output_dir/SHA256SUMS-macOS.txt"

mkdir -p "$output_dir"
"$project_dir/scripts/build-macos.sh" "$app"

rm -f "$archive" "$checksum_file"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
checksum=$(shasum -a 256 "$archive" | awk '{ print $1 }')
printf '%s  %s\n' "$checksum" "$(basename "$archive")" > "$checksum_file"

test -s "$archive"
codesign --verify --deep --strict "$app"
printf 'Packaged %s\n' "$archive"
printf 'Checksums: %s\n' "$checksum_file"
