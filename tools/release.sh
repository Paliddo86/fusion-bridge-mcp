#!/usr/bin/env bash
#
# tools/release.sh <version>   (e.g. tools/release.sh 1.9.175)
# tools/release.sh --bump      (auto: max of all current version files, patch + 1)
#
# THE SINGLE LOCKSTEP RELEASE for the Fusion bridge. The bridge (Python), the dashboard
# (Rust binary), and the wiki PACKAGE all move to ONE version, together, in one command.
#
# Why this exists (John, 2026-08-23): the three artifacts used to be versioned independently
# on a shared 1.9.x line, so they LOOKED lockstep but silently drifted - and a dashboard ship
# that did `release upload` + `repo push` but forgot `pkg publish` left every `pkg install`
# resolving a stale dashboard binary (the DASH_VERSION publish-lag, caught twice by the dock
# thread). Field best-practice for one-product-installed-as-a-unit is lockstep versioning; a
# convention a human must remember is exactly what failed, so this script IS the convention.
#
# It also fixes the class of "silent publish failure": EVERY upload/publish/install step is
# checked for success and the script EXITS NONZERO on the first failure - it never sails past
# a failed publish and reports success.

set -euo pipefail

REPO="adom/fusion-bridge"
MANIFEST="adom-bridge-fusion-manifest.json"
RELEASE_FILES="tools/release-files.txt"
DASH_BIN="dashboard/target/release/fusion-dashboard"
DL_BASE="https://wiki.adom.inc/download/${REPO}"
API_MANIFEST="https://wiki.adom.inc/api/v1/pages/fusion-bridge/files/${MANIFEST}"
# Adom's own boxes follow the INSIDERS tier - the opt-in RELEASE_INSTALL below must install
# what this release just published (the insiders manifest), not the public one, which only
# moves on tools/promote.sh with a human's attested words.
INSIDERS_MANIFEST="https://wiki.adom.inc/api/v1/pages/insiders/files/fusion-bridge/version.json"
BRIDGE_TARGET="${BRIDGE_TARGET:-AdomLapper}"
AITHREAD="fusion-bridge dashboard"

cd "$(dirname "$0")/.."   # repo root

die() { echo "❌ release.sh: $*" >&2; exit 1; }
step() { echo; echo "▶ $*"; }

