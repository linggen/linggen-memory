#!/usr/bin/env bash
# Copy the shared parts of the Claude Code plugin into this package.
#
# The skill, the command runbooks and the binary installer have ONE source —
# `plugins/linggen/` — and three hosts read them. Copying rather than
# symlinking because npm pack follows the package directory, and a symlink out
# of it would publish a dangling path.
#
# Run after changing anything under plugins/linggen/, before publishing.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_root="$(cd "$here/../linggen" && pwd)"

echo "==> syncing from $source_root"

# Vendor the installers INTO the skill first, so every artifact that carries
# bootstrap.sh also carries what it runs — no remote script execution at
# install time (a ClawScan review finding). install-bin.sh is canonical at
# plugins/linggen/scripts/; install-engine.sh is vendored VERBATIM from
# linggensite/public/install.sh when that checkout is present (byte-identical
# on purpose — drift is an md5 diff away).
cp "$source_root/scripts/install-bin.sh" "$source_root/skills/linggen/scripts/install-bin.sh"
chmod +x "$source_root/skills/linggen/scripts/install-bin.sh"
site_install="$source_root/../../../linggensite/public/install.sh"
if [ -f "$site_install" ]; then
  cp "$site_install" "$source_root/skills/linggen/scripts/install-engine.sh"
  chmod +x "$source_root/skills/linggen/scripts/install-engine.sh"
fi

rm -rf "$here/skills" "$here/commands"
mkdir -p "$here/skills" "$here/commands" "$here/scripts"

cp -R "$source_root/skills/linggen" "$here/skills/linggen"
cp "$source_root"/commands/*.md "$here/commands/"
cp "$source_root/scripts/install-bin.sh" "$here/scripts/install-bin.sh"
chmod +x "$here/scripts/install-bin.sh"

# The skill's own bootstrap detects its channel from its path. Under OpenClaw
# the plugin's skills are symlinked into ~/.openclaw/plugin-skills/, which its
# case arms already have to recognise — see scripts/bootstrap.sh in the skill.
echo "==> synced: $(find "$here/skills" -type f | wc -l | tr -d ' ') skill files, $(ls "$here/commands" | wc -l | tr -d ' ') commands, install-bin.sh"
