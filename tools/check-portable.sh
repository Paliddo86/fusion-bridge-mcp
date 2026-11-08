#!/usr/bin/env bash
# fusion-core must never depend on an OS crate. Run in CI and before every release.
set -euo pipefail
cd "$(dirname "$0")/../rust"
bad=$(cargo tree -p fusion-core -e normal 2>/dev/null | grep -iE "^\s*[│├└─ ]*(windows|windows-sys|winapi|cocoa|objc|core-foundation)\b" || true)
if [ -n "$bad" ]; then echo "PORTABILITY DRIFT: fusion-core depends on an OS crate:"; echo "$bad"; exit 1; fi
echo "fusion-core is OS-blind"
# No `todo!()` may ship in a crate the Windows exe links: one killed the shell on its first
# /status request. (os-mac is Kyle's crate and may still carry stubs until his port lands.)
todo=$(grep -rn "todo!(" core/src lbr/src os-win/src bridge/src | grep -vE ':[0-9]+:\s*//' || true)
if [ -n "$todo" ]; then echo "SHIP BLOCKER: todo!() in a linked crate:"; echo "$todo"; exit 1; fi
echo "no todo!() in the linked crates"