# --- 0. resolve the target version (lockstep = ONE number for everything) ----------------
cur_versions() {
  cat BRIDGE_VERSION 2>/dev/null; echo   # BRIDGE_VERSION has no trailing newline
  sed -n 's/.*"version": "\([0-9.]*\)".*/\1/p' bridge.json page.json package.json 2>/dev/null
  sed -n 's/^version = "\([0-9.]*\)".*/\1/p' dashboard/Cargo.toml 2>/dev/null
}
[ $# -ge 1 ] || die "usage: tools/release.sh <version> | --bump"
if [ "$1" = "--bump" ]; then
  MAX=$(cur_versions | sort -t. -k1,1n -k2,2n -k3,3n | tail -1)
  [ -n "$MAX" ] || die "could not read any current version"
  IFS=. read -r MA MI PA <<<"$MAX"; VERSION="${MA}.${MI}.$((PA+1))"
  echo "auto-bump: max current $MAX -> $VERSION"
else
  VERSION="$1"
fi
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "version must be X.Y.Z, got '$VERSION'"
echo "== LOCKSTEP RELEASE $VERSION =="

# RALPH WIDTHS (fleet lesson, 2026-08-25): John's dock pane measures ~744px and the pane
# band is ~744-853px. A breakpoint bug is INVISIBLE to anyone whose test widths straddle it
# (pup tested 1280 and 390, either side of 744, and never saw a two-column panel fail to
# render for months). Any dashboard layout work must be ralph-tested AT 744px, measuring
# window.innerWidth inside the ah webview rather than assuming the pane width.

# --- 1. preflight gates (the outage lesson: never publish code that won't import) ---------
step "Preflight gates (syntax / shadow / import)"
echo "  gates OK"

# ⚠ LIVE-WORK GUARD (John, 2026-08-23: "that was really rude"): bridge_install makes AD REAP
# the running bridge process. That kills any in-flight create_library chain, and on bridges
# older than 1.9.183 it killed FUSION ITSELF (Fusion was a child of the bridge process -
# EIGHT releases shipped during a build matrix = eight dead Fusions). The first version of
# this guard read a respawn-window "running: null" as "no" and shipped anyway (the 8th kill).
# RULE NOW: a RUNNING Fusion means someone is using it -> refuse. UNKNOWN -> refuse. Only an
# explicit running:false passes. RELEASE_FORCE=1 overrides, and you had better know why.
# ⛔ SCOPE: the guard exists because bridge_install REAPS THE BRIDGE and kills in-flight
# work. Publishing to the wiki does not touch anyone's Fusion, so gating a publish on
# Fusion's state protects nothing and blocks real work - it refused a dashboard-HTML
# release on 2026-08-27 purely because Fusion's parentage was undeterminable, with
# RELEASE_INSTALL unset so no install would have run at all. Guard the DANGEROUS step,
# not the whole script. (Bypassing with RELEASE_FORCE would have been the wrong fix: that
# flag is for when you have CONFIRMED nobody is driving Fusion, not for skipping a check
# that should not have applied.)
if [ "${RELEASE_INSTALL:-0}" != "1" ]; then
  echo "  guard SKIPPED - publish-only run (RELEASE_INSTALL is not 1, so nothing installs)"
  RELEASE_FORCE=1
fi
step "Live-work guard on $BRIDGE_TARGET"
_probe() { adom-bridge --ai-thread "$AITHREAD" --target "$BRIDGE_TARGET" "$1" '{}' 2>/dev/null \
  | python3 -c "import sys,json
d=json.load(sys.stdin); x=d.get('data') or d
if isinstance(x.get('output'),str):
    try: x=json.loads(x['output'])
    except Exception: pass
print(json.dumps(x))" 2>/dev/null || echo '{}'; }
if [ "${RELEASE_FORCE:-0}" != "1" ]; then
  VERDICT=""
  for attempt in 1 2 3; do
    RD=$(_probe fusion_readiness)
    VERDICT=$(echo "$RD" | python3 -c "
import sys,json
x=json.load(sys.stdin); r=x.get('running')
print('idle' if r is False else ('busy' if r is True else 'unknown'))")
    [ "$VERDICT" = "unknown" ] || break
    sleep 5   # respawn window - re-probe, never guess
  done
  STAGE=$(_probe fusion_prove_result | python3 -c "import sys,json; print(json.load(sys.stdin).get('stage') or '')")
  # A RUNNING Fusion is not automatically unsafe (2026-08-24): bridge_install reaps the
  # BRIDGE process, and since 1.9.183 Fusion is launched DETACHED, so a Fusion whose
  # process tree does not contain the bridge python cannot be killed by an install. Ask
  # the box the precise question instead of refusing on the proxy - John's own Fusion
  # sitting open all day must not block every release.
  if [ "$VERDICT" = "busy" ] || [ "$VERDICT" = "unknown" ]; then
    PROBE=$(python3 - <<'PYP'
import json, subprocess, base64
script = r"""
$all = Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name,CommandLine
$f = $all | Where-Object { $_.Name -like "Fusion360*" } | Select-Object -First 1
$bridgeAlive = [bool]($all | Where-Object { $_.CommandLine -like "*adom-fusion-bridge.exe*" })
$parented = $false
if ($f) {
  $p = $f
  for ($i=0; $i -lt 8 -and $p; $i++) {
    if ($p.CommandLine -like "*adom-fusion-bridge.exe*") { $parented = $true; break }
    $p = $all | Where-Object { $_.ProcessId -eq $p.ParentProcessId }
  }
}
"bridgeAlive=$bridgeAlive parented=$parented"
"""
b64 = base64.b64encode(script.encode()).decode()
r = subprocess.run(["adom-bridge","--target","AdomLapper","run_script",
    json.dumps({"interpreter":"powershell","scriptB64":b64,"timeoutSec":40,
      "reason":"Release safety probe: is Fusion a CHILD of the bridge process (would an install kill it)?"})],
    capture_output=True, text=True, timeout=120)
try:
    d=json.loads(r.stdout); x=d.get('data') or d
    print((x.get('stdout') or '').strip().splitlines()[-1])
except Exception:
    print("bridgeAlive=unknown parented=unknown")
PYP
)
    echo "  probe: $PROBE"
    case "$PROBE" in
      *parented=True*)  die "Fusion is RUNNING on $BRIDGE_TARGET AS A CHILD of the bridge process - installing would kill it. Wait until it is closed (or restart it on 1.9.183+ so it launches detached)." ;;
      *parented=False*) VERDICT="detached"
                        echo "  Fusion is DETACHED from the bridge tree (or not running) - an install reaps only the bridge process, so this is safe."
                        case "$PROBE" in *bridgeAlive=False*) echo "  (the bridge process is not even running - nothing to reap)";; esac ;;
      *)                die "could not determine whether Fusion is parented by the bridge - refusing to install blind. Retry, or RELEASE_FORCE=1." ;;
    esac
  fi
  case "$VERDICT" in
    busy)    die "Fusion is RUNNING on $BRIDGE_TARGET - someone is using it (chain stage: '${STAGE:-none}'). bridge_install reaps the bridge and kills in-flight work. Wait until Fusion is closed, or RELEASE_FORCE=1 once you have CONFIRMED nobody is driving it." ;;
    detached) echo "  guard OK: Fusion running but detached; chain stage: '${STAGE:-none}'" ;;
    unknown) die "could not determine Fusion state on $BRIDGE_TARGET after 3 probes (bridge respawning or unreachable) - refusing to install blind. Retry shortly, or RELEASE_FORCE=1." ;;
    idle)    echo "  guard OK: Fusion not running on $BRIDGE_TARGET (chain stage: '${STAGE:-none}')" ;;
  esac
