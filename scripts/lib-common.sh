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
