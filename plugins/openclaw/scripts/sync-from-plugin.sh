#!/usr/bin/env bash
# Copy the shared parts of the Claude Code plugin into this package.
#
# The skill and the command runbooks have ONE source — `plugins/linggen/` —
# and three hosts read them. Copying rather than symlinking because npm pack
# follows the package directory, and a symlink out of it would publish a
# dangling path.
#
# Run after changing anything under plugins/linggen/, before publishing.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_root="$(cd "$here/../linggen" && pwd)"

echo "==> syncing from $source_root"

# The skill carries the Linggen installer its bootstrap.sh runs — no remote
# script execution at install time (a ClawScan review finding).
# install-engine.sh is vendored VERBATIM from linggensite/public/install.sh
# when that checkout is present (byte-identical on purpose — drift is an md5
# diff away).
site_install="$source_root/../../../linggensite/public/install.sh"
if [ -f "$site_install" ]; then
  cp "$site_install" "$source_root/skills/linggen/scripts/install-engine.sh"
  chmod +x "$source_root/skills/linggen/scripts/install-engine.sh"
fi

rm -rf "$here/skills" "$here/commands"
mkdir -p "$here/skills" "$here/commands"

cp -R "$source_root/skills/linggen" "$here/skills/linggen"
cp "$source_root"/commands/*.md "$here/commands/"

# The skill's own bootstrap detects its channel from its path. Under OpenClaw
# the plugin's skills are symlinked into ~/.openclaw/plugin-skills/, which its
# case arms already have to recognise — see scripts/bootstrap.sh in the skill.
echo "==> synced: $(find "$here/skills" -type f | wc -l | tr -d ' ') skill files, $(ls "$here/commands" | wc -l | tr -d ' ') commands"
