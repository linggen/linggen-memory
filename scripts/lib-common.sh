#!/bin/bash
# Shared helpers for release and packaging scripts.
#
# Patterned on the linggen repo's scripts/lib-common.sh so the two projects
# share signing and platform-detection conventions.

# Detect the current platform and echo a slug used in artifact names.
# Supported: macos-aarch64, macos-x86_64, linux-x86_64, linux-aarch64.
detect_platform() {
  local OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
  local ARCH="$(uname -m)"
  local SLUG=""
  case "$OS" in
    darwin)
      case "$ARCH" in
        arm64|aarch64) SLUG="macos-aarch64" ;;
        x86_64|amd64)  SLUG="macos-x86_64" ;;
        *) echo "Unsupported macOS arch: $ARCH" >&2; return 1 ;;
      esac ;;
    linux)
      case "$ARCH" in
        x86_64|amd64)  SLUG="linux-x86_64" ;;
        arm64|aarch64) SLUG="linux-aarch64" ;;
        *) echo "Unsupported Linux arch: $ARCH" >&2; return 1 ;;
      esac ;;
    *)
      echo "Unsupported OS: $OS" >&2; return 1 ;;
  esac
  echo "$SLUG"
}

# Sign <file_path> with minisign using the shared Linggen key. Echoes the
# base64-encoded signature on success, returns non-zero (silently) on
# any failure — callers decide whether to hard-error or skip signing.
#
# Usage: sign_file <file_path> <root_dir>
sign_file() {
  local file_path="$1"
  local root_dir="$2"
  local sig_file="${file_path}.sig"

  if ! command -v minisign >/dev/null 2>&1; then
    echo "minisign not found. Skipping signing." >&2
    return 1
  fi

  # Read password from config file or environment variable.
  local password=""
  local config_file="$root_dir/.signing.conf"
  if [ -f "$config_file" ]; then
    password=$(grep -E "^SIGNING_KEY_PASSWORD=" "$config_file" \
      | grep -v "^#" \
      | cut -d'=' -f2- \
      | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' \
      | tr -d '"' | tr -d "'")
  fi
  password="${password:-${SIGNING_KEY_PASSWORD:-}}"

  # Locate the signing key. Prefer a repo-local `linggen.key`, then the
  # shared `~/.linggen/linggen.key` the main Linggen repo uses too.
  local key_file=""
  if [ -f "$root_dir/linggen.key" ]; then
    key_file="$root_dir/linggen.key"
  elif [ -f "$HOME/.linggen/linggen.key" ]; then
    key_file="$HOME/.linggen/linggen.key"
  else
    echo "No signing key found. Set SIGNING_KEY_PATH or create ~/.linggen/linggen.key" >&2
    return 1
  fi

  if [ -n "$password" ]; then
    echo "$password" | minisign -S -s "$key_file" -m "$file_path" -x "$sig_file" >/dev/null 2>&1
  else
    minisign -S -s "$key_file" -m "$file_path" -x "$sig_file" >/dev/null 2>&1
  fi

  if [ -f "$sig_file" ]; then
    base64 -i "$sig_file" | tr -d '\n'
  else
    echo "Signing failed for $file_path" >&2
    return 1
  fi
}

# Rewrite `version = "X.Y.Z"` in the crate's Cargo.toml so the built binary
# matches the release tag. Portable across BSD/GNU sed.
#
# Usage: sync_cargo_version <version_num> <cargo_toml>
sync_cargo_version() {
  local version="$1"
  local cargo_toml="$2"
  if [ ! -f "$cargo_toml" ]; then
    echo "Cargo.toml not found at $cargo_toml" >&2
    return 1
  fi
  if [[ "$OSTYPE" == "darwin"* ]]; then
    sed -i '' "s/^version = \"[^\"]*\"/version = \"$version\"/" "$cargo_toml"
  else
    sed -i "s/^version = \"[^\"]*\"/version = \"$version\"/" "$cargo_toml"
  fi
}
