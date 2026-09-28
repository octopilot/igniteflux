#!/usr/bin/env bash
# Unit tests for the pipeline Test leg (BP_TEST_COMMAND in skaffold.yaml).
set -euo pipefail
cargo test --all-features
