#!/bin/bash
set -e

# Uninstall ling-mem binary

CLI_NAME="ling-mem"
INSTALL_DIR="/usr/local/bin"
FALLBACK_DIR="$HOME/.local/bin"

removed=false

for dir in "$INSTALL_DIR" "$FALLBACK_DIR"; do
  path="${dir}/${CLI_NAME}"
  if [ -L "$path" ] || [ -f "$path" ]; then
    echo "Removing $path..."
    if [ -w "$dir" ]; then
      rm -f "$path"
    else
      sudo rm -f "$path"
    fi
    removed=true
  fi
done

if [ "$removed" = true ]; then
  echo "ling-mem uninstalled."
else
  echo "No ling-mem installation found."
fi
