#!/usr/bin/env bash
# Gate, then commit and push - never the other way round. Usage: tools/rust-commit.sh "message"
set -euo pipefail
cd "$(dirname "$0")/.."
bash tools/rust-ship.sh
git add -A
git commit -q -m "$1

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git push 2>&1 | tail -1
git log --oneline -1
