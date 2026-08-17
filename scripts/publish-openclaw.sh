#!/bin/bash
# Publish the OpenClaw plugin to ClawHub.
# Run from anywhere; provenance resolves from the pushed commit, so push first.
set -euo pipefail
cd "$(dirname "$0")/../plugins/openclaw"
exec clawhub package publish . --family code-plugin
