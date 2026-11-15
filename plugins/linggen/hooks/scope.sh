#!/usr/bin/env bash
# Shared by autostart.sh, recall.sh and stamp-cwd.sh: where a session stands,
# for memory (linggen-memory doc/scope-index-spec.md). Hosts hand memory paths;
# memory owns the rules (which dirs can hold rows, what a root sees).
#
# memory_root <cwd> — the session's root: the git root above <cwd> (never
# $HOME, which is a dotfiles repo at most), else CLAUDE_PROJECT_DIR (where a
# Claude Code session started), else <cwd>. A skill's own dir
# (~/.linggen/skills/<name>) is its own root.
# git_root <dir> — the nearest dir at or above <dir> holding a .git (a dir,
# or a worktree's file). Never runs git: on a Mac without developer tools
# /usr/bin/git is a stub, and running it opens the install dialog on every
# hook. Same walk as the OpenClaw plugin's scope.mjs.
git_root() {
    local dir="$1"
    while [ -n "$dir" ] && [ "$dir" != / ]; do
        [ -e "$dir/.git" ] && { printf '%s\n' "$dir"; return 0; }
        dir="$(dirname "$dir")"
    done
}

memory_root() {
    local cwd="$1" root=""
    [ -n "$cwd" ] || return 0
    case "$cwd" in
        "$HOME/.linggen/skills/"*)
            local rest="${cwd#"$HOME/.linggen/skills/"}"
            printf '%s\n' "$HOME/.linggen/skills/${rest%%/*}"
            return 0
            ;;
    esac
    root="$(git_root "$cwd")"
    # Never $HOME, and never a repo ABOVE it (a cwd under $HOME whose git
    # root is outside it): memory's find_root stops at $HOME too.
    [ "$root" = "$HOME" ] && root=""
    case "$cwd" in "$HOME"|"$HOME"/*) case "$root" in "$HOME"/*) ;; *) root="" ;; esac ;; esac
    [ -n "$root" ] || root="${CLAUDE_PROJECT_DIR:-}"
    [ -n "$root" ] || root="$cwd"
    printf '%s\n' "$root"
}

# session_root <cwd> — the root this SESSION's scope candidates were shown
# against: memory_root of where the session started (CLAUDE_PROJECT_DIR), for
# any cwd at or below it. Claude Code hands every hook the SHELL's cwd, which
# moves with `cd`; rooting each call at that cwd's own git root re-based the
# candidates under the model. Started in a non-git workspace and cd'd into a
# nested repo, `linggen/linggen` resolved against the wrong parent and an
# absolute sibling dir fell outside the per-call root, so the daemon dropped
# both for cwd (2026-10-05). A cwd outside the start dir — or no start dir
# (Codex) — is rooted on its own.
session_root() {
    local cwd="$1" start="${CLAUDE_PROJECT_DIR:-}" root=""
    [ -n "$cwd" ] || return 0
    start="${start%/}"
    if [ -n "$start" ]; then
        root="$(memory_root "$start")"
        if is_project_dir "$root"; then
            case "$cwd" in "$root"|"$root"/*) printf '%s\n' "$root"; return 0 ;; esac
        fi
    fi
    memory_root "$cwd"
}

# is_project_dir <dir> — can rows be about this dir? Not $HOME, not
# ~/.linggen outside a skill's own dir, not a temp dir. Same rule as
# memory's memory::scope::is_scope_dir and the engine's is_project_dir.
is_project_dir() {
    local d="$1" tmp="${TMPDIR:-/tmp}"
    case "$d" in
        "") return 1 ;;
        "$HOME/.linggen/skills/"?*) return 0 ;;
        "$HOME"|"$HOME/"|"$HOME/.linggen"|"$HOME/.linggen/"*) return 1 ;;
        "${tmp%/}"|"${tmp%/}/"*|/tmp|/tmp/*|/private/tmp|/private/tmp/*|/private/var/folders/*|/var/folders/*) return 1 ;;
    esac
    return 0
}