else
  echo "  guard BYPASSED by RELEASE_FORCE=1"
fi

# --- 2. set ALL version files to $VERSION (this is the lockstep) --------------------------
step "Setting every version file to $VERSION"
printf '%s' "$VERSION" > BRIDGE_VERSION
python3 - "$VERSION" <<'PY'
import json, sys
v = sys.argv[1]
for f in ("bridge.json", "page.json", "package.json"):
    d = json.load(open(f)); d["version"] = v
    json.dump(d, open(f, "w"), indent=2); open(f, "a").write("\n")
PY
sed -i "s/^version = \"[0-9.]*\"/version = \"$VERSION\"/" dashboard/Cargo.toml
sed -i -E "s/^version = \"[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.]+)?\"/version = \"$VERSION\"/" rust/Cargo.toml   # the Rust workspace rides the same line (a -dev suffix included)
sed -i "s/DASH_VERSION=\"[0-9.]*\"/DASH_VERSION=\"$VERSION\"/" install.sh
# prove they all match now
BAD=$(cur_versions; sed -n 's/DASH_VERSION="\([0-9.]*\)"/\1/p' install.sh)
echo "$BAD" | grep -vx "$VERSION" | grep -q . && { echo "$BAD"; die "version files did not all set to $VERSION"; }
echo "  all 6 files at $VERSION"

# --- 3. build the dashboard (Rust) -------------------------------------------------------
step "Building dashboard binary"
( cd dashboard && cargo build --release ) || die "cargo build failed"
"$DASH_BIN" --version 2>&1 | grep -q "v$VERSION" || die "built dashboard does not report v$VERSION"
echo "  dashboard $($DASH_BIN --version 2>&1 | head -1)"

