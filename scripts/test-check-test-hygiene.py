#!/usr/bin/env python3
"""Tests for scripts/check-test-hygiene.py."""

import importlib.util
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-test-hygiene.py")
_spec = importlib.util.spec_from_file_location("check_test_hygiene", SCRIPT)
guard = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(guard)


def rust(src):
    return [(rule, name) for rule, name, _line, _msg in guard.check_rust(src)]


class RustAssertions(unittest.TestCase):
    def test_test_without_assertion_is_reported_with_its_line(self):
        src = "fn helper() {}\n\n#[test]\nfn calls_only() {\n    helper();\n}\n"
        self.assertEqual(
            guard.check_rust(src),
            [("no-assert", "calls_only", 3, "test asserts nothing")],
        )

    def test_assert_macros_and_error_unwraps_count(self):
        for body in (
            "assert!(x);",
            "assert_eq!(a, b);",
            "prop_assert!(x);",
            "debug_assert_ne!(a, b);",
            "insta::assert_snapshot!(x);",
            "let e = f().unwrap_err();",
            'f().expect_err("must fail");',
            "assert_shape(&x);",
            'let Some(v) = f() else { panic!("none") };',
        ):
            with self.subTest(body=body):
                self.assertEqual(rust(f"#[test]\nfn t() {{\n    {body}\n}}\n"), [])

    def test_assert_in_a_comment_or_string_does_not_count(self):
        src = (
            "#[test]\nfn t() {\n"
            "    // assert!(never_run);\n"
            "    /* assert_eq!(1, 1); /* nested */ */\n"
            '    log("assert!(x)");\n'
            '    log(r#"assert!("quoted")"#);\n'
            "}\n"
        )
        self.assertEqual(rust(src), [("no-assert", "t")])

    def test_braces_in_strings_and_chars_do_not_end_the_body(self):
        src = (
            "#[test]\nfn t() {\n"
            "    let a = \"}\";\n    let b = '}';\n    let c = '\\'';\n"
            '    let d: &\'static str = r"}";\n'
            "    assert!(a.len() == 1);\n"
            "}\n"
        )
        self.assertEqual(rust(src), [])

    def test_assertion_in_the_next_function_does_not_count(self):
        src = "#[test]\nfn first() { run(); }\n\n#[test]\nfn second() { assert!(true); }\n"
        self.assertEqual(rust(src), [("no-assert", "first")])

    def test_async_and_parameterised_test_attributes_are_tests(self):
        src = (
            '#[tokio::test(flavor = "multi_thread")]\n'
            "async fn spawns() {\n    run().await;\n}\n"
        )
        self.assertEqual(rust(src), [("no-assert", "spawns")])

    def test_cfg_test_module_is_not_a_test(self):
        self.assertEqual(rust("#[cfg(test)]\nmod tests {\n    fn helper() {}\n}\n"), [])

    def test_should_panic_counts_above_or_below_the_test_attribute(self):
        below = '#[test]\n#[should_panic(expected = "boom")]\nfn t() { run(); }\n'
        above = "#[should_panic]\n#[test]\nfn t() { run(); }\n"
        self.assertEqual(rust(below), [])
        self.assertEqual(rust(above), [])

    def test_does_not_panic_needs_the_suffix_and_a_comment(self):
        documented = (
            "// Guards the slice panic fixed in 2.20.\n"
            "#[test]\nfn long_input_does_not_panic() { run(); }\n"
        )
        undocumented = "#[test]\nfn long_input_does_not_panic() { run(); }\n"
        wrong_name = "// Guards a panic.\n#[test]\nfn long_input_is_fine() { run(); }\n"
        self.assertEqual(rust(documented), [])
        self.assertEqual(
            rust(undocumented), [("no-assert", "long_input_does_not_panic")]
        )
        self.assertEqual(rust(wrong_name), [("no-assert", "long_input_is_fine")])


class RustIgnore(unittest.TestCase):
    def test_bare_ignore_is_reported(self):
        src = "#[test]\n#[ignore]\nfn slow() { assert!(true); }\n"
        self.assertEqual(rust(src), [("bare-ignore", "slow")])

    def test_ignore_with_a_reason_passes(self):
        src = '#[test]\n#[ignore = "needs docker; run by the e2e job"]\nfn slow() { assert!(true); }\n'
        self.assertEqual(rust(src), [])

    def test_ignore_in_a_comment_is_not_reported(self):
        src = "// #[ignore]\n#[test]\nfn fast() { assert!(true); }\n"
        self.assertEqual(rust(src), [])


