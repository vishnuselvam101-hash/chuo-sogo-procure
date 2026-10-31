#!/usr/bin/env bash
# PreToolUse: block reads/edits of secret material. Exit 2 = block, message goes to Claude.
input="$(cat)"
path="$(printf '%s' "$input" | python3 -I -c 'import sys,json; d=json.load(sys.stdin).get("tool_input",{}); print(d.get("file_path") or d.get("path") or "")')"
[ -z "$path" ] && exit 0
case "$path" in
  */.env|*/.env.*) case "$path" in */.env.example) exit 0;; esac ;&
  *.pem|*.key|*.p12|*.pfx|*/secrets/*|*/signing/*|*tauri*.key*)
    echo "Blocked by project hook: $path is secret material. Ask the founder." >&2; exit 2;;
esac
exit 0
