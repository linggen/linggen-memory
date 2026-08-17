#!/usr/bin/env bash
# bootstrap.sh — ensure the ling-mem CLI and the Linggen engine are installed.
# Run from SKILL.md's first-use gate; each install is a no-op when the binary
# is already present.
#
# Install-channel labels: this script derives via/agent from WHERE THIS FILE
# IS RUNNING FROM — the skill dir's path is a fingerprint of the channel that
# delivered it, measured at run time. Never hardcode a channel here: this one
# file ships through ClawHub, the CC/Codex plugin, and skills.sh, so any
# static label would be wrong on the other channels. An unrecognized path
# labels nothing and the installers use their own defaults. The labels only
# reach a local marker file (~/.linggen/.*-install-source) that the binary
# reports once on first launch; this script never POSTs anywhere.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
via="" agent=""
case "$here" in
  */.openclaw/workspace/skills/*) via=clawhub  agent=openclaw ;;
  # OpenClaw plugin skills: the manifest's `skills` entries are symlinked into
  # ~/.openclaw/plugin-skills/, and the package itself lands under extensions/
  # (local or archive install) or npm/ (ClawHub install) — three paths, one
  # channel. Whether $BASH_SOURCE resolves the symlink or not, one arm matches.
  */.openclaw/plugin-skills/*)    via=plugin   agent=openclaw ;;
  */.openclaw/extensions/*)       via=plugin   agent=openclaw ;;
  */.openclaw/npm/*)              via=plugin   agent=openclaw ;;
  */.claude/plugins/*)            via=plugin   agent=cc ;;
  */.codex/*)                     via=plugin   agent=codex ;;
  */.agents/skills/*)             via=skills-sh ;;   # shared dir — host unknowable
  */.claude/skills/*)             agent=cc ;;        # manual stub — door unknowable
esac
[ -n "$via" ]   && export LING_MEM_SOURCE="$via" LINGGEN_SOURCE="$via"
[ -n "$agent" ] && export LING_MEM_AGENT="$agent" LINGGEN_AGENT="$agent"

# Both installers ship IN this bundle and LINGGEN_NO_REMOTE_SCRIPT=1 makes
# install-engine.sh refuse its curl fallback outright — this flow never
# executes a remotely fetched script. What does come over the network are
# the release BINARIES: ling-mem is SHA-256-verified by install-bin.sh; the
# engine and bun binaries are fetched from GitHub releases over TLS without
# a checksum yet (release-manifest verification is on the roadmap).
export LINGGEN_NO_REMOTE_SCRIPT=1

# ling-mem — the memory backend. Installs to ~/.local/bin.
if ! command -v ling-mem >/dev/null 2>&1; then
  bash "$here/install-bin.sh" --version '^1'
fi

# Linggen engine — powers the browser/x/agent MCP tools. Optional: memory
# works with ling-mem alone, so a failure here is reported, not fatal.
if ! command -v ling >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/ling" ]; then
  bash "$here/install-engine.sh" \
    || echo "bootstrap: engine install failed (memory still works via ling-mem)" >&2
fi
