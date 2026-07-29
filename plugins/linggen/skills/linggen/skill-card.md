## Description: <br>
Linggen provides durable cross-host memory and browser control through one local MCP server, using a three-tier memory model shared across Claude Code, Codex, and OpenClaw. <br>

Licensed under Apache 2.0; commercial and non-commercial use are both permitted under those terms. <br>

## Publisher: <br>
[linggen](https://clawhub.ai/user/linggen) <br>

### License/Terms of Use: <br>
Apache 2.0 <br>


## Use Case: <br>
Developers and agent users use Linggen to give assistants persistent local memory across sessions and hosts, with commands for adding, searching, consolidating, forgetting, and auditing memories. Users can also use its local MCP server for controlled browser and X session reads when the Linggen engine and browser extension are available. <br>

### Deployment Geography for Use: <br>
Global <br>

## Known Risks and Mitigations: <br>
Risk: On first use the skill installs two binaries by piping remote shell scripts to bash — `install-bin.sh` from the linggen-memory GitHub repo and `install.sh` from linggen.dev — and SKILL.md instructs the agent to run them without asking first. Each is a no-op when the binary is already present. <br>
Mitigation: Read both installer scripts at their published URLs before installing this skill, or install `ling-mem` and `ling` yourself beforehand so the checks short-circuit. `ling-mem` lands in `~/.local/bin`, pinned to the 1.x line and SHA-256 verified. Later upgrades (`ling-mem upgrade --yes`) do ask before swapping the binary. <br>
Risk: The local MCP server exposes `agent_run`, which starts a Linggen engine agent. This is broader than the memory-and-browser purpose the skill's name and summary suggest. <br>
Mitigation: Treat `agent_run` as general agent-execution authority, not a memory tool. Do not install if your environment should not grant that; where your host supports per-tool allowlisting, exclude `agent_run` and keep only the `memory_*` tools. <br>
Risk: The skill stores durable memory under `~/.linggen/memory`, saves personal facts (relationships, location, timezone, identity, goals) from ordinary conversation on the agent's own judgment rather than an explicit per-capture opt-in, and injects recalled facts into the prompts sent to your configured LLM provider. Records include `[SESSION_CWD]` project-path metadata. <br>
Mitigation: Audit anything stored with the list and search commands, and remove rows by id with delete. Assume every stored row may reach your model provider on a later turn, so avoid the skill for secrets or regulated data. Core-tier writes are always-injected — review that tier first. <br>
Risk: The scan and dream maintenance flows read local assistant session transcripts from Claude Code, Codex, OpenClaw, and Linggen into the memory store. This is broad private-data access without a first-run source preview. <br>
Mitigation: Scan is user-triggered, never scheduled — do not run it on machines whose transcripts contain material you do not want retained. Back up `~/.linggen/memory` before a dream, solve, or condense pass. <br>
Risk: The agent may delete or rewrite memory rows on its own judgment during consolidation and forget flows. Scope is the memory store only; there is no filesystem deletion. <br>
Mitigation: Back up the store before maintenance passes, and use the audit and review-queue flows so removals surface to you rather than happening silently. <br>
Risk: Browser and X-session access reads logged-in user context and can act in the browser through the local MCP server. <br>
Mitigation: Use browser access only with the visible controlled tab and per-site permission prompts enabled; stop when permissions are declined, and require confirmation for credentials, payments, deletes, and posting. <br>
Risk: A shared memory store can be affected by version or schema skew between installers and hosts. <br>
Mitigation: Check Linggen and ling-mem status before maintenance operations, keep one source of truth for binary updates, and back up the memory store before maintenance passes. <br>


## Reference(s): <br>
- [Linggen homepage](https://linggen.dev) <br>
- [README](README.md) <br>
- [Routing rules](references/routing-rules.md) <br>
- [Dream flow](references/dream-flow.md) <br>
- [Extractor prompt](references/extractor-prompt.md) <br>
- [Condense flow](references/condense-flow.md) <br>
- [Shared memory design](doc/shared-memory-design.md) <br>


## Skill Output: <br>
**Output Type(s):** [Text, Markdown, Shell commands, Configuration, Guidance] <br>
**Output Format:** [Markdown responses with shell commands and JSON CLI output when memory records are listed or scanned] <br>
**Output Parameters:** [1D] <br>
**Other Properties Related to Output:** [May emit status lines for scan, dream, solve, and maintenance flows; CLI JSON output should omit embedding vectors before display.] <br>

## Skill Version(s): <br>
2.2.1 <br>

## Ethical Considerations: <br>
This skill retains personal information about its user by design. Users should evaluate whether persistent, provider-visible memory is appropriate for their environment, review stored rows before relying on them, and apply their organization's safety, security, and compliance requirements before deployment. <br>
