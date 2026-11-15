// Where a session stands, for memory — the OpenClaw port of
// `plugins/linggen/hooks/scope.sh` (linggen-memory doc/scope-index-spec.md).
//
// Hosts hand memory paths; memory owns the rules (which dirs can hold
// rows, what a root sees). Node builtins only.

import { existsSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";

/**
 * Can rows be about this dir? Not `$HOME` (no particular work), not
 * `~/.linggen` outside a skill's own dir (the engine's state), not a temp dir.
 * Same rule as memory's `memory::scope::is_scope_dir`.
 */
export function isProjectDir(dir) {
  if (!dir) return false;
  const home = homedir();
  const skills = join(home, ".linggen", "skills");
  const under = (base) => dir === base || dir.startsWith(`${base}/`);
  if (dir.startsWith(`${skills}/`)) return true;
  if (dir === home || under(join(home, ".linggen"))) return false;
  const tmp = tmpdir().replace(/\/$/, "");
  if (under(tmp) || under("/tmp") || under("/private/tmp") || under("/var/folders") || under("/private/var/folders")) {
    return false;
  }
  return true;
}

/**
 * The session's root: a skill's own dir for anything inside it, else the git
 * root above `cwd` (never `$HOME`), else `cwd` itself. "" for no cwd.
 */
export function memoryRoot(cwd) {
  if (!cwd) return "";
  const home = homedir();
  const skills = join(home, ".linggen", "skills");
  if (cwd.startsWith(`${skills}/`)) {
    return join(skills, cwd.slice(skills.length + 1).split("/")[0]);
  }
  for (let dir = cwd; dir && dir !== home; dir = dirname(dir)) {
    if (existsSync(join(dir, ".git"))) return dir;
    if (dirname(dir) === dir) break;
  }
  return cwd;
}