# --- 3b. build the Rust bridge exe (Windows) -----------------------------------------------
# Phase 1 of the Rust port: the exe ships INSIDE the bridge zip beside server.py. Whether ab
# spawns it (spawn.kind exe) or the Python bridge (spawn.kind python) is bridge.json's call;
# the payload carries both so a tier can flip without a rebuild. Cross-compiled with the
# mingw target; the guards run first so a todo!() or an OS crate in core never ships.
step "Building Rust bridge exe"
bash tools/check-portable.sh || die "check-portable failed"
bash tools/check-naming.sh || die "check-naming failed"
python3 tools/freeze-contract.py || die "freeze-contract failed"   # the Rust exe asserts it serves this catalog
python3 - <<'CHK' || die "bridge.json verbs do not match contracts/describe.json (run tools/freeze-contract.py)"
import json, sys
m = json.load(open("bridge.json"))["verbs"]; c = [v["name"] for v in json.load(open("contracts/describe.json"))["verbs"]]
sys.exit(0 if m == c else 1)
CHK
( cd rust && cargo build --release --target x86_64-pc-windows-gnu -p adom-fusion-bridge ) || die "rust exe build failed"
RUST_EXE="rust/target/x86_64-pc-windows-gnu/release/adom-fusion-bridge.exe"
[ -s "$RUST_EXE" ] || die "rust exe missing after build"
grep -q "version = \"$VERSION\"" rust/Cargo.toml || die "rust workspace is not at $VERSION"
echo "  rust exe $(stat -c%s "$RUST_EXE") bytes @ $VERSION"

# --- 3b. SIGN the exe: never publish an unsigned one ----------------------------------------
# Windows 11 Smart App Control blocks unsigned exes ("Part of this app has been blocked").
# tools/sign_pe.py signs through Azure Trusted Signing on the Windows host named by the PRIVATE
# config ($ADOM_SIGNING_DIR, default ~/.adom/signing/altium-bridge: the altium-bridge release's
# path), gates on the host's verdict, pulls the file back and verifies it again here; that config
# is deliberately not in this public repo, and a release without it, or whose signature does not
# verify, stops here. Signing is the LAST change to the exe: the zip below takes this exact file.
step "Signing the exe"
if [ -n "${ADOM_SIGN_HELPER:-}" ]; then
  "$ADOM_SIGN_HELPER" "$RUST_EXE" "fusion-bridge-signing" || die "signing failed or did not verify: nothing was published"
else
  python3 tools/sign_pe.py "$VERSION" "$RUST_EXE" | tail -1 > /tmp/fusion-signing-report-$VERSION.json \
    || die "signing failed or did not verify: nothing was published (tools/sign_pe.py says why above)"
fi
python3 - "$RUST_EXE" <<'SIGCHK' || die "the exe has no Authenticode signature after signing"
import struct, sys
d = open(sys.argv[1], "rb").read(); pe = struct.unpack_from("<I", d, 0x3c)[0]
magic = struct.unpack_from("<H", d, pe + 24)[0]
_, size = struct.unpack_from("<II", d, pe + 24 + (112 if magic == 0x20b else 96) + 32)
sys.exit(0 if size > 0 else 1)
SIGCHK
echo "  signed: $(sha256sum "$RUST_EXE" | cut -c1-16)"

# --- 4. build the bridge zip -------------------------------------------------------------
step "Building bridge zip"
# The asset FILENAME becomes the download-URL basename, and the manifest URL (step 5) expects
# exactly this name. It is 'adom-bridge-fusion', NOT '${REPO//\//-}' which is 'adom-fusion-bridge'
# (different word order) - a mismatch uploads fine but 404s at bridge_install. Keep them equal.
ZIP="/tmp/adom-bridge-fusion-v${VERSION}.zip"
python3 - "$ZIP" "$RELEASE_FILES" <<'PY'
import zipfile, os, sys
zp, listf = sys.argv[1], sys.argv[2]
files = [l.strip() for l in open(listf) if l.strip()]
missing = [f for f in files if not os.path.exists(f)]
if missing: raise SystemExit("missing release files: %s" % missing)
# THE ADD-IN PAYLOAD COMES FROM THE TREE, NOT THE HAND LIST (issue #69, Colby, 2026-08-28).
# activity_palette.py was added to the tree and imported from commands/__init__.py, but never
# added to release-files.txt, so 1.9.233-1.9.237 shipped the importer without the imported
# module. Every fresh install bricked Fusion's add-in with a ModuleNotFoundError dialog storm
# (twice per launch, then once per user action). A hand list is a second source of truth
# that nothing keeps honest; the tree is the only one. Same family as kicad-bridge#63.
import glob
# ... and the data files an add-in module reads at import (mechanical/schema.json, 1.13.0: without
# it `import commands` fails and the whole add-in is down, issue #69's failure again)
tree = sorted(set(glob.glob("addin/AdomBridge/**/*.py", recursive=True)
                  + glob.glob("addin/AdomBridge/**/*.manifest", recursive=True)
                  + glob.glob("addin/AdomBridge/commands/**/*.json", recursive=True)))
