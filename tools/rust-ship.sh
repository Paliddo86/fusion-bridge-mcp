#!/usr/bin/env bash
# The gate before any Rust commit or deploy: both targets build, the tests pass, the guards pass.
# (2026-09-04: a `cargo build | grep error` pipeline hid a type error and a broken commit was pushed.)
set -euo pipefail
cd "$(dirname "$0")/../rust"
cargo build --quiet
# (2026-09-04 again: `cargo test | grep FAILED && exit 1 || true` under pipefail never exited - a
# failing cargo made the pipeline fail, so the && branch was skipped and a red test was pushed.)
cargo test --quiet >"${TMPDIR:-/tmp}/rust-ship-test.log" 2>&1 || { grep -E 'FAILED|panicked|error' "${TMPDIR:-/tmp}/rust-ship-test.log" | head -20; echo "TESTS FAILED"; exit 1; }
cargo build --release --quiet --target x86_64-pc-windows-gnu
cd ..
bash tools/check-portable.sh >/dev/null
bash tools/check-naming.sh >/dev/null
echo "rust-ship: linux build, tests, windows build, guards: all green"
