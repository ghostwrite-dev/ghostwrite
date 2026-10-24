#!/bin/sh
# What has to pass before a commit: formatting, lints, and the tests.
set -eu
cd "$(dirname "$0")"

cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