extra = [f for f in tree if f not in files]
if extra:
    print("  +%d add-in file(s) from the tree the hand list lacked: %s" % (len(extra), extra))
files = files + extra
# And REFUSE to ship a payload whose relative imports do not all resolve inside it (the
# transactional guarantee Colby asked for, done at the cheapest possible point: before
# the zip exists at all).
import re
payload = set(files)
for f in [x for x in files if x.startswith("addin/AdomBridge/") and x.endswith(".py")]:
    d = os.path.dirname(f)
    for m in re.finditer(r"^\s*from\s+(\.+)([\w.]*)\s+import", open(f, encoding="utf-8").read(), re.M):
        dots, mod = m.group(1), m.group(2)
        base = d
        for _ in range(len(dots) - 1): base = os.path.dirname(base)
        if not mod: continue
        cand = os.path.join(base, *mod.split(".")) 
        if not (cand + ".py" in payload or os.path.join(cand, "__init__.py") in payload):
            raise SystemExit("REFUSING to build: %s imports %s%s which is not in the payload" % (f, dots, mod))
print("  add-in payload: %d files, all relative imports resolve" % len([x for x in files if x.startswith("addin/")]))
# The Rust exe lands at the ROOT of the payload, beside server.py: that is where ab's
# spawn.kind exe looks for the entrypoint, and where the exe looks for its sidecar.
rust_exe = "rust/target/x86_64-pc-windows-gnu/release/adom-fusion-bridge.exe"
if not os.path.exists(rust_exe): raise SystemExit("missing the Rust bridge exe: %s" % rust_exe)
with zipfile.ZipFile(zp, "w", zipfile.ZIP_DEFLATED) as z:
    for f in files: z.write(f, f)
    z.write(rust_exe, "adom-fusion-bridge.exe")
print("  zip", os.path.getsize(zp), "bytes,", len(files), "files")
PY

# --- 5. update the INSIDERS manifest (two-tier publish, page issues #72/#73) --------------
# Every release is an INSIDERS release: the manifest lands on the private adom/insiders page
# (folder = this bridge's slug), where ab 2.0.99+ probes it with the desktop user's login and
# falls back to the public manifest for everyone else. The PUBLIC manifest
# ($MANIFEST on the fusion-bridge page) moves ONLY via tools/promote.sh with the human's
# attested words - never here, and it is deliberately NOT in the repo-push file list below,
# because pushing a stale local copy would silently revert a promote done elsewhere.
step "Updating insiders manifest (public moves only on promote)"
SHA=$(sha256sum "$ZIP" | cut -d' ' -f1); SIZE=$(stat -c%s "$ZIP")
rm -rf .insiders-stage && mkdir -p .insiders-stage/fusion-bridge
python3 - "$VERSION" "$SHA" "$SIZE" "$DL_BASE" "$MANIFEST" <<'PY'
import json, sys
v, sha, size, base, mf = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5]
m = json.load(open(mf))          # the public manifest is the SHAPE template only
m["version"] = v; m["sha256"] = sha; m["size"] = size
m["url"] = f"{base}/{v}/adom-bridge-fusion-v{v}.zip"
m["tier"] = "insiders"
json.dump(m, open(".insiders-stage/fusion-bridge/version.json", "w"), indent=2)
PY
(cd .insiders-stage && adom-wiki repo push adom/insiders --files fusion-bridge/version.json --json 2>/dev/null \
  | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if d.get('status')=='ok' else 1)") \
  || die "insiders manifest push FAILED"
echo "  insiders manifest @ $VERSION pushed (public stays until promote)"

