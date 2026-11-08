#!/bin/bash
# Daily fusion-bridge verb test (see dev-skills/fusion-verb-tests).
# Cron-safe: runs the daily tier against AdomLapper, keeps a dated log, and on a
# red run files ONE issue on adom/fusion-bridge with the scorecard so a failure
# is a filed finding by the time anyone looks, not a log nobody reads.
# the wiki CLI that knows `issue create`: two lineages live on this box (/usr/local/bin is a
# June 1.12.x without `issue`; ~/.local/bin is the current one), so probe instead of guessing
WIKI=""
for w in "$HOME/.local/bin/adom-wiki" /usr/local/bin/adom-wiki "$(command -v adom-wiki 2>/dev/null)"; do
    [ -n "$w" ] && [ -x "$w" ] && "$w" issue --help >/dev/null 2>&1 && { WIKI="$w"; break; }
done
[ -n "$WIKI" ] || WIKI="$(command -v adom-wiki)"
set -u
REPO=/home/adom/project/adom-desktop-fusion-bridge
LOGDIR="$REPO/tests/reports"
STAMP=$(date +%Y-%m-%d)
LOG="$LOGDIR/daily-cron-$STAMP.log"
cd "$REPO" || exit 1
mkdir -p "$LOGDIR"

python3 tests/run_verb_tests.py --target "${FUSION_TEST_TARGET:-ConfRoomROG}" --tier daily >"$LOG" 2>&1
RC=$?
if [ $RC -eq 0 ]; then
    echo "GREEN $STAMP" >>"$LOGDIR/daily-cron-history.txt"
    exit 0
fi

echo "RED $STAMP (exit $RC)" >>"$LOGDIR/daily-cron-history.txt"
BODY=$(mktemp)
{
    echo "Automated daily verb run went red ($STAMP, exit $RC). Scorecard tail:"
    echo
    echo '```'
    tail -25 "$LOG"
    echo '```'
    echo
    echo "Full log: tests/reports/daily-cron-$STAMP.log on the dev container. Versions at run time are in the readiness line of the log. Triage per dev-skills/fusion-verb-tests: re-run the failing verbs with --only first, a startup race or relay flake re-passes; a repeat failure is the real finding."
} >"$BODY"
"$WIKI" issue create adom/fusion-bridge \
    --title "Daily verb run RED $STAMP: $(grep -c FAIL: "$LOG" 2>/dev/null || echo '?') failing verb(s)" \
    --body-file "$BODY" >>"$LOG" 2>&1
rm -f "$BODY"
exit $RC