class JsAndPython(unittest.TestCase):
    def test_focused_tests_are_reported(self):
        src = (
            "test.only('a', () => {});\n"
            "describe.only('b', () => {});\n"
            "it.only('c', () => {});\n"
            "fdescribe('d', () => {});\n"
            "fit('e', () => {});\n"
        )
        found = [(rule, name, line) for rule, name, line, _ in guard.check_js(src)]
        self.assertEqual(
            found,
            [
                ("focused-test", "test.only", 1),
                ("focused-test", "describe.only", 2),
                ("focused-test", "it.only", 3),
                ("focused-test", "fdescribe", 4),
                ("focused-test", "fit", 5),
            ],
        )

    def test_ordinary_tests_and_lookalikes_pass(self):
        src = "test('a', () => {});\nbenefit(1);\nconst only = test.skip;\n"
        self.assertEqual(guard.check_js(src), [])

    def test_python_skip_needs_a_reason(self):
        bare = "@pytest.mark.skip\ndef test_a():\n    pass\n"
        empty = "@pytest.mark.skip()\ndef test_b():\n    pass\n"
        reasoned = (
            '@pytest.mark.skip(reason="needs the lab image")\ndef test_c():\n    pass\n'
        )
        self.assertEqual(
            [(r, n) for r, n, _, _ in guard.check_python(bare)],
            [("bare-skip", "test_a")],
        )
        self.assertEqual(
            [(r, n) for r, n, _, _ in guard.check_python(empty)],
            [("bare-skip", "test_b")],
        )
        self.assertEqual(guard.check_python(reasoned), [])


class Tree(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.allow = self.root / "allow.txt"
        self.addCleanup(self._tmp.cleanup)

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def run_guard(self):
        out = io.StringIO()
        code = guard.run(self.root, self.allow, out)
        return code, out.getvalue()

    def test_clean_tree_passes(self):
        self.write("crate/src/lib.rs", "#[test]\nfn t() { assert!(true); }\n")
        self.assertEqual(self.run_guard(), (0, ""))

    def test_offence_fails_and_names_file_line_rule_and_test(self):
        self.write("crate/tests/it.rs", "\n#[test]\nfn empty() { run(); }\n")
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("crate/tests/it.rs:2: no-assert: empty:", out)

    def test_allowlisted_offence_passes_and_others_still_fail(self):
        self.write(
            "crate/src/lib.rs",
            "#[test]\nfn known() { run(); }\n#[test]\nfn fresh() { run(); }\n",
        )
        self.allow.write_text("crate/src/lib.rs::known  # deferred: crate in flight\n")
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("fresh", out)
        self.assertNotIn("known", out)

    def test_stale_allowlist_entry_is_noted_without_failing(self):
        self.write("crate/src/lib.rs", "#[test]\nfn t() { assert!(true); }\n")
        self.allow.write_text("# header\n\ncrate/src/lib.rs::gone  # fixed since\n")
        self.assertEqual(
            self.run_guard(),
            (0, "note: allowlist entry no longer needed: crate/src/lib.rs::gone\n"),
        )

    def test_build_and_dependency_directories_are_skipped(self):
        self.write("target/debug/x.rs", "#[test]\nfn t() { run(); }\n")
        self.write("web/node_modules/pkg/a.test.js", "test.only('a', () => {});\n")
        self.assertEqual(self.run_guard(), (0, ""))

    def test_focused_js_and_reasonless_python_skip_fail(self):
        self.write("web/tests/a.spec.ts", "test.only('a', async () => {});\n")
        self.write(
            "lab/tests/test_a.py", "@pytest.mark.skip\ndef test_a():\n    pass\n"
        )
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("web/tests/a.spec.ts:1: focused-test: test.only:", out)
        self.assertIn("lab/tests/test_a.py:1: bare-skip: test_a:", out)

    def test_command_line_exit_codes(self):
        self.write("crate/src/lib.rs", "#[test]\nfn empty() { run(); }\n")
        args = [sys.executable, str(SCRIPT), "--root", str(self.root)]
        failing = subprocess.run(
            [*args, "--allowlist", str(self.allow)],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(failing.returncode, 1)
        self.assertIn("1 test hygiene failure(s)", failing.stdout)
        listing = subprocess.run(
            [*args, "--list"], capture_output=True, text=True, check=False
        )
        self.assertEqual(listing.returncode, 0)
        self.assertEqual(listing.stdout, "crate/src/lib.rs::empty  # no-assert\n")


class Repository(unittest.TestCase):
    def test_repository_passes_with_its_allowlist(self):
        out = io.StringIO()
        code = guard.run(guard.ROOT, guard.ALLOWLIST, out)
        self.assertEqual(code, 0, out.getvalue())


if __name__ == "__main__":
    unittest.main()
