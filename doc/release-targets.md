# Release targets

Where a `ling-mem` release lands and what each channel pins to. The binary
GitHub release is the source of truth; every channel resolves against it.
How to cut the release — the order, the scripts, the local swap — is the
cross-product checklist: `linggen/doc/release-checklist.md`.

## Version numbers (don't conflate)

| Number | Lives in | Tracks |
|---|---|---|
| **binary tag** `vX.Y.Z` | GitHub releases on `linggen/linggen-memory` | the compiled `ling-mem` binary — three targets, each `ling-mem-<target>.tar.gz` + `.sha256` (macos-aarch64, linux-x86_64, linux-aarch64; no macos-x86_64 since 1.7.0) |
| **binary pin** `^1` | `plugins/linggen/hooks/autostart.sh` (`PIN="${LING_MEM_VERSION:-^1}"`); the engine's `LING_MEM_PIN` matches | which release hosts fetch — a semver range resolved by `install-bin.sh` against the release list, so a patch/minor binary reaches users with no plugin release |
| **plugin content** `X.Y.Z` | `plugins/linggen/.claude-plugin/plugin.json`, `plugins/linggen/.codex-plugin/plugin.json`, `plugins/openclaw/openclaw.plugin.json` | the plugin **bundle** (hooks, SKILL.md). Moves only when the bundle changes; may lead or lag the binary |
| **ClawHub version** | ClawHub registry (immutable per publish) | the OpenClaw skill bundle; a doc-only fix still needs a bump |
| **store schema** `N` | `schema_version.rs::STORE_SCHEMA_VERSION` (sidecar `SCHEMA_VERSION`) | on-disk store layout; bumps only on a layout change → MAJOR binary release, outside the `^1` range (`doc/schema-versioning.md`) |

skills.sh has no version — it tracks repo `HEAD`.

## Channels

| Target | Source | Publish / update | Status |
|---|---|---|---|
| **GitHub release** (binary) | `linggen/linggen-memory` tag `v*` | mac: `scripts/release.sh vX.Y.Z --draft` locally (ad-hoc codesigned); linux: `build-linux.yml` dispatch onto the draft; then publish | live |
| **Claude Code — decentralized** | repo marketplace (`.claude-plugin/marketplace.json`) | users: `/plugin marketplace add linggen/linggen-memory` → install | live |
| **Claude Code — community marketplace** | `anthropics/claude-plugins-community` | submit at claude.ai/settings/plugins/submit (`claude plugin validate --strict` first); CI pins a SHA on approval | submitted; acceptance not yet observed |
| **Claude Code — official** | `claude-plugins-official` | invite-only (Anthropic) | future |
| **Codex — self-host repo marketplace** | `.agents/plugins/marketplace.json` | users: `codex plugin marketplace add linggen/linggen-memory` → `codex plugin add linggen@linggen-memory` | live |
| **Codex — official directory** | OpenAI Codex Plugin Directory | curated, self-serve "coming soon" | future |
| **skills.sh** | auto-discovers `SKILL.md` in `linggen/linggen-memory` | no publish step; tracks repo `HEAD`. `npx skills add linggen/linggen-memory@linggen` | live |
| **ClawHub** | `clawhub skill publish <abs-path> --slug linggen --version X.Y.Z` | immutable versions; auto-runs ClawScan | live |
| **Linggen** | engine auto-install on first memory use (`memory_http.rs`: `install-bin.sh`, `^1`, linggen.dev/dl mirror fallback) | nothing to publish — the engine installs the binary itself | live |

## Notes

- ClawHub slug is **`linggen`** (owner `linggen` → clawhub.ai/linggen/linggen); the old `ling-mem` slug redirects (renamed 2026-07-10, skill v2.0.0).
- ClawHub / skills.sh / community-marketplace SKILL.md must self-reference the right slug and the per-platform install — no curl fan-out (`install-shared-memory.sh` is retired).
- MCP server `instructions` in `src/http/mcp.rs` are the canonical memory protocol; every host (including the engine) injects them from there. A doctrine change is a binary release.
