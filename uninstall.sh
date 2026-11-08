#!/bin/bash
SKILLS_DIR="$HOME/.claude/skills"
rm -rf "$SKILLS_DIR/adom-desktop-fusion-bridge" "$SKILLS_DIR/fusion-bridge"
for d in "$HOME/.claude/skills"/fusion-* "$HOME/.claude/skills/pcb-stackup"; do rm -rf "$d"; done
rm -f /usr/local/bin/fusion-dashboard "$HOME/.local/bin/fusion-dashboard"
echo "Uninstalled fusion-bridge skills + fusion-dashboard CLI"
