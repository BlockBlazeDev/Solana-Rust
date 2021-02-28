#!/usr/bin/env bash

set -e

cd "$(dirname "$0")/.."

source ci/_
source ci/rust-version.sh stable
source ci/rust-version.sh nightly
eval "$(ci/channel-info.sh)"
cargo="$(readlink -f "./cargo")"

scripts/increment-cargo-version.sh check

echo --- build environment
(
  set -x

  rustup run "$rust_stable" rustc --version --verbose
  rustup run "$rust_nightly" rustc --version --verbose

  "$cargo" stable --version --verbose
  "$cargo" nightly --version --verbose

  "$cargo" stable clippy --version --verbose
  "$cargo" nightly clippy --version --verbose

  # audit is done only with stable
  "$cargo" stable audit --version
)

export RUST_BACKTRACE=1
export RUSTFLAGS="-D warnings -A incomplete_features"

# Only force up-to-date lock files on edge
if [[ $CI_BASE_BRANCH = "$EDGE_CHANNEL" ]]; then
  # Exclude --benches as it's not available in rust stable yet
  if _ scripts/cargo-for-all-lock-files.sh +"$rust_stable" check --locked --tests --bins --examples; then
    true
  else
    check_status=$?
    echo "$0: Some Cargo.lock might be outdated; sync them (or just be a compilation error?)" >&2
    echo "$0: protip: $ ./scripts/cargo-for-all-lock-files.sh [--ignore-exit-code] ... \\" >&2
    echo "$0:   [tree (for outdated Cargo.lock sync)|check (for compilation error)|update -p foo --precise x.y.z (for your Cargo.toml update)] ..." >&2
    exit "$check_status"
  fi

  # Ensure nightly and --benches
  _ scripts/cargo-for-all-lock-files.sh +"$rust_nightly" check --locked --all-targets
else
  echo "Note: cargo-for-all-lock-files.sh skipped because $CI_BASE_BRANCH != $EDGE_CHANNEL"
fi

_ ci/order-crates-for-publishing.py

# -Z... is needed because of clippy bug: https://github.com/rust-lang/rust-clippy/issues/4612
# run nightly clippy for `sdk/` as there's a moderate amount of nightly-only code there
_ "$cargo" nightly clippy -Zunstable-options --workspace --all-targets -- --deny=warnings --deny=clippy::integer_arithmetic

_ "$cargo" stable fmt --all -- --check

_ ci/do-audit.sh

{
  cd programs/bpf
  for project in rust/*/ ; do
    echo "+++ do_bpf_checks $project"
    (
      cd "$project"
      _ "$cargo" nightly clippy -- --deny=warnings --allow=clippy::missing_safety_doc
      _ "$cargo" stable fmt -- --check
      _ "$cargo" nightly test
    )
  done
}

echo --- ok
