set -u
IN=/mnt/c/tmp/partaudit
OUT=/mnt/c/tmp/lbrout
mkdir -p "$OUT"
for f in "$IN"/*.kicad_sym; do
  p=$(basename "$f" .kicad_sym)
  fp="$IN/$p.kicad_mod"
  [ -f "$fp" ] || { echo "$p SKIP no_footprint"; continue; }
  if adom-lbr generate --sym "$f" --fp "$fp" -o "$OUT/$p.lbr" >/dev/null 2>"$OUT/$p.err"; then
    v=$(adom-lbr validate "$OUT/$p.lbr" 2>&1 | tr '\n' ' ' | cut -c1-90)
    echo "$p GENERATED | $v"
  else
    echo "$p FAILED | $(head -c 90 "$OUT/$p.err")"
  fi
done
