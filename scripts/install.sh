#!/bin/bash

# Build in release mode and install the binary and its launcher to ~/.local/bin.
# Hooking it into Omarchy's idle service is a separate step: install-omarchy-hook.sh.

set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release

bin=$HOME/.local/bin
mkdir -p "$bin"
# Through a temporary file and a rename, so a running screensaver does not make the
# copy fail with "text file busy".
for src in target/release/tan-screensaver scripts/tan-screensaver-launch scripts/tan-screensaver-select; do
  dst=$bin/${src##*/}
  install -m755 "$src" "$dst.tmp"
  mv -f "$dst.tmp" "$dst"
done
echo "Installed tan-screensaver, tan-screensaver-launch and tan-screensaver-select to $bin"
