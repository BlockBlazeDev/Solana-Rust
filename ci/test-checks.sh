#!/usr/bin/env bash
set -e

cd "$(dirname "$0")/.."

ci/version-check.sh stable
export RUST_BACKTRACE=1
export RUSTFLAGS="-D warnings"

_() {
  echo "--- $*"
  "$@"
}

_ cargo fmt --all -- --check
_ cargo clippy --all -- --version
_ cargo clippy --all -- --deny=warnings

_ ci/audit.sh
