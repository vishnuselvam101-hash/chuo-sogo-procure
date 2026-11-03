#!/usr/bin/env python3
"""Repack the official UB Mannheim Tesseract Windows setup (an NSIS installer) as a plain zip
of the files the app runs, so the app's installer never runs another installer.

Kept: tesseract.exe, every DLL next to it, tessdata/configs, doc/ (LICENSE, AUTHORS, README).
Dropped: training tools and their manuals, Java viewer, the bundled eng/osd traineddata (the
ocr-data pack has tessdata_best), the uninstaller and NSIS plugins.
Deterministic: sorted entries, fixed timestamps, so the result's SHA-256 can be pinned.
Usage: repack-tesseract.py <setup.exe> <out.zip>     (needs 7z)
"""
import os
import subprocess
import sys
import tempfile
import zipfile

setup, out = sys.argv[1], sys.argv[2]
top = "tesseract"
with tempfile.TemporaryDirectory() as x:
    subprocess.run(["7z", "x", "-y", f"-o{x}", setup], check=True, stdout=subprocess.DEVNULL)
    keep = []
    for root, _, files in os.walk(x):
        for f in files:
            rel = os.path.relpath(os.path.join(root, f), x).replace(os.sep, "/")
            if rel.startswith("$PLUGINSDIR/"):
                continue
            if rel == "tesseract.exe" or ("/" not in rel and rel.lower().endswith(".dll")) \
                    or rel.startswith("tessdata/configs/") or rel.startswith("doc/"):
                keep.append(rel)
    if "tesseract.exe" not in keep or "doc/LICENSE" not in keep:
        sys.exit("unexpected installer layout")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for rel in sorted(keep):
            info = zipfile.ZipInfo(f"{top}/{rel}", date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            with open(os.path.join(x, rel), "rb") as fh:
                z.writestr(info, fh.read(), compresslevel=9)
print(f"{out}: {len(keep)} files")
