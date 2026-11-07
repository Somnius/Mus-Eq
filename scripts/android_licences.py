#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
"""Lists the Android app's release dependencies with the licence each one declares in its POM.

Used by gen-third-party.py. It runs `./gradlew :app:dependencies` for the release runtime classpath and reads the POM
files Gradle has already downloaded into ~/.gradle (so run an Android build first). A library that declares no
licence in its own POM is looked up through its parent POMs; if none names one, it is reported as not declared.
The licence texts themselves are not read."""
import os, re, subprocess, glob
import xml.etree.ElementTree as ET

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "android")
CACHE = os.path.expanduser("~/.gradle/caches/modules-2/files-2.1")
NS = {"m": "http://maven.apache.org/POM/4.0.0"}

LINE = re.compile(r"[+\\]---\s+([\w.\-]+):([\w.\-]+)(?::([\w.\-+]+))?(?:\s*->\s*([\w.\-+]+))?")

def parse_dependencies(text):
    """(group, artifact, version) for every dependency line of `gradle dependencies`. A version that comes from a
    platform/BOM is printed as `group:artifact -> resolved` and has no version of its own; the resolved one is used.
    A project dependency or a line without any version is reported by name instead of being dropped."""
    found, unversioned = {}, []
    for line in text.splitlines():
        m = LINE.search(line)
        if not m:
            continue
        g, a, v, resolved = m.groups()
        version = resolved or v
        if not version:
            unversioned.append(f"{g}:{a}")
            continue
        found[(g, a)] = version
    if unversioned:
        raise SystemExit("dependencies without a resolvable version: " + ", ".join(sorted(set(unversioned))))
    return sorted((g, a, v) for (g, a), v in found.items())

def dependencies():
    out = subprocess.check_output(
        ["./gradlew", "--no-daemon", "-q", ":app:dependencies", "--configuration", "releaseRuntimeClasspath"],
        cwd=ROOT, text=True, stderr=subprocess.DEVNULL)
    deps = parse_dependencies(out)
    if not deps:
        raise SystemExit("gradle listed no dependencies; the output format may have changed. Nothing was listed.")
    return deps

def find_pom(g, a, v):
    hits = glob.glob(os.path.join(CACHE, g, a, v, "*", f"{a}-{v}.pom"))
    return hits[0] if hits else None

def licences(g, a, v, depth=0):
    pom = find_pom(g, a, v)
    if not pom or depth > 5:
        return None
    root = ET.parse(pom).getroot()
    names = [n.text.strip() for n in root.findall("m:licenses/m:license/m:name", NS) if n.text]
    if names:
        return names
    p = root.find("m:parent", NS)
    if p is not None:
        pg, pa, pv = (p.findtext(f"m:{k}", namespaces=NS) for k in ("groupId", "artifactId", "version"))
        return licences(pg, pa, pv, depth + 1)
    return None

def section():
    rows, missing = [], []
    for g, a, v in dependencies():
        lic = licences(g, a, v)
        rows.append((f"{g}:{a}", v, "; ".join(lic) if lic else "NOT DECLARED IN ITS POM"))
        if not lic:
            missing.append(f"{g}:{a}")
    out = ["## Android app (release runtime classpath)\n",
           f"{len(rows)} artifacts, each with the licence its POM declares (parent POMs followed; texts not read).\n"]
    if missing:
        out.append(f"{len(missing)} declare none in a POM and need a manual look: " + ", ".join(missing) + "\n")
    out += ["| Artifact | Version | Declared licence |", "|---|---|---|"]
    out += [f"| {n} | {v} | {l} |" for n, v, l in rows]
    return out

if __name__ == "__main__":
    print("\n".join(section()))
