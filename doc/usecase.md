# Linggen Memory: Project Memory & Long-Term Context for AI

This document describes the **core use cases, concepts, and workflows** behind Linggen.

Linggen is **not another AI agent**.
It is a **long-term project memory layer** that sits _before_ Cursor / LLMs, ensuring they do not lose historical context, decisions, and constraints.

---

## One-Sentence Positioning

**Linggen helps AI remember _why_ your project is the way it is —  
so it doesn’t undo past decisions when you come back later.**

---

## Core Problem Linggen Solves

Modern LLM-based tools (Cursor, Copilot, ChatGPT):

- Are very good at reasoning **within the current session**
- Are very bad at remembering **across time, projects, and decisions**

As a result:

- Old projects feel like new ones
- Past tradeoffs are forgotten
- AI repeatedly suggests changes that were already rejected

Linggen fills this gap by providing **persistent, project-scoped and cross-project memory**.

---

## Killer Primary Use Case (Main Scenario)

### Returning to an Old Project (or New Project Onboarding)

**Situation**

- You reopen a repository after weeks or months
- Cursor is smart, but:
  - It doesn’t know _why_ the architecture is like this
  - It doesn’t know which approaches were already tried and rejected
  - It doesn’t know which changes are dangerous

**Without Linggen**

- You (and the AI) re-learn everything from scratch
- AI proposes “reasonable” but historically wrong refactors
- You waste time rediscovering old constraints

**With Linggen**

- Project memory is automatically injected via MCP
- Cursor sees:
  - Hard rules (Do / Don’t)
  - Past decisions with reasons
  - Known gotchas and pitfalls
- AI behaves like a teammate who has worked on this project before

> This is Linggen’s **single most important scenario**.

---

## Supporting Use Cases (Compressed)

### 1. AI Safety Brake for Refactors and Big Changes

**Problem**

- LLMs often suggest:
  - Large refactors
  - Replacing core dependencies
  - Breaking public APIs

**Linggen’s Role**

- Surface historical decisions and constraints _before execution_
- Prevent AI from repeating rejected approaches
- Act as a “memory-based guardrail”

---

### 2. Multi-Project Switching (Cross-Project Memory)

**Problem**

- Developers maintain multiple repositories
- Each project has different:
  - Tech stacks
  - Rules
  - Risk tolerance
- AI treats every project the same

**Linggen’s Role**

- Maintain **separate project memories per workspace**
- Automatically switch context when you switch projects
- (Optionally) reuse personal patterns across projects

> Cross-project memory is **structurally missing** from Cursor and LLMs.

---

### 3. Capturing “Why” Without Writing Documentation

**Problem**

- Developers know _why_ something is written a certain way
- But don’t want to write long docs

**Linggen’s Role**

- Allow one-line notes to be pinned directly from code
- Preserve intent, tradeoffs, and rationale
- Make “future you” and AI aware of past reasoning

---

### 4. Making AI Ask the Right Clarifying Questions

**Problem**

- AI often guesses intent:
  - quick fix vs long-term
  - allow breaking changes or not

**Linggen’s Role**

- Provide suggested clarification questions
- Encourage AI to pause before acting
- Improve plan quality without extra prompting

## VSCode extension

select code, right click, linggen: pin to memory
or linggen: add good case
linggen: add bad case
these could be saved in file and RAG like :
.linggen/cases.jsonl

```json
{
  "id": "uuid",
  "ts": "2025-12-12T12:00:00Z",
  "case_type": "good|bad",
  "title": "Prefer explicit Result handling",
  "note": "Avoid unwrap in auth path; map errors to HTTP responses",
  "tags": ["#auth", "#reliability"],
  "source": {
    "path": "src/auth/jwt.rs",
    "range": { "startLine": 12, "endLine": 40 },
    "snippet": "..."
  }
}
```

---

## Project Memory

Memories are stored in the Linggen Memory server (LanceDB) and retrieved via semantic search. They can also be managed through the AI skill scripts (`store_memory.sh`, `search_memory.sh`, `fetch_memory.sh`).
