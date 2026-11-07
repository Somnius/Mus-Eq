#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
"""Adds (or with --check, verifies) the SPDX header on every tracked source file.

    python3 scripts/add-spdx.py          add the header where it is missing
    python3 scripts/add-spdx.py --check  list files without it and exit 1

Covers Rust, TypeScript/JavaScript, Kotlin and the repository's own scripts. Generated files, fixtures and
other people's files (the Gradle wrapper scripts) are skipped.
"""
import subprocess, sys, pathlib

LICENSE = "SPDX-License-Identifier: MIT OR Apache-2.0"
COPYRIGHT = "SPDX-FileCopyrightText: 2026 Lef"
SLASH = {".rs", ".ts", ".mjs", ".js", ".kt", ".kts"}
HASH = {".py", ".sh"}
SKIP_PARTS = ("fixtures/", "node_modules/", "/dist/", "gradle/wrapper/")
SKIP_NAMES = {"gradlew", "gradlew.bat"}

def files():
    out = subprocess.check_output(["git", "ls-files"], text=True).splitlines()
    for f in out:
        p = pathlib.Path(f)
        if p.name in SKIP_NAMES or any(s in f for s in SKIP_PARTS):
            continue
        if p.suffix in SLASH | HASH:
            yield p

def header(p):
    c = "//" if p.suffix in SLASH else "#"
    return f"{c} {LICENSE}\n{c} {COPYRIGHT}\n"

def has(text):
    return LICENSE in "\n".join(text.splitlines()[:6])

check = "--check" in sys.argv
missing = []
for p in files():
    text = p.read_text()
    if has(text):
        continue
    missing.append(p)
    if check:
        continue
    lines = text.splitlines(keepends=True)
    # a shebang must stay on the first line
    if lines and lines[0].startswith("#!"):
        new = lines[0] + header(p) + "".join(lines[1:])
    else:
        new = header(p) + text
    p.write_text(new)

if check:
    for p in missing:
        print("missing SPDX header:", p)
    sys.exit(1 if missing else 0)
print(f"added the header to {len(missing)} files")
