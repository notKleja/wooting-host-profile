#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_dir="$HOME/Applications/Wooting Switch.app"
build_root=$(mktemp -d "${TMPDIR:-/tmp}/wooting-switch-install.XXXXXX")
trap 'rm -rf "$build_root"' EXIT HUP INT TERM

"$project_dir/scripts/build-macos.sh" "$build_root/Wooting Switch.app"
mkdir -p "$(dirname "$app_dir")"
rm -rf "$app_dir"
ditto "$build_root/Wooting Switch.app" "$app_dir"
codesign --verify --deep --strict "$app_dir"

open "$app_dir"
echo "Installed and opened $app_dir"
echo "Choose a profile or option; changes apply immediately."
