#!/usr/bin/env bash
set -euo pipefail
#
# install.sh — install the shared-memory skill.
#
# Layout (one canonical bundle, thin per-host SKILL.md stubs):
#
#   ~/.linggen/skills/shared-memory/   ← CANONICAL. All references/,
#                                        scripts/, hooks/, plus the
#                                        SKILL.md every host reads.
#   ~/.claude/skills/shared-memory/SKILL.md      ← copy of canonical
#   ~/.codex/skills/shared-memory/SKILL.md       ← copy of canonical
#   ~/.openclaw/skills/shared-memory/SKILL.md    ← copy of canonical
#
#   /usr/local/bin/ling-mem  (preferred)   ← the daemon binary, on PATH
#   ~/.local/bin/ling-mem    (fallback)      sits next to `ling`
#
# Why this shape:
# - Single source of truth — edit content once at ~/.linggen/skills/
#   shared-memory/ (e.g. by re-running install.sh, or by an update tool)
#   and every host sees the change. No per-host references/ to keep in
#   sync.
# - SKILL.md uses absolute paths (~/.linggen/skills/shared-memory/...)
#   for cross-tree reads, so the host's agent never needs the per-host
#   bundle to contain references/ or scripts/.
# - Binary on PATH at /usr/local/bin or ~/.local/bin — no per-host
#   `bin/ling-mem` copies, no PATH symlink dance.
#
# Supply-chain posture: defaults are pinned versions; SHA-256 verification
# is mandatory by default and can only be disabled by an explicit
# LING_MEM_SKIP_CHECKSUM=1 (not recommended). No data leaves the machine.
# No remote code execution beyond extracting tarballs published by the
# linggen org. Source + releases:
# https://github.com/linggen/linggen-memory
# https://github.com/linggen/skills

SOURCE_DIR="$(cd "$(dirname "$0")" && pwd 2>/dev/null)" || SOURCE_DIR=""
REPO="linggen/linggen-memory"

# Default version is pinned to a specific release. `latest` is allowed as
# an explicit opt-in (LING_MEM_VERSION=latest) but is not the default —
# unpinned defaults are flagged as a supply-chain weakness by skill
# scanners and create silent drift between install.sh and the release
# its checksum verifier expects.
VERSION="${LING_MEM_VERSION:-v0.7.1}"

# Canonical layout — one bundle, on disk under ~/.linggen.
CANONICAL_DIR="$HOME/.linggen/skills/shared-memory"

# Per-host stub destinations. The agent inside each host reads the
# host's <skills>/shared-memory/SKILL.md (a copy of canonical), which
# uses absolute paths back into CANONICAL_DIR for references / scripts.
HOST_DIRS=(
  "$HOME/.claude/skills/shared-memory"
  "$HOME/.codex/skills/shared-memory"
  "$HOME/.openclaw/skills/shared-memory"
)

# Self-bootstrap: when invoked via `curl ... | bash`, $0 is bash itself
# and $SOURCE_DIR ends up pointing at the user's cwd — there is no local
# clone with SKILL.md, scripts/, references/, etc. Detect this and fetch
# the canonical skill tree from GitHub into a temp dir, then re-target
# SOURCE_DIR.
if [ -z "$SOURCE_DIR" ] || [ ! -f "$SOURCE_DIR/SKILL.md" ]; then
  BOOTSTRAP_REPO="${LING_MEM_SKILLS_REPO:-linggen/skills}"
  # Pinned to a per-skill scoped tag so a `curl | bash` one-liner
  # fetches a known revision, not whatever main currently points at.
  # Users can override with LING_MEM_REPO_REF=main for HEAD.
  BOOTSTRAP_REF="${LING_MEM_REPO_REF:-shared-memory-v0.6.2}"
  BOOTSTRAP_URL="https://github.com/${BOOTSTRAP_REPO}/archive/${BOOTSTRAP_REF}.tar.gz"
  BOOTSTRAP_TMP="$(mktemp -d -t shared-memory-bootstrap-XXXXXX)"
  BOOTSTRAP_TAR="$BOOTSTRAP_TMP/skills.tar.gz"
  trap 'rm -rf "$BOOTSTRAP_TMP"' EXIT

  echo "Fetching shared-memory skill from ${BOOTSTRAP_REPO}@${BOOTSTRAP_REF}..."
  if ! curl -fsSL --retry 3 --retry-delay 2 "$BOOTSTRAP_URL" -o "$BOOTSTRAP_TAR"; then
    echo "Error: failed to download $BOOTSTRAP_URL" >&2
    exit 1
  fi

  echo "Extracting skill tree..."
  if ! tar -xzf "$BOOTSTRAP_TAR" -C "$BOOTSTRAP_TMP"; then
    echo "Error: failed to extract $BOOTSTRAP_TAR" >&2
    exit 1
  fi
  rm -f "$BOOTSTRAP_TAR"

  SOURCE_DIR="$(find "$BOOTSTRAP_TMP" -maxdepth 3 -type d -name shared-memory | head -n1)"
  if [ -z "$SOURCE_DIR" ] || [ ! -f "$SOURCE_DIR/SKILL.md" ]; then
    echo "Error: shared-memory/SKILL.md not found in tarball" >&2
    exit 1
  fi
