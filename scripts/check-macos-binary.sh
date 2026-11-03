#!/usr/bin/env bash
# Checks the macOS release binary: an arm64 Mach-O, still signed after
# `strip = true`, that starts. Used by the CI `macos` job and the Release
# workflow's `build-macos` job; run on macOS.
#
# Usage: scripts/check-macos-binary.sh target/aarch64-apple-darwin/release/opendeck-utilities
set -euo pipefail

bin="${1:?usage: $0 <path to the macOS binary>}"
file "$bin"
# macOS's file says "executable arm64", Linux's "arm64 executable".
file -b "$bin" | grep -Eq '^Mach-O 64-bit (arm64 executable|executable arm64)'
# Apple Silicon kills unsigned binaries; the linker's ad-hoc
# signature must survive `strip = true`.
codesign --verify --verbose "$bin"
# Without OpenDeck's arguments it stops at the first missing flag,
# which proves it got as far as main().
out=$("$bin" 2>&1 || true)
echo "$out"
echo "$out" | grep -q "missing CLI flag: -port"
