#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."

annotate() {
  ${BUILDKITE:-false} && {
    buildkite-agent annotate "$@"
  }
}

ci/affects-files.sh \
  .rs$ \
  ci/test-stable-perf.sh \
  ci/test-stable.sh \
|| {
  annotate --style info --context test-stable-perf \
    "Stable Perf skipped as no .rs files were modified"
  exit 0
}


./fetch-perf-libs.sh
# shellcheck source=/dev/null
source ./target/perf-libs/env.sh

FEATURES=bpf_c,cuda,erasure,chacha
exec ci/test-stable.sh "$FEATURES"
