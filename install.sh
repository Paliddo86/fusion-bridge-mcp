#!/bin/bash
# Install the Adom Desktop Fusion 360 Bridge's Claude skills from THIS repo (the
# single source of truth) into the user's skills dir. App-specific skills live
# ONLY here, in the app's repo.
set -e
PKG_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILLS_DIR="$HOME/.claude/skills"
# 2026-08 rename (#24): the page skill now lives under the NEW slug; sweep the old dir
rm -rf "$SKILLS_DIR/adom-desktop-fusion-bridge"
mkdir -p "$SKILLS_DIR/fusion-bridge"
[ -f "$PKG_DIR/SKILL.md" ] && cp "$PKG_DIR/SKILL.md" "$SKILLS_DIR/fusion-bridge/SKILL.md"
for skill_dir in "$PKG_DIR/skills"/*/; do
  [ -d "$skill_dir" ] || continue
  name=$(basename "$skill_dir")
  mkdir -p "$SKILLS_DIR/$name"
  cp -r "$skill_dir"* "$SKILLS_DIR/$name/"
done
echo "Installed fusion-bridge skills: $(ls "$PKG_DIR/skills" 2>/dev/null | tr '\n' ' ')"

# The Dashboard CLI (the bridge's Launch Page server) - first container-side bridge
# binary (the three-artifact pattern: bridge zip -> ab, THIS pkg -> container skills +
# this download, page media -> wiki). Per the wiki's PKG_VS_RELEASE policy, binaries are
# platform-tagged RELEASE assets, never tarball content - the installer fetches them.
DASH_VERSION="1.13.84"
DASH_URL="https://wiki.adom.inc/download/adom/fusion-bridge/${DASH_VERSION}/fusion-dashboard"
DEST=""
if [ -w /usr/local/bin ]; then DEST=/usr/local/bin
else mkdir -p "$HOME/.local/bin"; DEST="$HOME/.local/bin"; fi
if curl -fsSL -o "$DEST/fusion-dashboard.tmp" "$DASH_URL"; then
  mv "$DEST/fusion-dashboard.tmp" "$DEST/fusion-dashboard"
  chmod +x "$DEST/fusion-dashboard"
  echo "Installed fusion-dashboard CLI -> $DEST/fusion-dashboard ($("$DEST/fusion-dashboard" health 2>/dev/null || echo 'installed'))"
  # SAY IT WHEN THE BINARY IS INSTALLED BUT INVISIBLE (page issue #37). /usr/local/bin is
  # not writable in a workspace container, so we land in ~/.local/bin - which is NOT on
  # PATH in a clean process. The install then "succeeds" while `command -v fusion-dashboard`
  # reports nothing, which is exactly how the Hydrogen dock step failed on every real
  # machine while passing on the dev box (where the binary had been placed by hand).
  if ! command -v fusion-dashboard >/dev/null 2>&1; then
    echo "NOTE: $DEST is not on PATH, so 'fusion-dashboard' will not resolve by name."
    echo "      Run it as $DEST/fusion-dashboard, or add to your shell profile:"
    echo "        export PATH=\"\$HOME/.local/bin:\$PATH\""
    echo "      (dock.json already resolves it this way, so the Hydrogen dock is unaffected.)"
  fi
else
  rm -f "$DEST/fusion-dashboard.tmp"
  echo "WARN: fusion-dashboard download failed ($DASH_URL) - the Dashboard is unavailable until 'adom-wiki pkg update' retries it"
fi

# KEEP THE DASHBOARD SERVING ACROSS RESTARTS (page issue #121, Muzzer 2026-10-17): after a
# container restart the Dashboard tab read "Its server is not running", because only the dock
# button starts `serve` and nothing brings it back. Where systemd runs and sudo needs no
# password, install a unit like adom-shotlog's (Restart=always). The dock's own launch step
# finds this instance on its port and reuses it. A reinstall restarts it onto the new binary.
if [ -x "$DEST/fusion-dashboard" ] && [ -d /run/systemd/system ] && sudo -n true 2>/dev/null; then
  UNIT=/etc/systemd/system/adom-fusion-dashboard.service
  sudo -n tee "$UNIT" >/dev/null <<UNIT_EOF
[Unit]
Description=Adom Fusion Dashboard (the fusion-bridge Launch Page server)
After=network-online.target

[Service]
Type=simple
User=$(id -un)
Environment=HOME=$HOME
ExecStart=/bin/bash -lc 'exec $DEST/fusion-dashboard serve --port 8930 --ai-thread fusion-dashboard'
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
UNIT_EOF
  # a dock-launched server (setsid, outside the unit) holds port 8930 and would push the service
  # onto 8931 as a second copy: stop THIS binary's serve processes only, then let the unit own it
  pkill -f "^$DEST/fusion-dashboard serve" 2>/dev/null || true
  sudo -n systemctl daemon-reload \
    && sudo -n systemctl enable adom-fusion-dashboard.service >/dev/null 2>&1 \
    && sudo -n systemctl restart adom-fusion-dashboard.service \
    && echo "fusion-dashboard runs as a service (adom-fusion-dashboard.service): it comes back after a restart" \
    || echo "WARN: could not enable adom-fusion-dashboard.service; the dock button still starts the Dashboard"
else
  echo "NOTE: no systemd here (or sudo needs a password), so the Dashboard starts from the dock button and does not come back on its own after a restart"
fi
