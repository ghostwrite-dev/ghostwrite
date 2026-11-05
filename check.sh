#!/bin/sh
# What has to pass before a commit: formatting, lints, and the tests.
set -eu
cd "$(dirname "$0")"

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/build-plugins.sh
cargo test --workspace
# Without local models, as it can be built without a C++ compiler.
cargo check -p ghostwrite --no-default-features --features wasm-plugins