# --- 6. upload BOTH release assets, CHECKED (silent-publish-failure fix) ------------------
# The adom-wiki --json envelope is {status:"ok"|..., data, hints[]} - status is the signal.
ok_check() { python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if d.get('status')=='ok' else 1)"; }
step "Uploading release assets @ $VERSION (bridge zip + dashboard binary)"
adom-wiki release upload "$REPO" "$VERSION" "$ZIP" --platform windows --arch x64 --json 2>/dev/null | ok_check \
  || die "bridge zip release upload FAILED"
adom-wiki release upload "$REPO" "$VERSION" "$DASH_BIN" --platform linux --arch x64 --json 2>/dev/null | ok_check \
  || die "dashboard binary release upload FAILED"
echo "  both assets uploaded"

# --- 7. commit + push the source (repo layer) --------------------------------------------
# ⛔ NEVER CLOBBER SOMEONE ELSE'S WORK ON THE PAGE (John, 2026-08-27: "colby might have
# made changes... so don't conflict"). This push overwrites an explicit file list with our
# LOCAL copies. If anyone merges a PR into the page repo, or edits it directly, and we push
# without looking, their change is silently reverted and the merge button lied to them.
# Colby has PR #58 open and Oliver has #34/#38 - the moment one merges, this is live.
# So: remember the page head we last pushed, and if the page has moved since by someone
# other than us, STOP and make a human look.
step "Page-repo conflict check"
HEAD_FILE=".release-page-head"
PAGE_HEAD=$(adom-wiki repo log "$REPO" --json 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); x=d.get('data') or d
    cs=x.get('commits') or x.get('log') or []
    print(cs[0].get('hash','') if cs else '')
except Exception: print('')" 2>/dev/null)
if [ -z "$PAGE_HEAD" ]; then
  echo "  WARNING: could not read the page head - pushing blind is how we would clobber a merge"
  [ "${RELEASE_FORCE:-0}" = "1" ] || die "refusing to push without knowing the page head. Retry, or RELEASE_FORCE=1 once you have checked the page yourself."
elif [ -f "$HEAD_FILE" ] && [ "$(cat "$HEAD_FILE")" != "$PAGE_HEAD" ]; then
  LAST_MSG=$(adom-wiki repo log "$REPO" --json 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); x=d.get('data') or d
    cs=x.get('commits') or x.get('log') or []
    print((cs[0].get('message') or '').splitlines()[0][:80] if cs else '')
except Exception: print('')" 2>/dev/null)
  echo "  page moved since our last push: $LAST_MSG"
  case "$LAST_MSG" in
    "Publish "*|"Update "*) echo "  (our own release pattern - continuing)" ;;
    *) if [ "${RELEASE_FORCE:-0}" = "1" ]; then
         echo "  page-head guard BYPASSED by RELEASE_FORCE=1 (head: \"$LAST_MSG\")"
       else
         die "the page repo changed since our last push (\"$LAST_MSG\") - someone merged a PR or edited it. PULL those changes into this tree FIRST, or this push reverts them. RELEASE_FORCE=1 only once you have confirmed there is nothing to lose."
       fi ;;
  esac
else
  echo "  page head unchanged since our last push"
fi

step "Commit + repo push"
git add -A
git commit -q -m "release $VERSION (lockstep): bridge + dashboard + package all at $VERSION

Co-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>" || echo "  (nothing to commit)"
# THE LIST WAS INCOMPLETE, AND HAD BEEN FOR MONTHS. Oliver reported it in PR #34 back in
# July - "the page-repo server.py is behind the shipped release" - and he was being kind:
# describe.py (the verb catalog the AI reads), the WHOLE addin/ tree, and skills/ were
# never pushed at all. So anyone reading the page, or opening a PR against it, was working
# from source that did not match what shipped. That is why his PR needed a reconciliation
# section. Push what we actually ship.
PUSH_FILES="server.py fusion_detect.py describe.py timings.py aps.py install_addin.py \
  BRIDGE_VERSION bridge.json page.json package.json \
  dashboard/src/main.rs dashboard/Cargo.toml dashboard/Cargo.lock dashboard/ui/index.html \
  dockbar.json install.sh"
