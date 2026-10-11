#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_dir/web"
# Trunk requires a boolean even when the caller exports NO_COLOR=1.
env NO_COLOR=true trunk build --release
