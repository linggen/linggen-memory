#!/usr/bin/env bash
# Point the plugin's hooks at the eval's scratch Linggen, never the real 9527.
# The host is written as 0.0.0.0 (not 127.0.0.1) so autostart.sh treats it as
# another machine: nothing installed, nothing started.
# Port 29548 is fixed: scripts/plugin-eval.sh starts the scratch engine there.
set -eu
curl -sf --max-time 2 http://127.0.0.1:29548/api/health >/dev/null \
  || { echo "scratch Linggen not up on 29548 — run ./scripts/check.sh eval" >&2; exit 1; }
mkdir -p "$HOME/.linggen"
printf '{"ling":"http://0.0.0.0:29548"}\n' >"$HOME/.linggen/client.json"
# Workspace: a non-git start dir holding the lingjing repo with a story/ dir.
mkdir -p lingjing/story && git init -q lingjing && echo '# story' > lingjing/story/README.md
