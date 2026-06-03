# Release targets

Where a `ling-mem` / shared-memory release has to land, in order. The binary
GitHub release is the source of truth; every channel pins to it.

## Version numbers (don't conflate)

| Number | Lives in | Tracks |
|---|---|---|
| **binary tag** `vX.Y.Z` | GitHub releases on `linggen/linggen-memory` | the compiled `ling-mem` binary |
| **binary pin** | `plugins/shared-memory/VERSION` (+ `install-bin.sh` default) | which release plugins/skills fetch (exact tag or `~X.Y` range) |
| **plugin content** `X.Y.Z` | `.claude-plugin/plugin.json` + `.codex-plugin/plugin.json` | the CC/Codex plugin **bundle** (hooks, SKILL.md). May LEAD the binary |
| **ClawHub version** | ClawHub registry (immutable per publish) | the OpenClaw skill bundle |
| **store schema** `N` | `schema_version.rs::STORE_SCHEMA_VERSION` (sidecar `SCHEMA_VERSION`) | on-disk store layout; bumps only on layout change → MAJOR binary release |

skills.sh has no version — it tracks repo `HEAD`.

## Release order

1. **Binary** — `scripts/release.sh` builds and publishes the GitHub release **manually — there is no CI** (`.github/workflows` does not exist): macOS `aarch64` is built locally and `codesign --force --sign -`'d before the tarball (Sequoia SIGKILLs unsigned), Linux `x86_64` is built natively on DS242 over ssh, then the tarball is `scp`'d back and `gh release upload`'d from the mac (which holds the `gh` auth). Each target ships `ling-mem-<target>.tar.gz` + `.sha256`.
2. **Binary pin** — if the binary changed, bump `plugins/shared-memory/VERSION` (exact tag, or a `~X.Y` range post-1.0). Skip if the binary is unchanged.
3. **Plugin bundle** — bump `.claude-plugin` + `.codex-plugin` `version` whenever the bundle (hooks/SKILL.md) changes, even if the binary didn't. `build-plugin.sh` stamps only `VERSION` from Cargo — it must NOT touch the manifest versions.
4. **Channels** — push the targets below.

## Channels

| Target | Source | Publish / update | Status |
|---|---|---|---|
| **GitHub release** (binary) | `linggen/linggen-memory` tag `v*` | `scripts/release.sh` — manual (mac local + DS242 linux, no CI) | live |
| **Claude Code — decentralized** | repo marketplace | users: `/plugin marketplace add linggen/linggen-memory` → install | live |
| **Claude Code — community marketplace** | `anthropics/claude-plugins-community` | submit at claude.ai/settings/plugins/submit (`claude plugin validate --strict` first); CI pins a SHA on approval | submitted, pending review |
| **Claude Code — official** | `claude-plugins-official` | invite-only (Anthropic) | future |
| **Codex — self-host repo marketplace** | `.agents/plugins/marketplace.json` in `linggen/linggen-memory` | users: `codex plugin marketplace add linggen/linggen-memory` → `codex plugin add shared-memory@linggen-memory` | live |
| **Codex — official directory** | OpenAI Codex Plugin Directory | curated, self-serve "coming soon" | future |
| **skills.sh** | auto-discovers `SKILL.md` in `linggen/linggen-memory` | no publish step; tracks repo `HEAD`. `npx skills add linggen/linggen-memory@shared-memory` | live |
| **ClawHub** | `clawhub skill publish <abs-path> --slug ling-mem --version X.Y.Z` (CLI-only variant) | immutable versions — a doc-only fix needs a bump; auto-runs ClawScan | live |
| **Linggen** | in-app marketplace (`/api/marketplace/install`) | engine-side install; UI: Settings → Skills | ⚠️ binary-bootstrap gap (engine session) |

## Notes

- ClawHub slug is **`ling-mem`** (owner `linggen` → clawhub.ai/linggen/ling-mem), not `shared-memory`.
- ClawHub / skills.sh / community-marketplace SKILL.md must self-reference the right slug and the per-platform install (no curl fan-out — `install-shared-memory.sh` is retired).
- Post-1.0: switch the binary pin from an exact tag to a `~1.x` range so patch/minor binaries reach users without a plugin release (`install-bin.sh` resolves ranges; the store-schema guard makes it safe). See `doc/schema-versioning.md`.
