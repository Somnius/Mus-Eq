#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
"""Regenerate the stack badges in README.md from the versions the project really uses.

  scripts/readme-badges.py          rewrite the block between the stack markers
  scripts/readme-badges.py --check  exit 1 if README.md is out of date (used by scripts/check.sh)

Versions come from crates/museq-server/Cargo.toml, web/package.json and docs/openapi.yaml,
so changing a dependency and running this keeps the README honest.
"""
import html, json, re, sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
START, END = "<!-- stack:start -->", "<!-- stack:end -->"


def cargo_version(text: str, name: str) -> str:
    m = re.search(rf'^{re.escape(name)}\s*=\s*(?:"([^"]+)"|\{{[^}}]*version\s*=\s*"([^"]+)")', text, re.M)
    if not m:
        raise SystemExit(f"{name} not found in Cargo.toml")
    return (m.group(1) or m.group(2)).lstrip("^~=")


def short(v: str) -> str:
    return ".".join(v.split(".")[:2])


def badge(label: str, message: str, color: str, logo: str = "") -> str:
    esc = lambda s: s.replace("-", "--").replace("_", "__").replace(" ", "_")
    url = f"https://img.shields.io/badge/{esc(label)}-{esc(message)}-{color}?style=flat-square"
    if logo:
        url += f"&logo={logo}&logoColor=white"
    alt = f"{label} {message}"
    return f'<img src="{html.escape(url, quote=True)}" alt="{html.escape(alt, quote=True)}" height="20">'


def build() -> str:
    cargo = (root / "crates/museq-server/Cargo.toml").read_text()
    pkg = json.loads((root / "web/package.json").read_text())
    web = {**pkg.get("dependencies", {}), **pkg.get("devDependencies", {})}
    api = re.search(r"^openapi:\s*([\d.]+)", (root / "docs/openapi.yaml").read_text(), re.M).group(1)
    edition = re.search(r'^edition\s*=\s*"(\d+)"', cargo, re.M).group(1)
    badges = [
        badge("Rust", f"edition {edition}", "dea584", "rust"),
        badge("axum", short(cargo_version(cargo, "axum")), "2f2f2f"),
        badge("tokio", short(cargo_version(cargo, "tokio")), "2f2f2f"),
        badge("SQLite", f"rusqlite {short(cargo_version(cargo, 'rusqlite'))}", "003b57", "sqlite"),
        badge("Symphonia", short(cargo_version(cargo, "symphonia")), "2f2f2f"),
        badge("TypeScript", short(web["typescript"].lstrip("^~")), "3178c6", "typescript"),
        badge("Vite", short(web["vite"].lstrip("^~")), "646cff", "vite"),
        badge("Webamp", short(web["webamp"].lstrip("^~")), "4caf50"),
        badge("OpenAPI", api, "6ba539", "openapiinitiative"),
        badge("Docker", "Containerfile", "2496ed", "docker"),
    ]
    return START + "\n" + " ".join(badges) + "\n" + END


def main() -> int:
    readme = root / "README.md"
    text = readme.read_text()
    if START not in text or END not in text:
        raise SystemExit("README.md has no stack markers")
    new = re.sub(re.escape(START) + r".*?" + re.escape(END), lambda _: build(), text, flags=re.S)
    if "--check" in sys.argv:
        if new != text:
            print("README.md stack badges are out of date: run scripts/readme-badges.py")
            return 1
        print("ok   README stack badges are current")
        return 0
    readme.write_text(new)
    return 0


sys.exit(main())
