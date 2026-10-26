#!/usr/bin/env bash
# Sets scout up from nothing: checks Rust, builds, then `scout setup` (the `scout` command, MCP for
# Claude Code and Codex, Claude Code hooks, agent instructions, JavaScript parsers). `--dry-run` only shows it.
set -euo pipefail
home="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
command -v cargo >/dev/null || { echo "Rust is required: https://rustup.rs (then run this again)" >&2; exit 1; }
"$home/scout" --version
"$home/scout" setup "$@"