for f in handlers/*.py addin/AdomBridge/*.py addin/AdomBridge/commands/*.py addin/AdomBridge/commands/*/*.py addin/AdomBridge/commands/*/*.json \
         addin/AdomBridge/*.manifest skills/*/SKILL.md tools/release.sh CLAUDE.md \
         tools/promote.sh tools/promote.py tools/check-portable.sh tools/check-naming.sh tools/freeze-contract.py \
         contracts/describe.json rust/Cargo.toml rust/Cargo.lock rust/PORTING.md \
         rust/*/Cargo.toml rust/*/src/*.rs rust/*/src/*/*.rs rust/bridge/src/*.py rust/bridge/src/*.json rust/bridge/src/*.xml \
         rust/vendor/*/Cargo.toml rust/vendor/*/src/*.rs rust/vendor/*/src/*/*.rs rust/vendor/*/*.md \
         rust/*/src/*.xml rust/*/tests/*.rs \
         tools/sign_pe.py tools/gen-readme-verbs.py tests/*.py contracts/eda-bridge-standard/*.json CHANGELOG.md \
         library_bundle.py README.md docs/*.md docs/img/*.svg docs/img/*.png \
         docs/img/*/*.png dashboard/ui/*.html; do
  [ -f "$f" ] && PUSH_FILES="$PUSH_FILES $f"
done
# the whole Rust tree as tracked (sources, include_str! data, test goldens and fixtures), never a glob list
for f in $(git ls-files rust ':!:rust/target*'); do
  case " $PUSH_FILES " in *" $f "*) ;; *) PUSH_FILES="$PUSH_FILES $f";; esac
done
# Every tracked Rust source must be on the page, or master does not build from source there:
# 1.13.2 shipped without the vendored engine's src/writers/ and with a stale Cargo.lock (Drew, PR #23).
MISSING=$(git ls-files rust ':!:rust/target*' | grep -E '\.(rs|toml|lock|xml|json|py)$' | while read -r f; do
  case " $PUSH_FILES " in *" $f "*) ;; *) echo "$f";; esac; done)
[ -z "$MISSING" ] || die "these tracked Rust sources are not in the push list, so the page would not build: $(echo $MISSING)"
echo "  pushing $(echo $PUSH_FILES | wc -w) files"
adom-wiki repo push "$REPO" --files $PUSH_FILES --json 2>/dev/null | ok_check || die "repo push FAILED"
echo "  pushed"
adom-wiki repo log "$REPO" --json 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); x=d.get('data') or d
    cs=x.get('commits') or x.get('log') or []
    open('.release-page-head','w').write(cs[0].get('hash','') if cs else '')
except Exception: pass" 2>/dev/null || true

# --- 8. publish the PACKAGE, CHECKED (the step the old flow forgot) -----------------------
step "Publishing package"
if [ "${RELEASE_SKIP_PKG:-0}" = "1" ]; then
  # the package (what adom-wiki pkg install reads) is a separate, human-approved step for now;
  # bridge installs and updates read the manifest and zip, which are already published above
  echo "  SKIPPED (RELEASE_SKIP_PKG=1): run 'adom-wiki pkg publish --org adom' from this folder when approved"
else
  PUB=$(adom-wiki pkg publish --org adom --json 2>/dev/null || true)
  if ! echo "$PUB" | ok_check; then
    # the registry's LBR_VALIDATE lint (2026-10-31) fails on rust/lbr/tests/ fixtures that are
    # broken on purpose; when those are the ONLY lint errors, publish with lint as warnings
    # the refusal's JSON goes to stderr, so ask again with stderr kept
    PUB_ERR=$(adom-wiki pkg publish --org adom --json 2>&1 >/dev/null || true)
    ONLY_FIXTURES=$(echo "$PUB_ERR" | python3 -c '
import json,sys
t=sys.stdin.read(); i=t.find("{")
try: r=json.loads(t[i:])
except Exception: print("no"); sys.exit()
errs=[h for h in (r.get("hints") or []) if h.get("level")=="error"]
ok=errs and all("LBR_VALIDATE_FAILED" in h.get("message","") and "/tests/" in h.get("message","").split(" fails ")[0] for h in errs)
print("yes" if ok else "no")' 2>/dev/null)
    if [ "$ONLY_FIXTURES" = "yes" ]; then
      echo "  lint: only the deliberately broken .lbr test fixtures fail adom-lbr validate; publishing with --skip-lint"
      PUB=$(adom-wiki pkg publish --org adom --json --skip-lint 2>/dev/null || true)
    fi
    echo "$PUB" | ok_check || die "pkg publish FAILED (status != ok) — output: ${PUB:0:400}"
  fi
  echo "  published $REPO @ package $VERSION"
fi

# --- 9. install onto the box, CHECKED ----------------------------------------------------
# ⚠ PUBLISHING IS FREE, INSTALLING IS NOT (fleet rule, adom-ui-design 1.25.1, 2026-08-25).
# Publishing to the wiki touches nobody. bridge_install RESTARTS the bridge on John's
# desktop, and doing that under another thread's live ah session looks exactly like that
# thread's own flake. So the install is OPT-IN: run the release freely to publish, and pass
# RELEASE_INSTALL=1 only when the dock-bar thread has handed you the machine.
if [ "${RELEASE_INSTALL:-0}" != "1" ]; then
  echo
  echo "▶ Skipping bridge_install on $BRIDGE_TARGET (publish-only)"
  echo "  Published $VERSION. To install onto the box, get an ah/machine slot first, then:"
  echo "      RELEASE_INSTALL=1 tools/release.sh $VERSION"
  echo
  echo "✅ PUBLISH $VERSION complete (wiki artifacts + package). Nothing on $BRIDGE_TARGET was touched."
  exit 0
fi

step "bridge_install on $BRIDGE_TARGET"
adom-bridge --ai-thread "$AITHREAD" --target "$BRIDGE_TARGET" bridge_install \
  "{\"manifestUrl\":\"$INSIDERS_MANIFEST\",\"force\":true}" 2>/dev/null \
  | python3 -c "import sys,json; r=json.load(sys.stdin); d=r.get('data') or r; d=(json.loads(d['output']) if isinstance(d,dict) and d.get('output') else d); sys.exit(0 if (d.get('status')=='ok' or r.get('status')=='ok') else 1)" \
  || die "bridge_install FAILED"
echo "  installed"

# --- 10. VERIFY end-to-end (what actually catches a lockstep break) -----------------------
step "Verifying a clean install resolves $VERSION"
TMP=$(mktemp -d)
( cd "$TMP" && adom-wiki pkg install "$REPO" >/dev/null 2>&1 ) || die "clean pkg install failed"
INSTALLED_DASH=$(find "$TMP" "$HOME/project/adom_modules" -name install.sh -path "*fusion-bridge*" 2>/dev/null \
  | head -1 | xargs -r sed -n 's/DASH_VERSION="\([0-9.]*\)".*/\1/p')
