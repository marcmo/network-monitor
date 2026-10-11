#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app_dir="$project_dir/target/Network Monitor.app"

"$project_dir/scripts/build-web.sh"
cargo build --release --manifest-path "$project_dir/Cargo.toml" --target-dir "$project_dir/target"
mkdir -p "$app_dir/Contents/MacOS"
mkdir -p "$app_dir/Contents/Resources/web"
cp -R "$project_dir/web/dist/." "$app_dir/Contents/Resources/web/"
cp "$project_dir/native/Info.plist" "$app_dir/Contents/Info.plist"
cp "$project_dir/target/release/network-monitor" "$app_dir/Contents/MacOS/network-monitor"
codesign --force --sign - "$app_dir"
codesign --verify --strict "$app_dir"
printf 'Launch in your terminal:\n"%s/Contents/MacOS/network-monitor"\n' "$app_dir"
