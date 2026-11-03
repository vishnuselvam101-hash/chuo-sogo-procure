#!/usr/bin/env python3
"""Build packs/manifest.json from packs/sources.json plus the repo-authored packs.

Upstream files keep the sha256/size pinned in sources.json. For "large" packs `url` is the
official upstream URL the app downloads on first run; for "core" packs `url` is null (the
installer bundles them) and `upstream_url` records where fetch.sh stages them from.
Repo-authored packs (`repo_dir`) are hashed from packs/<repo_dir>/.
Usage: scripts/packs/build-manifest.py   (run from anywhere)
"""
import hashlib
import json
import pathlib

REPO = pathlib.Path(__file__).resolve().parents[2]
PACKS = REPO / "packs"
SKIP_NAMES = {".gitignore", ".DS_Store"}


def sha256(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def repo_files(pack: dict) -> list[dict]:
    base = PACKS / pack["repo_dir"]
    out = []
    for p in sorted(base.rglob("*")):
        if p.is_file() and p.name not in SKIP_NAMES and p.suffix != ".py" and "__pycache__" not in p.parts:
            rel = p.relative_to(base).as_posix()
            out.append({
                "path": f"{pack['id']}/{rel}",
                "url": None,
                "repo_path": f"packs/{pack['repo_dir']}/{rel}",
                "sha256": sha256(p),
                "size": p.stat().st_size,
            })
    if not out:
        raise SystemExit(f"{pack['id']}: no files in {base}")
    return out


def upstream_file(pack: dict, f: dict) -> dict:
    g = {"path": f["path"]}
    if "platform" in f:
        g["platform"] = f["platform"]
    g["url"] = f["upstream_url"] if pack["kind"] == "large" else None
    g["upstream_url"] = f["upstream_url"]
    g["sha256"] = f["sha256"]
    g["size"] = f["size"]
    return g


def main() -> None:
    src = json.loads((PACKS / "sources.json").read_text(encoding="utf-8"))
    packs = []
    for p in src["packs"]:
        entry = {k: p[k] for k in ("id", "version", "kind", "title_en", "title_ja")}
        if "repo_dir" in p:
            entry["files"] = repo_files(p)
            entry["size"] = sum(f["size"] for f in entry["files"])
        elif "variants" in p:
            entry["variants"] = []
            for v in p["variants"]:
                files = [upstream_file(p, f) for f in v["files"]]
                entry["variants"].append({**{k: v[k] for k in ("variant", "default", "min_ram_gb", "title_en", "title_ja")},
                                          "files": files, "size": sum(f["size"] for f in files)})
            # installed size of a pack with variants = its default variant
            entry["size"] = next(v["size"] for v in entry["variants"] if v["default"])
        else:
            entry["files"] = [upstream_file(p, f) for f in p["files"]]
            entry["size"] = sum(f["size"] for f in entry["files"])
        entry.update({k: p[k] for k in ("licence", "licence_url", "source")})
        packs.append(entry)
    manifest = {
        "schema": 1,
        "variants_note": "A pack with `variants` installs exactly one of them: the `default` one unless the machine has less than its `min_ram_gb` RAM, then the next variant whose min_ram_gb fits. total_size counts default variants.",
        "root_note": "Each file path is relative to the pack root (/srv/procure-packs on the build server; the app's packs directory on a user machine).",
        "total_size": {k: sum(p["size"] for p in packs if p["kind"] == k) for k in ("core", "large")},
        "packs": packs,
    }
    (PACKS / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(manifest["total_size"]))


if __name__ == "__main__":
    main()
