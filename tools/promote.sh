#!/usr/bin/env bash
# Promote an insiders build of the fusion bridge to the PUBLIC tier.
#
# The public manifest (adom-bridge-fusion-manifest.json on the fusion-bridge page) moves
# ONLY through this script, and only with the human's exact words recorded. release.sh
# never touches it (page issues #72/#73; pattern: the insiders-tier-publish skill).
#
#   tools/promote.sh <version> --attest "<the human's exact words approving the promote>"
#
# Promotion is a manifest edit, never a re-upload: the zip already on the release row is
# the one canonical download for that version in both tiers.
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION="${1:?usage: promote.sh <version> --attest \"<human words>\"}"
shift
ATTEST=""
while [ $# -gt 0 ]; do
  case "$1" in
    --attest) ATTEST="${2:?--attest needs the human words}"; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 2 ;;
  esac
done
if [ -z "$ATTEST" ]; then
  echo "REFUSED: --attest \"<the human's exact words>\" is required - promotion happens on a human's words, never on an agent's judgment." >&2
  exit 2
fi

MANIFEST="adom-bridge-fusion-manifest.json"
TMP=$(mktemp)
adom-wiki api "/api/v1/pages/insiders/files/fusion-bridge/version.json" > "$TMP" 2>/dev/null \
  || { echo "could not fetch the insiders manifest" >&2; exit 1; }
python3 tools/promote.py "$TMP" "$VERSION" "$MANIFEST"
rm -f "$TMP"

echo "Promoting $VERSION to public with attestation: $ATTEST"
adom-wiki repo push adom/fusion-bridge --files "$MANIFEST" --json 2>/dev/null \
  | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if d.get('status')=='ok' else 1)" \
  || { echo "public manifest push FAILED" >&2; exit 1; }
printf '%s\tpromoted %s\tattest: %s\n' "$(date -u +%FT%TZ)" "$VERSION" "$ATTEST" >> tools/promotions.log
git add "$MANIFEST" tools/promotions.log && git commit -qm "promote $VERSION to public tier

Attestation (the human's words): $ATTEST

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>" || true
echo "PROMOTED: public manifest now serves $VERSION"
