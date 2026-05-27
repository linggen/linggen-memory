#!/usr/bin/env bash
command -v ling-mem >/dev/null 2>&1 || exit 0
ling-mem start >/dev/null 2>&1 || true
