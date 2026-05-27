#!/usr/bin/env bash
# Ensure ling-mem daemon is up before the host tries to connect via MCP.
# `ling-mem start` is idempotent — exits 0 if the daemon is already running.
command -v ling-mem >/dev/null 2>&1 || exit 0
ling-mem start >/dev/null 2>&1 || true
