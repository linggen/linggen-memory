---
name: linggen
description: Use Linggen for cross-project code search, indexed context, and prompt enhancement. For single-repo tasks, use local repo tools first; Linggen server startup is optional. A skill registry for lookup and install.
argument-hint: "[query]"
---

# Linggen Skill

Use this skill when Linggen-specific capabilities add value. Keep the user experience simple and do not require Linggen server startup for normal single-repo work.

## When To Use This Skill

- Use Linggen when the user needs cross-project search, indexed memory/context, or prompt enhancement.
- For single-repo tasks, prefer normal local repo tools.
- If `.linggen/` exists, treat it as project-specific guidance and review relevant memory files.

## Script Location

Resolve scripts path before running Linggen commands:

```bash
LINGGEN_SCRIPTS_DIR="$PWD/.claude/skills/linggen/scripts"
[ -d "$LINGGEN_SCRIPTS_DIR" ] || LINGGEN_SCRIPTS_DIR="$PWD/.codex/skills/linggen/scripts"
[ -d "$LINGGEN_SCRIPTS_DIR" ] || LINGGEN_SCRIPTS_DIR="${CODEX_HOME:-$HOME/.codex}/skills/linggen/scripts"
[ -d "$LINGGEN_SCRIPTS_DIR" ] || LINGGEN_SCRIPTS_DIR="$HOME/.claude/skills/linggen/scripts"
```

## Core Workflows

### 1. Local Memory And Context

- If present, inspect local guidance with `ls -R .linggen/`.
- Prioritize `.linggen/memory/` for architectural decisions.
- If code comments contain `// linggen memory: <file>.md`, read `.linggen/memory/<file>.md` immediately.
- If code comments contain `// linggen anchor: <repo/relative/path>`, open that referenced repository file.

### 2. Prompt Enhancement (Optional)

Use when the task is ambiguous, cross-project, or likely to benefit from indexed context:

- `bash "$LINGGEN_SCRIPTS_DIR/enhance_prompt.sh" "<query>" [strategy] [source_id]`

### 3. Code Discovery And Search

Use Linggen search primarily for cross-project discovery:

- Search chunks: `bash "$LINGGEN_SCRIPTS_DIR/search_codebase.sh" "<query>" [strategy] [limit] [source_id]`
- Deep metadata search: `bash "$LINGGEN_SCRIPTS_DIR/query_codebase.sh" "<query>" [limit] [exclude_source_id]`
- List indexed sources: `bash "$LINGGEN_SCRIPTS_DIR/list_sources.sh"`

### 4. Linggen Library (Remote Packs)

- Browse library packs: `bash "$LINGGEN_SCRIPTS_DIR/list_library_packs.sh"`
- Read a pack: `bash "$LINGGEN_SCRIPTS_DIR/get_library_pack.sh" "<pack_id>"`
- Lookup skills (local + online): `bash "$LINGGEN_SCRIPTS_DIR/lookup_skills.sh" "<keyword>"`
- Install online skill only with user approval: `bash "$LINGGEN_SCRIPTS_DIR/install_online_skill.sh" "<keyword>"`

### 5. Linggen Server Bootstrap

Start server only when Linggen-backed features are needed:

- Start Linggen server: `bash "$LINGGEN_SCRIPTS_DIR/start_linggen_server.sh"`
- Health check: `bash "$LINGGEN_SCRIPTS_DIR/get_status.sh"`

## Server Dependency Matrix

Requires Linggen server (`API_URL` / local Linggen API):

- `enhance_prompt.sh`
- `search_codebase.sh`
- `query_codebase.sh`
- `memory_search_semantic.sh`
- `memory_fetch_by_meta.sh`
- `list_sources.sh`
- `list_library_packs.sh`
- `get_library_pack.sh`
- `get_status.sh`

Does not require Linggen server:

- `start_linggen_server.sh` (starts server via `linggen`; may need internet only when CLI install is needed)
- `install_online_skill.sh` (uses online registry/GitHub APIs)
- `config.sh` (configuration only)

Mixed behavior:

- `lookup_skills.sh`
- Uses Linggen server for local library-pack search.
- Falls back to online registries (`linggen-analytics` / `skills.sh`) even if server is unavailable.

## Operational Notes

- Linggen scripts use network calls; ensure network permission when required by the environment.
- Omit `source_id` to search across indexed projects.
- Keep single-repo work local by default; use Linggen when cross-project or indexed context is needed.
