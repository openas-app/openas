#!/bin/bash
# Builds OpenAs from source and installs it in ~/Applications.
# Needs Rust (https://rustup.rs) and Apple's command line tools (xcode-select --install).
set -euo pipefail
cd "$(dirname "$0")"

[ "$(uname)" = Darwin ] || { echo "OpenAs runs on macOS for now."; exit 1; }
command -v cargo >/dev/null || { echo "Rust is missing. Install it from https://rustup.rs, then run this again."; exit 1; }
xcode-select -p >/dev/null 2>&1 || { echo "Apple's command line tools are missing. Run: xcode-select --install"; exit 1; }

echo "Building OpenAs (the first build takes a few minutes)…"
cargo build --release
runner/scripts/bundle_mac.sh >/dev/null 2>&1

target/release/openas setup
echo
echo "OpenAs is in ~/Applications and opening now. Choose a model, then describe an app."
