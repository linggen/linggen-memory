#!/usr/bin/env bash
# Backfill `cwd` onto rows written before the field was stamped.
#
# The join key is already on the row: `source_session`. A Claude Code session
# log records its own cwd per entry; a Linggen session records it in
# session.yaml. Neither is a guess — both are the host's own record of where
# the work was.
#
# Only ever FILLS A NULL. A row that already carries a cwd is left alone, so
# this is re-runnable and cannot rewrite history it did not author.
#
# Written for bash 3.2 (macOS system bash): no associative arrays. The
# session→cwd map is built once into a TSV and joined with awk.
#
# Dry run by default. Pass --apply to write.

set -uo pipefail
API="http://127.0.0.1:9528"
APPLY=0
[ "${1:-}" = "--apply" ] && APPLY=1

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# ── 1. session id → cwd, from each host's own record ────────────────────────
: > "$work/map.tsv"

for log in "$HOME"/.claude/projects/*/*.jsonl; do
  [ -f "$log" ] || continue
  sid="$(basename "$log" .jsonl)"
  # First non-null wins: where the session started, before any cd into a
  # subdirectory. grep -m1 stops at the first hit instead of reading the
  # whole transcript.
  cwd="$(grep -m1 -o '"cwd":"[^"]*"' "$log" 2>/dev/null | head -1 | sed 's/^"cwd":"//; s/"$//')"
  [ -n "$cwd" ] && printf '%s\t%s\n' "$sid" "$cwd" >> "$work/map.tsv"
done

for meta in "$HOME"/.linggen/sessions/*/session.yaml; do
  [ -f "$meta" ] || continue
  sid="$(basename "$(dirname "$meta")")"
  cwd="$(awk '/^cwd:/ {sub(/^cwd:[[:space:]]*/, ""); print; exit}' "$meta" 2>/dev/null)"
  [ -n "$cwd" ] && printf '%s\t%s\n' "$sid" "$cwd" >> "$work/map.tsv"
done

echo "sessions on disk: $(wc -l < "$work/map.tsv" | tr -d ' ')"

# ── 2. rows needing a cwd, joined against that map ──────────────────────────
ling-mem export - > "$work/sem.ndjson" 2>/dev/null
ling-mem --episodic export - > "$work/epi.ndjson" 2>/dev/null

: > "$work/todo.tsv"
for table in sem epi; do
  epi_flag=false
  [ "$table" = "epi" ] && epi_flag=true
  jq -r 'select((.cwd // "") == "" and (.source_session // "") != "")
         | "\(.id)\t\(.source_session)"' "$work/$table.ndjson" 2>/dev/null \
  | awk -v FS='\t' -v OFS='\t' -v epi="$epi_flag" -v home="$HOME" '
      NR == FNR { seen[$1] = $2; next }
      {
        if (!($2 in seen)) next
        p = seen[$2]
        # A scope that is not a project HIDES the row instead of placing it:
        # $HOME is the parent of every project (so a project-scoped search
        # would not match it), and a temp dir is nobody. Leaving cwd null
        # keeps the row global, which is the honest answer for both.
        if (p == home) next
        if (p ~ /^\/private\/tmp/ || p ~ /^\/tmp/ || p ~ /^\/var\/folders/) next
        print $1, p, epi
      }
    ' "$work/map.tsv" - >> "$work/todo.tsv"
done

resolvable="$(wc -l < "$work/todo.tsv" | tr -d ' ')"
needed="$(cat "$work/sem.ndjson" "$work/epi.ndjson" \
          | jq -r 'select((.cwd // "") == "") | .id' 2>/dev/null | wc -l | tr -d ' ')"

echo "rows without cwd: $needed"
echo "resolvable:       $resolvable"
echo
echo "would set (top paths):"
cut -f2 "$work/todo.tsv" | sort | uniq -c | sort -rn | head -8

if [ "$APPLY" != "1" ]; then
  echo
  echo "dry run — pass --apply to write"
  exit 0
fi

# ── 3. write ────────────────────────────────────────────────────────────────
echo
filled=0; failed=0
while IFS=$'\t' read -r id cwd epi; do
  [ -z "$id" ] && continue
  code="$(curl -s -o /dev/null -w '%{http_code}' -X POST "$API/api/memory/update" \
    -H 'content-type: application/json' \
    -d "$(jq -nc --arg i "$id" --arg c "$cwd" --argjson e "$epi" \
            '{id:$i, cwd:$c, episodic:$e}')")"
  if [ "$code" = "200" ]; then
    filled=$((filled + 1))
    [ $((filled % 50)) -eq 0 ] && echo "  … $filled"
  else
    echo "  !! $id -> HTTP $code" >&2
    failed=$((failed + 1))
  fi
done < "$work/todo.tsv"

echo
echo "filled $filled"
[ "$failed" -gt 0 ] && echo "failed $failed"
exit 0
