#!/usr/bin/env bash
#
# Build per-host plugin trees from plugins/shared-memory/skill/ + templates.
#
# Reads version from Cargo.toml and stamps it into both manifests.
# Run from the linggen-memory repo root.

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)"/\1/')"
SRC="plugins/shared-memory/skill"
TPL="plugins/shared-memory/templates"

[ -d "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }
[ -d "$TPL" ] || { echo "missing $TPL" >&2; exit 1; }

echo "build-plugin: version=$VERSION"

# ── Claude Code ──────────────────────────────────────────────────────────────
CC="plugins/shared-memory/cc"
rm -rf "$CC"
mkdir -p "$CC/.claude-plugin" "$CC/skills/shared-memory" "$CC/hooks"
sed "s/__VERSION__/$VERSION/g" "$TPL/cc.plugin.json.tmpl" > "$CC/.claude-plugin/plugin.json"
cp -R "$SRC/." "$CC/skills/shared-memory/"
# Hook scripts: move from inside skills/shared-memory/hooks → CC's plugin-level hooks/
[ -d "$CC/skills/shared-memory/hooks" ] && mv "$CC/skills/shared-memory/hooks/"* "$CC/hooks/" || true
rmdir "$CC/skills/shared-memory/hooks" 2>/dev/null || true

cat > "$CC/hooks/hooks.json" <<'JSON'
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/recall.sh" }] }
    ],
    "Stop": [
      { "hooks": [{ "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/encode.sh" }] }
    ]
  }
}
JSON

cat > "$CC/.mcp.json" <<'JSON'
{
  "mcpServers": {
    "ling-mem": {
      "type": "stdio",
      "command": "ling-mem",
      "args": ["mcp"]
    }
  }
}
JSON

cat > "$CC/README.md" <<'MD'
# shared-memory (Claude Code plugin)

Built from `plugins/shared-memory/skill/`. Edit there, not here — regenerate via:

    ./scripts/build-plugin.sh

Requires the `ling-mem` binary on PATH. Install via:

    curl -fsSL https://linggen.dev/install-ling-mem.sh | bash
MD

# ── Codex ────────────────────────────────────────────────────────────────────
CX="plugins/shared-memory/codex"
rm -rf "$CX"
mkdir -p "$CX/skills/shared-memory" "$CX/hooks" "$CX/assets"
sed "s/__VERSION__/$VERSION/g" "$TPL/codex.plugin.json.tmpl" > "$CX/plugin.json"
cp -R "$SRC/." "$CX/skills/shared-memory/"
[ -d "$CX/skills/shared-memory/hooks" ] && mv "$CX/skills/shared-memory/hooks/"* "$CX/hooks/" || true
rmdir "$CX/skills/shared-memory/hooks" 2>/dev/null || true

cat > "$CX/hooks/hooks.json" <<'JSON'
{
  "hooks": [
    { "event": "UserPromptSubmit", "command": "${PLUGIN_ROOT}/hooks/recall.sh" },
    { "event": "Stop",             "command": "${PLUGIN_ROOT}/hooks/encode.sh" }
  ]
}
JSON

cat > "$CX/.mcp.json" <<'JSON'
{
  "mcpServers": {
    "ling-mem": {
      "command": "ling-mem",
      "args": ["mcp"]
    }
  }
}
JSON

# Copy any host-marketing assets if present.
if [ -d "$TPL/assets" ] && [ -n "$(ls -A "$TPL/assets" 2>/dev/null)" ]; then
  cp -R "$TPL/assets/." "$CX/assets/"
fi

cat > "$CX/README.md" <<'MD'
# shared-memory (Codex plugin)

Built from `plugins/shared-memory/skill/`. Edit there, not here — regenerate via:

    ./scripts/build-plugin.sh

Requires the `ling-mem` binary on PATH. Install via:

    curl -fsSL https://linggen.dev/install-ling-mem.sh | bash
MD

echo "build-plugin: wrote $CC and $CX"
