#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
"""Fails if a Rust dependency declares a licence that deny.toml does not allow.

    python3 scripts/check-licences.py            check the tree in Cargo.lock
    python3 scripts/check-licences.py --json     the verdict for every crate as JSON (for comparing with cargo-deny)
    python3 scripts/check-licences.py --config X use another deny.toml
    python3 scripts/check-licences.py --root DIR check another cargo workspace

No tool to build: it needs python3 (3.11 or later) and cargo, reads the allow list from deny.toml, and evaluates each
crate's SPDX licence expression itself: OR, AND, WITH, parentheses, and the old "MIT/Apache-2.0" slash (meaning OR).
A crate with no licence field, an id that is not on the list, a LicenseRef-, or an expression it cannot parse fails.
A crate whose licence is only in a file, or one the owner has read and accepted, goes in
scripts/licence-exceptions.toml as name@version with the reason; the version is part of the key, so an upgrade
needs another look. Every crate cargo actually builds is checked, from `cargo tree` (all features, all targets, dev and build dependencies too,
and our own workspace crates); this is the same set cargo-deny looks at.
(cargo-deny skips dev and build dependencies unless deny.toml sets include-dev and include-build, which it does.)"""
import json, os, re, subprocess, sys, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
if "--root" in sys.argv:   # check another cargo workspace (used by the tests)
    ROOT = os.path.abspath(sys.argv[sys.argv.index("--root") + 1])

TOKEN = re.compile(r"\s*(\(|\)|/|[A-Za-z0-9][A-Za-z0-9.\-+:]*)")

class Bad(Exception):
    pass

def tokenize(expr):
    out, pos = [], 0
    expr = expr.strip()
    if not expr:
        raise Bad("empty expression")
    while pos < len(expr):
        m = TOKEN.match(expr, pos)
        if not m:
            raise Bad(f"cannot read {expr[pos:pos+12]!r}")
        out.append(m.group(1))
        pos = m.end()
    # the legacy slash is OR
    return ["OR" if t == "/" else t for t in out]

def allowed(expr, allow):
    """True if the SPDX expression can be satisfied with licences from `allow`. Raises Bad on a bad expression."""
    toks = tokenize(expr)
    i = 0
    def peek():
        return toks[i] if i < len(toks) else None
    def take():
        nonlocal i
        t = peek(); i += 1
        return t
    def atom():
        t = take()
        if t is None or t in ("AND", "OR", "WITH", ")"):
            raise Bad("a licence id was expected")
        if t == "(":
            v = expr_or()
            if take() != ")":
                raise Bad("missing )")
            return v
        if peek() == "WITH":
            take()
            exc = take()
            if exc is None or exc in ("AND", "OR", "WITH", "(", ")"):
                raise Bad("an exception id was expected after WITH")
            return f"{t} WITH {exc}" in allow
        return t in allow
    def expr_and():
        v = atom()
        while peek() == "AND":
            take()
            v = atom() and v   # both sides are always parsed
        return v
    def expr_or():
        v = expr_and()
        while peek() == "OR":
            take()
            v = expr_and() or v
        return v
    v = expr_or()
    if i != len(toks):
        raise Bad(f"unexpected {toks[i]!r}")
    return v

# [licenses] keys this script understands. Anything else (cargo-deny's exceptions, clarify, ...) would make the two tools
# disagree, so it is refused loudly instead of being ignored.
KNOWN_KEYS = {"allow", "confidence-threshold", "include-dev", "include-build", "version", "private"}

def load_allow(config):
    with open(config, "rb") as f:
        lic = tomllib.load(f)["licenses"]
    extra = set(lic) - KNOWN_KEYS
    if extra:
        raise SystemExit(f"{config}: [licenses] has {sorted(extra)}, which scripts/check-licences.py does not implement; "
                         "put the exception in scripts/licence-exceptions.toml or teach the script about it")
    return set(lic["allow"])

def load_exceptions():
    p = os.path.join(HERE, "licence-exceptions.toml")
    if not os.path.exists(p):
        return {}
    with open(p, "rb") as f:
        return {e["crate"]: e["reason"] for e in tomllib.load(f).get("exception", [])}

def built_crates():
    """name@version of every crate cargo actually builds: all features, all targets, normal, build and dev edges."""
    out = subprocess.check_output(
        ["cargo", "tree", "--workspace", "--all-features", "--target", "all", "--edges", "normal,build,dev",
         "--prefix", "none", "--format", "{p}", "--locked"], cwd=ROOT, text=True, stderr=subprocess.DEVNULL)
    crates = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            crates.add(f"{parts[0]}@{parts[1][1:]}")
    return crates

def check_tree(built, members):
    """The crate list must be believable: not empty, and it must contain every workspace crate. Otherwise `cargo tree`
    printed something this script does not understand, and "0 crates checked" must not pass."""
    if not built:
        raise SystemExit("cargo tree listed no crates; the output format may have changed. Nothing was checked.")
    missing = sorted(members - built)
    if missing:
        raise SystemExit(f"cargo tree did not list our own crates {missing}; the output format may have changed.")

def verdicts(allow, exceptions):
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"], cwd=ROOT, text=True))
    # `cargo metadata` also lists crates that no enabled feature or target pulls in; the tree is what is built
    built = built_crates()
    by_id = {p["id"]: f'{p["name"]}@{p["version"]}' for p in meta["packages"]}
    check_tree(built, {by_id[i] for i in meta["workspace_members"]})
    out = []
    for p in meta["packages"]:
        key = f'{p["name"]}@{p["version"]}'
        if key not in built:
            continue
        lic = p.get("license")
        if key in exceptions:
            out.append((key, lic, True, f"exception: {exceptions[key]}"))
        elif not lic:
            why = "licence only in a file (" + p["license_file"] + ")" if p.get("license_file") else "no licence declared"
            out.append((key, lic, False, why))
        else:
            try:
                ok = allowed(lic, allow)
                out.append((key, lic, ok, "" if ok else "no allowed licence in this expression"))
            except Bad as e:
                out.append((key, lic, False, f"cannot parse the expression: {e}"))
    missing = built - {r[0] for r in out}
    for key in sorted(missing):
        out.append((key, None, False, "built but not found in cargo metadata"))
    return sorted(out)

def main(argv):
    config = os.path.join(ROOT, "deny.toml")
    if "--config" in argv:
        config = argv[argv.index("--config") + 1]
    allow = load_allow(config)
    res = verdicts(allow, load_exceptions())
    if "--json" in argv:
        print(json.dumps([{"crate": k, "license": l, "ok": ok, "why": w} for k, l, ok, w in res], indent=1))
        return 0 if all(r[2] for r in res) else 1
    bad = [r for r in res if not r[2]]
    for key, lic, _, why in bad:
        print(f"NOT ALLOWED  {key}  license={lic!r}  ({why})")
    print(f"checked {len(res)} crates against {len(allow)} allowed licences: {len(bad)} not allowed")
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