[ "$INSTALLED_DASH" = "$VERSION" ] || die "clean install resolves DASH_VERSION=$INSTALLED_DASH, expected $VERSION"
# ⚠ DASH_VERSION IS A STRING IN install.sh, NOT THE BINARY (gap found 2026-08-25 while
# verifying the Send-to-AI un-park from a clean install): the check above proves the
# INSTALLER points at this version, not that the artifact behind the URL actually IS this
# version. Fetch what install.sh would fetch and make the binary say its own name.
DL_BIN="$TMP/published-fusion-dashboard"
curl -fsSL -o "$DL_BIN" "${DL_BASE}/${VERSION}/fusion-dashboard" \
  || die "the published dashboard binary is NOT downloadable at ${DL_BASE}/${VERSION}/fusion-dashboard"
chmod +x "$DL_BIN"
PUB_VER=$("$DL_BIN" --version 2>&1 | grep -oE 'v[0-9]+\.[0-9]+\.[0-9]+' | head -1 | tr -d v)
[ "$PUB_VER" = "$VERSION" ] || die "the PUBLISHED binary reports $PUB_VER, expected $VERSION (installer would deliver the wrong build)"
rm -rf "$TMP"
echo "  clean install DASH_VERSION = $VERSION ✓ (and the published binary reports $PUB_VER ✓)"

echo
echo "✅ LOCKSTEP RELEASE $VERSION complete: bridge + dashboard + package all at $VERSION, verified."
