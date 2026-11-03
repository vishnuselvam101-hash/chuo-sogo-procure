#!/usr/bin/env python3
"""Validate packs/item-dictionary/items.json: JSON schema, unique ids and names, every
mpn pattern compiles and matches its own example. Exit 1 on any error.
Usage: python3 -I scripts/packs/validate-dictionary.py
"""
import json
import pathlib
import re
import sys

import jsonschema

DIR = pathlib.Path(__file__).resolve().parents[2] / "packs" / "item-dictionary"


def main() -> int:
    data = json.loads((DIR / "items.json").read_text(encoding="utf-8"))
    schema = json.loads((DIR / "schema.json").read_text(encoding="utf-8"))
    errors = [f"schema: {e.json_path}: {e.message}" for e in jsonschema.Draft202012Validator(schema).iter_errors(data)]
    seen: dict[str, set] = {"id": set(), "name_ja": set(), "name_en": set()}
    for it in data.get("items", []):
        for key, values in seen.items():
            if it[key] in values:
                errors.append(f"duplicate {key}: {it[key]}")
            values.add(it[key])
        for p in it["mpn_patterns"]:
            try:
                rx = re.compile(p["pattern"])
            except re.error as e:
                errors.append(f"{it['id']}: bad regex {p['pattern']}: {e}")
                continue
            if not rx.search(p["example"]):
                errors.append(f"{it['id']}: example {p['example']!r} does not match {p['pattern']}")
    for e in errors:
        print(e)
    items = data.get("items", [])
    n_mpn = sum(1 for i in items if i["mpn_patterns"])
    print(f"items={len(items)} with_mpn={n_mpn} uncertain={sum(i['uncertain'] for i in items)} errors={len(errors)}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
