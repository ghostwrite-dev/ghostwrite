#!/bin/sh
# What has to pass before a commit: formatting, lints, and the tests.
set -eu
cd "$(dirname "$0")"

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/build-plugins.sh
cargo test --workspace
# With Typst's native crate built in instead of its plugin.
cargo test -p ghostwrite --no-default-features --features typst,lean,wasm-plugins
