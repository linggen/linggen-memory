#!/usr/bin/env bash
# bootstrap.sh — ensure Linggen (`ling`, the engine that also hosts memory) is
# installed. Run from SKILL.md's first-use gate; a no-op when `ling` is present.
#
# Only when Linggen lives on THIS machine. A host pointed at another machine's
# Linggen (~/.linggen/client.json, or LINGGEN_HOST) installs nothing: a local
# Linggen would open a second, empty store — memory forked in two.
#
# Install-channel labels: this script derives via/agent from WHERE THIS FILE
# IS RUNNING FROM — the skill dir's path is a fingerprint of the channel that
# delivered it, measured at run time. Never hardcode a channel here: this one
# file ships through ClawHub, the CC/Codex plugin, and skills.sh, so any
# static label would be wrong on the other channels. An unrecognized path
# labels nothing and the installer uses its own defaults. The labels only
# reach a local marker file (~/.linggen/.linggen-install-source) that the
# binary reports once on first launch; this script never POSTs anywhere.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Where Linggen lives, read the way the plugin hooks read it (client.json's
# `ling`, else LINGGEN_HOST). Plain text tools only — no jq on a fresh Mac.
client="${LINGGEN_DATA_DIR:-$HOME/.linggen}/client.json"
addr=""
[ -r "$client" ] && addr="$(tr -d '\n' <"$client" | sed -n 's/.*"ling"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
host="${addr#*://}"; host="${host%%/*}"; host="${host%:*}"
[ -n "$host" ] || host="${LINGGEN_HOST:-127.0.0.1}"
case "$host" in
  127.0.0.1|localhost|::1) ;;
  *) echo "bootstrap: this host uses the Linggen on $host — nothing to install here."; exit 0 ;;
esac

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
[ -n "$via" ]   && export LINGGEN_SOURCE="$via"
[ -n "$agent" ] && export LINGGEN_AGENT="$agent"

# The installer ships IN this bundle and LINGGEN_NO_REMOTE_SCRIPT=1 makes it
# refuse any curl fallback outright — this flow never executes a remotely
# fetched script. What does come over the network is the release binary,
# checked against its published SHA-256 (and bun, against bun's own sums).
export LINGGEN_NO_REMOTE_SCRIPT=1

if ! command -v ling >/dev/null 2>&1 && [ ! -x "$HOME/.local/bin/ling" ]; then
  bash "$here/install-engine.sh"
fi
