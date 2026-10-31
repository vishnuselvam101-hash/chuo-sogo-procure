#!/usr/bin/env bash
# PostToolUse: format the single edited Rust file. Cheap; full gate runs at milestones.
path="$(python3 -I -c 'import sys,json; print(json.load(sys.stdin).get("tool_input",{}).get("file_path",""))')"
case "$path" in *.rs) [ -f "$path" ] && rustfmt --edition 2024 --quiet "$path" 2>/dev/null ;; esac
exit 0
