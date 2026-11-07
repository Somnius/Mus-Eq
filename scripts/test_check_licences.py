# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Lef
"""Tests for scripts/check-licences.py. Run: python3 -m unittest discover -s scripts

The expected verdicts were compared with cargo-deny 0.20.2 on a throwaway workspace of 31 crates, one licence
expression each: the two agree on all of them except the two noted at the end, where this script is stricter."""
import importlib.util, os, unittest

spec = importlib.util.spec_from_file_location("check_licences", os.path.join(os.path.dirname(__file__), "check-licences.py"))
cl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cl)

ALLOW = {"MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause", "Unicode-3.0", "Zlib"}

PASSES = [
    "MIT", "MIT OR Apache-2.0", "MIT/Apache-2.0", "MIT / Apache-2.0", "Apache-2.0/MIT/Unlicense",
    "Apache-2.0 WITH LLVM-exception", "(MIT OR Apache-2.0) AND Unicode-3.0",
    "MIT AND (GPL-3.0-only OR Apache-2.0)",
    "MIT OR Apache-2.0 AND GPL-2.0-only",          # AND binds tighter: MIT OR (Apache-2.0 AND GPL), MIT passes
    "  MIT  ", "((MIT))", "Unicode-3.0 AND MIT", "BSD-3-Clause AND Zlib", "MIT OR GPL-3.0-only", "GPL-3.0-only OR MIT",
    "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
]
FAILS = [
    "MIT WITH LLVM-exception",                       # the exception form is not on the list
    "Apache-2.0 AND GPL-3.0-only", "GPL-2.0+", "GPL-3.0-only", "LicenseRef-ring", "MIT-like", "MIT OR",
    "mit or apache-2.0",                             # operators are upper case
    "MIT OR (Apache-2.0", "GPL-3.0-or-later OR LGPL-2.1-only", "MIT AND", "WITH MIT",
    "Apache 2.0",                                    # cargo-deny accepts this and the next one; we do not
    "Apache-2.0+",
]
UNPARSEABLE = ["", "   ", "MIT OR", "MIT AND", "WITH MIT", "MIT OR (Apache-2.0", "MIT )", "MIT MIT", "( )", "MIT WITH"]

class Evaluator(unittest.TestCase):
    def test_expressions_that_can_be_satisfied(self):
        for e in PASSES:
            with self.subTest(e):
                self.assertTrue(cl.allowed(e, ALLOW))

    def test_expressions_that_cannot(self):
        for e in FAILS:
            with self.subTest(e):
                try:
                    self.assertFalse(cl.allowed(e, ALLOW))
                except cl.Bad:
                    pass  # an expression that cannot be read also fails the check

    def test_unreadable_expressions_raise_and_never_pass(self):
        for e in UNPARSEABLE:
            with self.subTest(e):
                with self.assertRaises(cl.Bad):
                    cl.allowed(e, ALLOW)

    def test_with_must_match_as_one_unit(self):
        self.assertTrue(cl.allowed("Apache-2.0 WITH LLVM-exception", {"Apache-2.0 WITH LLVM-exception"}))
        # plain Apache-2.0 on the list does not make a WITH form pass on its own
        self.assertFalse(cl.allowed("Apache-2.0 WITH LLVM-exception", {"Apache-2.0"}))

    def test_and_needs_both_sides_and_or_needs_one(self):
        self.assertFalse(cl.allowed("MIT AND Zlib", {"MIT"}))
        self.assertTrue(cl.allowed("MIT OR Zlib", {"MIT"}))
        # both sides of AND are always read, so a bad right side is an error even when the left side fails
        with self.assertRaises(cl.Bad):
            cl.allowed("GPL-3.0-only AND (MIT", {"MIT"})

class Guards(unittest.TestCase):
    def test_an_empty_or_odd_cargo_tree_does_not_pass(self):
        with self.assertRaises(SystemExit):
            cl.check_tree(set(), {"museq-server@0.1.0"})          # nothing listed
        with self.assertRaises(SystemExit):
            cl.check_tree({"serde@1.0.0"}, {"museq-server@0.1.0"})  # our own crate is missing
        cl.check_tree({"serde@1.0.0", "museq-server@0.1.0"}, {"museq-server@0.1.0"})  # believable: fine

    def test_deny_toml_options_the_script_does_not_implement_are_refused(self):
        import tempfile
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('[licenses]\nallow = ["MIT"]\nexceptions = [{ allow = ["GPL-3.0-only"], crate = "x" }]\n')
        try:
            with self.assertRaises(SystemExit):
                cl.load_allow(f.name)
        finally:
            os.unlink(f.name)

class Config(unittest.TestCase):
    def test_the_repository_allow_list_is_read(self):
        allow = cl.load_allow(os.path.join(os.path.dirname(__file__), "..", "deny.toml"))
        self.assertIn("MIT", allow)
        self.assertIn("Apache-2.0", allow)
        self.assertNotIn("GPL-3.0-only", allow)



class AndroidParser(unittest.TestCase):
    def test_gradle_dependency_lines(self):
        spec = importlib.util.spec_from_file_location("android_licences", os.path.join(os.path.dirname(__file__), "android_licences.py"))
        al = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(al)
        text = "\n".join([
            "releaseRuntimeClasspath - Runtime classpath of compilation 'release'.",
            "+--- androidx.media3:media3-exoplayer:1.8.0",
            "|    +--- androidx.media3:media3-common:1.8.0 (*)",
            "|    \\--- com.google.guava:guava:31.0 -> 33.3.1-android (*)",
            "+--- androidx.compose.ui:ui -> 1.5.4",                      # version from a BOM: no own version
            "\\--- org.jetbrains.kotlin:kotlin-stdlib:2.2.20",
        ])
        got = al.parse_dependencies(text)
        self.assertIn(("com.google.guava", "guava", "33.3.1-android"), got)   # the resolved version wins
        self.assertIn(("androidx.compose.ui", "ui", "1.5.4"), got)             # a BOM-managed artifact is not dropped
        self.assertIn(("androidx.media3", "media3-common", "1.8.0"), got)
        self.assertEqual(len(got), 5)
        with self.assertRaises(SystemExit):
            al.parse_dependencies("+--- some.group:no-version-at-all")


if __name__ == "__main__":
    unittest.main()