fi

# -------------------------------------------------------------------
# Args
# -------------------------------------------------------------------

for arg in "$@"; do
  case "$arg" in
    -h|--help)
      cat <<EOF
Usage: install.sh

Installs the shared-memory skill, one canonical copy at
~/.linggen/skills/shared-memory/. Each detected host (~/.claude/,
~/.codex/, ~/.openclaw/) gets a thin SKILL.md stub that points back
to the canonical bundle.

Binary: /usr/local/bin/ling-mem if writable, otherwise
~/.local/bin/ling-mem (matches the \`ling\` install).

  LING_MEM_VERSION=vX.Y.Z    pin a specific binary version (default: ${VERSION})
                             use 'latest' for the most recent release
  LING_MEM_REPO_REF=<ref>    skills repo ref for curl|bash bootstrap
                             (default: shared-memory-v0.6.2)
  LING_MEM_SKIP_CHECKSUM=1   skip SHA256 verification (not recommended)
  LING_MEM_FORCE_DOWNLOAD=1  re-fetch the binary even if present
  LING_MEM_SKIP_CODEX=1      skip the Codex stub + sandbox wiring
  LING_MEM_SKIP_OPENCLAW=1   skip the OpenClaw USER.md directive
EOF
      exit 0
      ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# -------------------------------------------------------------------
# Platform → release asset slug
# -------------------------------------------------------------------

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$OS-$ARCH" in
  darwin-arm64|darwin-aarch64) TARGET="macos-aarch64" ;;
  darwin-x86_64|darwin-amd64)  TARGET="macos-x86_64" ;;
  linux-x86_64|linux-amd64)    TARGET="linux-x86_64" ;;
  linux-aarch64|linux-arm64)   TARGET="linux-aarch64" ;;
  *)
    echo "Error: unsupported platform $OS-$ARCH" >&2
    echo "  Manual install: https://github.com/${REPO}/releases" >&2
    exit 1
    ;;
esac

# -------------------------------------------------------------------
# Helpers — checksum, writability
# -------------------------------------------------------------------

sha256_check() {
  local checksum_file="$1" cwd="$2"
  if   command -v shasum    >/dev/null 2>&1; then ( cd "$cwd" && shasum -a 256 -c "$checksum_file" >/dev/null 2>&1 )
  elif command -v sha256sum >/dev/null 2>&1; then ( cd "$cwd" && sha256sum -c "$checksum_file" >/dev/null 2>&1 )
  else return 127
  fi
}

ensure_dir_writable() {
  local dir="$1"
  if [ ! -d "$dir" ]; then mkdir -p "$dir" 2>/dev/null || return 1; fi
  [ -w "$dir" ]
}

# -------------------------------------------------------------------
# Canonical bundle — every file lives once at ~/.linggen/skills/shared-memory/
# -------------------------------------------------------------------

install_canonical_bundle() {
  echo "Installing canonical bundle at $CANONICAL_DIR/"

  if [ "$SOURCE_DIR" = "$CANONICAL_DIR" ]; then
    echo "  Files already in place — skipping copy"
    return
  fi

  mkdir -p "$CANONICAL_DIR/scripts" \
           "$CANONICAL_DIR/references" \
           "$CANONICAL_DIR/hooks"

  install -m 0644 "$SOURCE_DIR/SKILL.md"  "$CANONICAL_DIR/SKILL.md"
  install -m 0644 "$SOURCE_DIR/index.html" "$CANONICAL_DIR/index.html"
  install -m 0644 "$SOURCE_DIR/LICENSE"    "$CANONICAL_DIR/LICENSE"

  for ref in routing-rules.md dream-flow.md dashboard.md extractor-prompt.md; do
    install -m 0644 "$SOURCE_DIR/references/$ref" "$CANONICAL_DIR/references/$ref"
  done

  for f in collect.sh collect_sessions.sh extract_session.sh; do
    install -m 0755 "$SOURCE_DIR/scripts/$f" "$CANONICAL_DIR/scripts/$f"
  done
  for f in api.js chat-bridge.js memory-app.js memory.css memory.html \
           page-renderer.js style.css widget-renderers.js; do
    install -m 0644 "$SOURCE_DIR/scripts/$f" "$CANONICAL_DIR/scripts/$f"
  done

  install -m 0755 "$SOURCE_DIR/hooks/recall.sh" "$CANONICAL_DIR/hooks/recall.sh"
}

# -------------------------------------------------------------------
# Per-host SKILL.md stub — copies of the canonical SKILL.md into each
# detected host's skills dir. The SKILL.md body uses absolute paths
# into ~/.linggen/skills/shared-memory/ so the agent finds references
# and scripts regardless of which host's discovery surfaced the file.
# -------------------------------------------------------------------

install_host_stub() {
  local host_dir="$1"
  local parent; parent="$(dirname "$host_dir")"   # ~/.<host>/skills

  # Only install when the host's home is present — never auto-create
  # ~/.claude/ etc. on a machine that doesn't have that host.
  local host_home; host_home="$(dirname "$parent")"   # ~/.<host>
  [ -d "$host_home" ] || return 0

  mkdir -p "$host_dir"
  install -m 0644 "$CANONICAL_DIR/SKILL.md" "$host_dir/SKILL.md"
  echo "  Stub: $host_dir/SKILL.md"
}

install_host_stubs() {
  echo "Installing per-host SKILL.md stubs..."
  local installed=0
  for d in "${HOST_DIRS[@]}"; do
    # Honor opt-outs.
    case "$d" in
      */.codex/*)    [ "${LING_MEM_SKIP_CODEX:-0}"    = "1" ] && continue ;;
      */.openclaw/*) [ "${LING_MEM_SKIP_OPENCLAW:-0}" = "1" ] && continue ;;
    esac
    if install_host_stub "$d"; then
      [ -f "$d/SKILL.md" ] && installed=$((installed + 1))
    fi
  done
  if [ "$installed" -eq 0 ]; then
    echo "  (no host runtimes detected — canonical bundle still installed)"
  fi
}

# -------------------------------------------------------------------
# Binary install — /usr/local/bin/ling-mem if writable, else
# ~/.local/bin/ling-mem. Same logic the `ling` installer uses, so the
# two binaries land in the same directory by default. SHA-256
# verified against the release's sibling .sha256 file.
# -------------------------------------------------------------------

BIN_DIR=""

choose_bin_dir() {
  if ensure_dir_writable "/usr/local/bin"; then
    BIN_DIR="/usr/local/bin"
  elif ensure_dir_writable "$HOME/.local/bin"; then
    BIN_DIR="$HOME/.local/bin"
  else
    echo "Error: neither /usr/local/bin nor ~/.local/bin is writable" >&2
    exit 1
  fi
  echo "Binary install dir: $BIN_DIR"
}

download_binary() {
  local bin="$BIN_DIR/ling-mem"

  if [ -x "$bin" ] \
     && [ "${LING_MEM_FORCE_DOWNLOAD:-}" != "1" ] \
     && [ "$VERSION" = "latest" ]; then
    echo "  ling-mem already at $bin — skipping download"
    return
  fi

  local asset="ling-mem-${TARGET}.tar.gz"
  local base
  if [ "$VERSION" = "latest" ]; then
    base="https://github.com/${REPO}/releases/latest/download"
  else
    base="https://github.com/${REPO}/releases/download/${VERSION}"
  fi
  local url="${base}/${asset}"
  local sum_url="${base}/${asset}.sha256"

  local tmp_dir; tmp_dir="$(mktemp -d -t "ling-mem-dl-XXXXXX")"
  trap 'rm -rf "$tmp_dir"' RETURN
  local tmp_tar="$tmp_dir/$asset"
  local tmp_sum="$tmp_dir/${asset}.sha256"

  echo "  Downloading ling-mem ${VERSION} (${TARGET})"
  if ! curl -fsSL --retry 3 --retry-delay 2 "$url" -o "$tmp_tar"; then
    echo "Error: download failed. See https://github.com/${REPO}/releases" >&2
    exit 1
  fi

  if [ "${LING_MEM_SKIP_CHECKSUM:-0}" = "1" ]; then
    echo "  Warning: SHA256 verification disabled by LING_MEM_SKIP_CHECKSUM=1" >&2
  else
    if ! curl -fsSL --retry 3 --retry-delay 2 "$sum_url" -o "$tmp_sum"; then
      echo "Error: failed to fetch checksum from $sum_url" >&2
      echo "  Override with LING_MEM_SKIP_CHECKSUM=1 (not recommended)." >&2
      exit 1
    fi
    if ! sha256_check "${asset}.sha256" "$tmp_dir"; then
      local rc=$?
      if [ "$rc" -eq 127 ]; then
        echo "Error: neither shasum nor sha256sum found on this system." >&2
      else
        echo "Error: SHA256 verification failed for $asset" >&2
        sed 's/^/    /' "$tmp_sum" >&2
      fi
      exit 1
    fi
    echo "  Verified: SHA256 matches $sum_url"
  fi

  tar -xzf "$tmp_tar" -C "$BIN_DIR" ling-mem
  chmod +x "$bin"

  local built_ver
  built_ver="$("$bin" --version 2>/dev/null | awk '{print $2}' || true)"
  if [ "$VERSION" != "latest" ]; then
    local expected="${VERSION#v}"
    if [ "$built_ver" != "$expected" ]; then
      echo "  Warning: binary reports '$built_ver', expected '$expected'." >&2
    fi
  fi
  echo "  Installed: $bin (${built_ver:-unknown})"

  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
      echo "  Note: $BIN_DIR is not on PATH. Add it to your shell rc, e.g.:"
      echo "    echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.zshrc"
      ;;
  esac
}

# -------------------------------------------------------------------
# Memory dir + telemetry marker
# -------------------------------------------------------------------
#
# Note: `dream` ships as a `/shared-memory dream` slash command, not a
# cron mission. Missions are owned by the engine (Linggen seeds its own
# built-in `~/.linggen/missions/dream/`); skills should not install
# missions. See the design doc for the rationale.

seed_memory_dir() {
  mkdir -p "$HOME/.linggen/memory"
}

write_install_source_marker() {
  local via="${LING_MEM_SOURCE:-wrapper}"
  local marker="$HOME/.linggen/.ling-mem-install-source"
  mkdir -p "$HOME/.linggen"
  {
    printf 'via=%s\n' "$via"
    printf 'installer_version=%s\n' "$VERSION"
    printf 'installed_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$marker"
}

# -------------------------------------------------------------------
# Claude Code wiring — CLAUDE.md @-imports + settings.json hook entry
# -------------------------------------------------------------------

configure_claude_md() {
  [ -d "$HOME/.claude" ] || return 0
  local claude_md="${CLAUDE_MD:-$HOME/.claude/CLAUDE.md}"
  local marker_start="<!-- ling-mem:core-start -->"
  local marker_end="<!-- ling-mem:core-end -->"

  local block="$marker_start
## Who I am · How I work

Core memory lives in the \`ling-mem\` daemon's \`semantic\` table with
\`tier=core\` — narrow universals about the person + standing-instruction
preferences. **At the start of every session**, load it:

\`\`\`
ling-mem list --tier core --limit 100 --format json | jq -c 'del(.vector)'
\`\`\`

## Memory

Durable signal lives in the \`ling-mem\` daemon (semantic + episodic
tables). The \`shared-memory\` skill at \`~/.linggen/skills/shared-memory/\`
holds the canonical SKILL.md, references/, scripts/; each host's
\`skills/shared-memory/SKILL.md\` is a stub copy pointing back here.
For ad-hoc retrieval, call
\`ling-mem search \"<query>\" --format json | jq -c 'del(.vector)'\`
**before** answering when the user's question could connect to past
preferences / decisions / gotchas. Mention relevant hits inline —
*\"From memory: you prefer X …\"*. The \`del(.vector)\` filter is
mandatory — raw output includes 1024-dim embeddings (Qwen3-Embedding-0.6B)
that blow up context.
$marker_end"

  mkdir -p "$(dirname "$claude_md")"
  touch "$claude_md"

  local tmp_md; tmp_md="$(mktemp -t "claude-md-XXXXXX")"
  awk -v s_new="$marker_start" -v e_new="$marker_end" \
      -v s_old="<!-- linggen-memory:core-start -->" \
      -v e_old="<!-- linggen-memory:core-end -->" '
    BEGIN            { skip = 0; blanks = 0 }
    $0 == s_new      { skip = 1; next }
    $0 == s_old      { skip = 1; next }
    $0 == e_new      { skip = 0; next }
    $0 == e_old      { skip = 0; next }
    skip             { next }
    /^[[:space:]]*$/ { blanks++; next }
                     { while (blanks--) print ""; blanks = 0; print }
  ' "$claude_md" > "$tmp_md"
  if [ -s "$tmp_md" ]; then
    printf '\n%s\n' "$block" >> "$tmp_md"
  else
    printf '%s\n' "$block" > "$tmp_md"
  fi
  mv "$tmp_md" "$claude_md"
  echo "  Updated: $claude_md (core directive)"
}

configure_claude_hook() {
  [ -d "$HOME/.claude" ] || return 0
  local hook="$CANONICAL_DIR/hooks/recall.sh"
  local settings="${CLAUDE_SETTINGS:-$HOME/.claude/settings.json}"

  command -v python3 >/dev/null 2>&1 || {
    echo "  Warning: python3 not found — skipping settings.json patch" >&2
    echo "    Manually add to $settings:" >&2
    echo "      hooks.UserPromptSubmit += [{ matcher: \"\", hooks: [{ type: \"command\", command: \"$hook\" }] }]" >&2
    return 0
  }

  python3 - "$settings" "$hook" <<'PY'
import json, os, sys
path, hook_cmd = sys.argv[1], sys.argv[2]
data = {}
if os.path.exists(path):
    try:
        with open(path) as f:
            data = json.load(f) or {}
    except Exception:
        data = {}
hooks_root = data.setdefault("hooks", {})
ups = hooks_root.setdefault("UserPromptSubmit", [])

def is_ours(h):
    if not isinstance(h, dict):
        return False
    if h.get("_lingMemRecall"):
        return True
    if h.get("command") == hook_cmd:
        return True
    for inner in (h.get("hooks") or []):
        if isinstance(inner, dict) and inner.get("command") == hook_cmd:
            return True
        # Catch stale entries that pointed at the old per-host bundle path.
        if isinstance(inner, dict):
            cmd = inner.get("command") or ""
            if cmd.endswith("/hooks/recall.sh"):
                return True
    return False

ups[:] = [h for h in ups if not is_ours(h)]
ups.append({
    "_lingMemRecall": True,
    "matcher": "",
    "hooks": [{"type": "command", "command": hook_cmd}],
})
os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path, "w") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
  echo "  Hook registered: $settings (UserPromptSubmit → $hook)"
}

# -------------------------------------------------------------------
# Codex wiring — features flag, sandbox writable_roots, PATH (if
# binary went to ~/.local/bin), hooks.json
# -------------------------------------------------------------------

configure_codex_features() {
  local codex_toml="${CODEX_CONFIG:-$HOME/.codex/config.toml}"
  mkdir -p "$(dirname "$codex_toml")"
  touch "$codex_toml"

  if grep -qE '^\s*codex_hooks\s*=' "$codex_toml"; then
    return 0
  fi

  if grep -qE '^\[features\]' "$codex_toml"; then
    local tmp; tmp="$(mktemp)"
    awk '
      BEGIN { inserted=0 }
      /^\[features\]/ && !inserted {
        print
        print "codex_hooks = true   # added by shared-memory install.sh"
        inserted=1
        next
      }
      { print }
    ' "$codex_toml" > "$tmp"
    mv "$tmp" "$codex_toml"
    echo "  Codex features: codex_hooks = true"
    return 0
  fi

  local tmp; tmp="$(mktemp)"
  awk '
    /^# BEGIN ling-mem features$/ { skip=1; next }
    /^# END ling-mem features$/   { skip=0; next }
    !skip { print }
  ' "$codex_toml" > "$tmp"

  cat >> "$tmp" <<TOML

# BEGIN ling-mem features
[features]
codex_hooks = true
# END ling-mem features
TOML

  mv "$tmp" "$codex_toml"
  echo "  Codex features: codex_hooks = true"
}

configure_codex_sandbox() {
  local codex_toml="${CODEX_CONFIG:-$HOME/.codex/config.toml}"
  local linggen_root="$HOME/.linggen"

  mkdir -p "$(dirname "$codex_toml")"
  touch "$codex_toml"

  local tmp; tmp="$(mktemp)"
  awk '
    /^# BEGIN ling-mem sandbox$/ { skip=1; next }
    /^# END ling-mem sandbox$/   { skip=0; next }
    !skip { print }
  ' "$codex_toml" > "$tmp"

  if grep -qE '^\[sandbox_workspace_write' "$tmp"; then
    rm -f "$tmp"
    echo "  Warning: $codex_toml already has a [sandbox_workspace_write] block." >&2
    echo "    Add this entry to its writable_roots manually:" >&2
    echo "      writable_roots = [\"$linggen_root\"]" >&2
    return 0
  fi

  cat >> "$tmp" <<TOML

# BEGIN ling-mem sandbox
[sandbox_workspace_write]
writable_roots = ["$linggen_root"]
# END ling-mem sandbox
TOML

  mv "$tmp" "$codex_toml"
  echo "  Codex sandbox: writable_roots += $linggen_root"
}

# Only wire PATH if the binary went somewhere Codex's default sandbox
# PATH doesn't already cover. /usr/local/bin is on every sane PATH;
# ~/.local/bin sometimes isn't (especially on macOS GUI launches).
configure_codex_env() {
  [ "$BIN_DIR" = "/usr/local/bin" ] && return 0

  local codex_toml="${CODEX_CONFIG:-$HOME/.codex/config.toml}"
  mkdir -p "$(dirname "$codex_toml")"
  touch "$codex_toml"

  local tmp; tmp="$(mktemp)"
  awk '
    /^# BEGIN ling-mem env$/ { skip=1; next }
    /^# END ling-mem env$/   { skip=0; next }
    !skip { print }
  ' "$codex_toml" > "$tmp"

  if grep -qE '^\[shell_environment_policy' "$tmp"; then
    rm -f "$tmp"
    echo "  Warning: $codex_toml already has a [shell_environment_policy] block." >&2
    echo "    Add this entry to [shell_environment_policy.set] manually:" >&2
    echo "      PATH = \"$BIN_DIR:<your existing PATH>\"" >&2
    return 0
  fi

  local new_path="$PATH"
  case ":$new_path:" in
    *":$BIN_DIR:"*) ;;
    *) new_path="$BIN_DIR:$new_path" ;;
  esac

  cat >> "$tmp" <<TOML

# BEGIN ling-mem env
[shell_environment_policy.set]
PATH = "$new_path"
# END ling-mem env
TOML

  mv "$tmp" "$codex_toml"
  echo "  Codex env: PATH includes $BIN_DIR"
}

configure_codex_hook() {
  local hook="$CANONICAL_DIR/hooks/recall.sh"
  local hooks_json="${CODEX_HOOKS:-$HOME/.codex/hooks.json}"

  mkdir -p "$(dirname "$hooks_json")"

  if [ -f "$hooks_json" ] && ! grep -q "ling-mem\|shared-memory" "$hooks_json" 2>/dev/null; then
    echo "  Warning: $hooks_json exists and is not ling-mem-managed." >&2
    echo "    Add this UserPromptSubmit entry manually:" >&2
    echo "      { \"type\": \"command\", \"command\": \"$hook\" }" >&2
    return 0
  fi

  cat > "$hooks_json" <<JSON
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "$hook"
          }
        ]
      }
    ]
  }
}
JSON

  echo "  Codex hook registered: $hooks_json (UserPromptSubmit → $hook)"
}

# -------------------------------------------------------------------
# OpenClaw wiring — USER.md directive append (no hook surface in OC)
# -------------------------------------------------------------------

configure_openclaw_user_md() {
  local user_md="$HOME/.openclaw/workspace/USER.md"
  [ -f "$user_md" ] || return 0

  local marker_start="<!-- ling-mem:openclaw-start -->"
  local marker_end="<!-- ling-mem:openclaw-end -->"

  local block="$marker_start
## Memory — \`ling-mem\` is your second memory system

Alongside \`MEMORY.md\` (workspace-scoped, narrative), you have access to
\`ling-mem\` — a shared RAG store at \`~/.linggen/memory/\`, used across
Claude Code, Linggen, Codex, and OpenClaw. The \`shared-memory\` skill
ships its canonical bundle at \`~/.linggen/skills/shared-memory/\`; the
SKILL.md you see in this host is a thin stub pointing back there.

**Search before answering** when the question could connect to past
preferences / decisions / gotchas:

\`\`\`
ling-mem search \"<query>\" --format json | jq -c 'del(.vector)'
\`\`\`

Mention hits inline — *\"From memory: you prefer X …\"*. The
\`del(.vector)\` filter is mandatory — raw output includes 1024-dim
embeddings (Qwen3-Embedding-0.6B) that blow up context.

**Save high-signal user statements** per the routing rules in the
canonical SKILL.md. Same security boundary as \`MEMORY.md\` — don't
search or share \`ling-mem\` content in group chats / shared contexts.
$marker_end"

  local tmp_md; tmp_md="$(mktemp -t "openclaw-user-md-XXXXXX")"
  awk -v s="$marker_start" -v e="$marker_end" '
    BEGIN            { skip = 0; blanks = 0 }
    $0 == s          { skip = 1; next }
    $0 == e          { skip = 0; next }
    skip             { next }
    /^[[:space:]]*$/ { blanks++; next }
                     { while (blanks--) print ""; blanks = 0; print }
  ' "$user_md" > "$tmp_md"
  if [ -s "$tmp_md" ]; then
    printf '\n%s\n' "$block" >> "$tmp_md"
  else
    printf '%s\n' "$block" > "$tmp_md"
  fi
  mv "$tmp_md" "$user_md"
  echo "  Updated: $user_md (shared-memory usage directive)"
}

# -------------------------------------------------------------------
# Main
# -------------------------------------------------------------------

install_canonical_bundle
choose_bin_dir
download_binary
seed_memory_dir
write_install_source_marker
install_host_stubs

# Host-specific wiring — runs only when the host's home exists.
if [ -d "$HOME/.claude" ]; then
  echo "Wiring Claude Code..."
  configure_claude_md
  configure_claude_hook
fi

if [ -d "$HOME/.codex" ] && [ "${LING_MEM_SKIP_CODEX:-0}" != "1" ]; then
  echo "Wiring Codex..."
  configure_codex_features
  configure_codex_sandbox
  configure_codex_env
  configure_codex_hook
fi

if [ "${LING_MEM_SKIP_OPENCLAW:-0}" != "1" ]; then
  configure_openclaw_user_md
fi

echo ""
echo "Done. Canonical bundle: $CANONICAL_DIR"
echo "      Binary:           $BIN_DIR/ling-mem"
echo ""
echo "Browse / edit rows: run 'ling-mem start' then open http://127.0.0.1:9888"
echo ""
echo "Note: ling-mem sends anonymous usage pings (install, daily-active, command"
echo "      name) to help improve it. No content, no identity. Disable any time:"
echo "        touch ~/.linggen/no-telemetry"
