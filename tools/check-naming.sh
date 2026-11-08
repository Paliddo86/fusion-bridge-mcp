#!/usr/bin/env bash
# No remnant of the retired host-app names in the Rust workspace or the contract.
# (John, 2026-09-01: "make sure there's no more remnants of ad or adom-desktop naming".)
set -euo pipefail
cd "$(dirname "$0")/.."
hits=$(grep -rnE --exclude-dir=target "adom-desktop|adom_desktop|ad_client|_ad_call|Adom Desktop|\bAD\b" rust/ contracts/ 2>/dev/null | grep -v "check-naming" || true)
if [ -n "$hits" ]; then echo "NAMING REMNANTS:"; echo "$hits"; exit 1; fi
echo "no retired names in rust/ or contracts/"
