# Linggen Memory Release Process

This document describes how to create and publish new releases of Linggen Memory.

## Overview

Linggen Memory releases to its own GitHub repository (`linggen/linggen-memory`).

The single binary `ling-mem` includes the HTTP API server, MCP endpoint, and embedded Web UI.

## Version Management

Version numbers follow semantic versioning (MAJOR.MINOR.PATCH) and must be kept in sync across:

1. `backend/Cargo.toml` — workspace version
2. `frontend/package.json` — version field

Use `scripts/sync-version.sh` to update both at once.

## Prerequisites

### GitHub CLI
Ensure you have the `gh` CLI installed and authenticated.

## Release Workflow

### Step 1: Prepare Release

1. **Update version numbers** using the sync script:

   ```bash
   ./scripts/sync-version.sh 0.7.0
   ```

2. **Test locally**:

   ```bash
   ./scripts/build.sh v0.7.0 --skip-linux
   ```

3. **Commit changes**:
   ```bash
   git add .
   git commit -m "chore: bump version to 0.7.0"
   git push origin main
   ```

### Step 2: Create Release Tag

```bash
git tag v0.7.0
git push origin v0.7.0
```

### Step 3: Publish Release

To build, sign, and upload to GitHub in one go:

```bash
./scripts/release.sh v0.7.0
```

Use `--draft` if you want to inspect the release before publishing:

```bash
./scripts/release.sh v0.7.0 --draft
```

Use `--skip-linux` to skip multi-arch Linux builds (requires Docker):

```bash
./scripts/release.sh v0.7.0 --skip-linux
```

### Step 4: Verify Release

1. Check `linggen/linggen-memory` repository for new release
2. Verify these files are present:

   - `ling-mem-macos-aarch64.tar.gz` (macOS Apple Silicon)
   - `ling-mem-linux-x86_64.tar.gz` (Linux x86_64)
   - `ling-mem-linux-aarch64.tar.gz` (Linux ARM64)
   - `manifest.json` (Update manifest)

## Script Architecture

- **`scripts/lib-common.sh`**: Shared helpers for platform detection and signing.
- **`scripts/build.sh`**: Master build orchestrator.
- **`scripts/build-mac.sh`**: macOS-specific build logic.
- **`scripts/build-linux.sh`**: Linux multi-arch build via Docker Buildx.
- **`scripts/release.sh`**: Orchestrates packaging and GitHub publishing.
- **`scripts/sync-version.sh`**: Ensures version consistency across all files.
- **`scripts/install.sh`**: End-user installer (`curl | bash`).

## Release Artifacts

Each release produces a single `ling-mem` binary per platform:

| Artifact | Platform |
|---|---|
| `ling-mem-macos-aarch64.tar.gz` | macOS Apple Silicon |
| `ling-mem-macos-x86_64.tar.gz` | macOS Intel |
| `ling-mem-linux-x86_64.tar.gz` | Linux x86_64 |
| `ling-mem-linux-aarch64.tar.gz` | Linux ARM64 |

## End-User Install

```bash
curl -fsSL https://linggen.dev/install-mem.sh | bash
```

Or install a specific version:

```bash
curl -fsSL https://linggen.dev/install-mem.sh | bash -s -- --version v0.7.0
```

## Security Notes

- **Never commit private keys** to the repository.
- Keep signing keys secure.
- Update signatures ensure users only install authentic updates.
