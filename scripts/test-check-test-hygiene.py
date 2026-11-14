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


def rust(src, **kwargs):
    return [(rule, name) for rule, name, _line, _msg in guard.check_rust(src, **kwargs)]


def test_fn(body, signature="fn t()", attrs="#[test]"):
    return f"{attrs}\n{signature} {{\n    {body}\n}}\n"


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
            'let Some(v) = f() else { panic!("none") };',
            "match f() { Ok(v) => drop(v), Err(_) => unreachable!() }",
            "Cli::command().debug_assert();",
        ):
            with self.subTest(body=body):
                self.assertEqual(rust(test_fn(body)), [])

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

    def test_an_assertion_on_a_literal_true_does_not_count(self):
        for body in (
            "run(); assert!(true);",
            'run(); assert!(true, "reached the end");',
            "run(); assert!( true );",
            "run(); assert_eq!(true, true);",
        ):
            with self.subTest(body=body):
                self.assertEqual(rust(test_fn(body)), [("no-assert", "t")])
        self.assertEqual(rust(test_fn("assert!(true_enough(x));")), [])

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
        src = "#[test]\nfn first() { run(); }\n\n#[test]\nfn second() { assert!(ok()); }\n"
        self.assertEqual(rust(src), [("no-assert", "first")])

    def test_async_and_parameterised_test_attributes_are_tests(self):
        src = (
            '#[tokio::test(flavor = "multi_thread")]\n'
            "async fn spawns() {\n    run().await;\n}\n"
        )
        self.assertEqual(rust(src), [("no-assert", "spawns")])

    def test_rstest_and_test_case_functions_are_tests(self):
        rstest = "#[rstest]\n#[case(1)]\n#[case(2)]\nfn cases(#[case] n: u8) {\n    run(n);\n}\n"
        test_case = (
            '#[test_case(&[1, 2] ; "pair")]\n#[test_case(&[] ; "empty")]\n'
            "fn slices(input: &[u8]) {\n    run(input);\n}\n"
        )
        qualified = "#[test_case::test_case(1)]\nfn one(n: u8) {\n    run(n);\n}\n"
        self.assertEqual(rust(rstest), [("no-assert", "cases")])
        self.assertEqual(rust(test_case), [("no-assert", "slices")])
        self.assertEqual(rust(qualified), [("no-assert", "one")])

    def test_test_case_with_an_expected_value_asserts(self):
        src = "#[test_case(2 => 4)]\n#[test_case(3 => 9)]\nfn square(n: u8) -> u8 {\n    n * n\n}\n"
        self.assertEqual(rust(src), [])

    def test_cfg_test_module_is_not_a_test(self):
        self.assertEqual(rust("#[cfg(test)]\nmod tests {\n    fn helper() {}\n}\n"), [])

    def test_should_panic_counts_above_or_below_the_test_attribute(self):
        below = '#[test]\n#[should_panic(expected = "boom")]\nfn t() { run(); }\n'
        above = "#[should_panic]\n#[test]\nfn t() { run(); }\n"
        self.assertEqual(rust(below), [])
        self.assertEqual(rust(above), [])

    def test_does_not_panic_needs_the_suffix_and_a_comment_naming_the_panic(self):
        documented = (
            "// Guards the slice panic fixed in 2.20.\n"
            "#[test]\nfn long_input_does_not_panic() { run(); }\n"
        )
        undocumented = "#[test]\nfn long_input_does_not_panic() { run(); }\n"
        unrelated_comment = (
            "// ── long inputs ──\n#[test]\nfn long_input_does_not_panic() { run(); }\n"
        )
        wrong_name = "// Guards a panic.\n#[test]\nfn long_input_is_fine() { run(); }\n"
        self.assertEqual(rust(documented), [])
        self.assertEqual(
            rust(undocumented), [("no-assert", "long_input_does_not_panic")]
        )
        self.assertEqual(
            rust(unrelated_comment), [("no-assert", "long_input_does_not_panic")]
        )
        self.assertEqual(rust(wrong_name), [("no-assert", "long_input_is_fine")])


class RustHelpers(unittest.TestCase):
    def test_a_helper_defined_in_the_file_counts_when_it_asserts(self):
        src = "fn assert_shape(x: &X) {\n    assert_eq!(x.sides, 4);\n}\n\n" + test_fn(
            "assert_shape(&x);"
        )
        self.assertEqual(rust(src), [])

    def test_a_helper_that_asserts_through_another_helper_counts(self):
        src = (
            "fn assert_inner(x: &X) {\n    assert!(x.ok);\n}\n"
            "fn assert_outer(x: &X) {\n    assert_inner(x);\n}\n\n"
            + test_fn("assert_outer(&x);")
        )
        self.assertEqual(rust(src), [])

    def test_a_helper_that_asserts_nothing_does_not_count(self):
        src = "fn assert_shape(x: &X) {\n    log(x);\n}\n\n" + test_fn(
            "assert_shape(&x);"
        )
        self.assertEqual(rust(src), [("no-assert", "t")])

    def test_an_undefined_assert_named_call_does_not_count(self):
        self.assertEqual(rust(test_fn("assert_shape(&x);")), [("no-assert", "t")])
        self.assertEqual(rust(test_fn("x.assert_ready();")), [("no-assert", "t")])

    def test_a_helper_defined_elsewhere_in_the_crate_counts(self):
        src = test_fn("assert_shape(&x);")
        self.assertEqual(rust(src, helpers=frozenset({"assert_shape"})), [])

    def test_asserting_helpers_are_collected_from_a_source_file(self):
        src = (
            "pub fn assert_a(x: u8) { assert!(x > 0); }\n"
            "fn assert_b(x: u8) { log(x); }\n"
            "fn check_c(x: u8) { assert!(x > 0); }\n"
        )
        self.assertEqual(guard.asserting_helpers([src]), frozenset({"assert_a"}))

    def test_helpers_are_shared_across_the_files_of_one_crate_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, text in {
                "one/tests/common/mod.rs": "pub fn assert_shape(x: u8) { assert!(x > 0); }\n",
                "one/tests/it.rs": test_fn("common::assert_shape(1);"),
                "two/tests/it.rs": test_fn("assert_shape(1);"),
            }.items():
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding="utf-8")
            keys = [key for key, _rule, _line, _msg in guard.scan(root)]
        self.assertEqual(keys, ["two/tests/it.rs::t::no-assert"])


class RustResultTests(unittest.TestCase):
    signature = "fn t() -> Result<(), Box<dyn std::error::Error>>"

    def test_question_mark_with_an_assertion_passes(self):
        body = "let v = load()?;\n    assert_eq!(v, 1);\n    Ok(())"
        self.assertEqual(rust(test_fn(body, self.signature)), [])

    def test_ensure_bail_and_returned_errors_are_assertions_in_a_result_test(self):
        for body in (
            "let v = load()?;\n    anyhow::ensure!(v == 1);\n    Ok(())",
            'let v = load()?;\n    if v != 1 { bail!("wrong") }\n    Ok(())',
            'let v = load()?;\n    if v != 1 { return Err("wrong".into()); }\n    Ok(())',
        ):
            with self.subTest(body=body):
                self.assertEqual(rust(test_fn(body, self.signature)), [])

    def test_question_mark_alone_is_not_an_assertion(self):
        body = "load()?;\n    Ok(())"
        self.assertEqual(rust(test_fn(body, self.signature)), [("no-assert", "t")])

    def test_err_in_a_test_that_returns_nothing_is_not_an_assertion(self):
        self.assertEqual(
            rust(test_fn("let _ = Err::<(), _>(1);")), [("no-assert", "t")]
        )


class RustIgnore(unittest.TestCase):
    def test_bare_ignore_is_reported(self):
        src = "#[test]\n#[ignore]\nfn slow() { assert!(ok()); }\n"
        self.assertEqual(rust(src), [("bare-ignore", "slow")])

    def test_ignore_with_a_reason_passes(self):
        src = '#[test]\n#[ignore = "needs docker; run by the e2e job"]\nfn slow() { assert!(ok()); }\n'
        self.assertEqual(rust(src), [])

    def test_ignore_in_a_comment_is_not_reported(self):
        src = "// #[ignore]\n#[test]\nfn fast() { assert!(ok()); }\n"
        self.assertEqual(rust(src), [])

    def test_conditional_ignore_needs_a_reason_too(self):
        bare = '#[test]\n#[cfg_attr(target_os = "macos", ignore)]\nfn slow() { assert!(ok()); }\n'
        nested = "#[test]\n#[cfg_attr(any(miri, windows), ignore)]\nfn slow() { assert!(ok()); }\n"
        reasoned = '#[test]\n#[cfg_attr(miri, ignore = "miri has no sockets; the unit job runs it")]\nfn slow() { assert!(ok()); }\n'
        other = (
            "#[test]\n#[cfg_attr(miri, allow(unused))]\nfn fast() { assert!(ok()); }\n"
        )
        self.assertEqual(rust(bare), [("bare-ignore", "slow")])
        self.assertEqual(rust(nested), [("bare-ignore", "slow")])
        self.assertEqual(rust(reasoned), [])
        self.assertEqual(rust(other), [])


class RustSleep(unittest.TestCase):
    def test_sleep_in_a_test_is_reported_with_the_test_name(self):
        for call in (
            "std::thread::sleep(Duration::from_millis(50));",
            "tokio::time::sleep(Duration::from_millis(50)).await;",
            "sleep(Duration::from_millis(50));",
        ):
            with self.subTest(call=call):
                src = test_fn(f"{call}\n    assert!(ok());", "fn waits()")
                self.assertEqual(rust(src), [("bare-sleep", "waits")])

    def test_sleep_with_a_reason_comment_passes(self):
        same_line = test_fn(
            "sleep(d); // sleep: the fake server delays its reply\n    assert!(ok());"
        )
        line_above = test_fn(
            "// sleep: the fake server delays its reply\n    sleep(d);\n    assert!(ok());"
        )
        unrelated = test_fn("// wait a bit\n    sleep(d);\n    assert!(ok());")
        long_comment = test_fn(
            "// sleep: the fake server delays its reply,\n"
            "    // which is the fault under test.\n    sleep(d);\n    assert!(ok());"
        )
        detached = test_fn(
            "// sleep: explained\n    sleep(d);\n    run();\n    sleep(d);\n    assert!(ok());"
        )
        self.assertEqual(rust(same_line), [])
        self.assertEqual(rust(line_above), [])
        self.assertEqual(rust(long_comment), [])
        self.assertEqual(rust(detached), [("bare-sleep", "t")])
        self.assertEqual(rust(unrelated), [("bare-sleep", "t")])

    def test_sleep_on_a_paused_clock_passes(self):
        attr = test_fn(
            "tokio::time::sleep(d).await;\n    assert!(ok());",
            "async fn t()",
            "#[tokio::test(start_paused = true)]",
        )
        call = test_fn(
            "tokio::time::pause();\n    tokio::time::sleep(d).await;\n    assert!(ok());",
            "async fn t()",
            "#[tokio::test]",
        )
        self.assertEqual(rust(attr), [])
        self.assertEqual(rust(call), [])

    def test_sleep_outside_test_code_is_not_reported(self):
        src = (
            "pub async fn backoff(d: Duration) {\n    tokio::time::sleep(d).await;\n}\n"
        )
        self.assertEqual(rust(src, path="crate/src/retry.rs"), [])

    def test_sleep_in_a_test_module_helper_or_tests_directory_is_reported(self):
        module = (
            "pub fn run() {}\n\n#[cfg(test)]\nmod tests {\n"
            "    fn settle() {\n        std::thread::sleep(D);\n    }\n}\n"
        )
        helper = "pub fn settle() {\n    std::thread::sleep(D);\n}\n"
        self.assertEqual(
            rust(module, path="crate/src/lib.rs"), [("bare-sleep", "settle")]
        )
        self.assertEqual(
            rust(helper, path="crate/tests/common/mod.rs"), [("bare-sleep", "settle")]
        )

    def test_a_method_called_sleep_and_a_shell_sleep_are_not_reported(self):
        src = test_fn('clock.sleep(d);\n    run("sleep 1");\n    assert!(ok());')
        self.assertEqual(rust(src), [])


class RustEnv(unittest.TestCase):
    def test_raw_environment_changes_in_a_test_are_reported(self):
        for call in (
            'std::env::set_var("K", "v");',
            'env::remove_var("K");',
            'unsafe { std::env::set_var("K", "v") };',
        ):
            with self.subTest(call=call):
                src = test_fn(f"{call}\n    assert!(ok());", "fn configures()")
                self.assertEqual(rust(src), [("raw-env", "configures")])

    def test_env_guard_and_child_process_environment_pass(self):
        src = test_fn(
            "let mut env = EnvGuard::lock();\n"
            '    env.set("K", "v").remove("J");\n'
            '    Command::new("x").env("K", "v").env_remove("J");\n'
            "    assert!(ok());"
        )
        self.assertEqual(rust(src), [])

    def test_raw_environment_changes_outside_test_code_are_not_reported(self):
        src = 'pub fn init() {\n    std::env::set_var("K", "v");\n}\n'
        self.assertEqual(rust(src, path="crate/src/main.rs"), [])

    def test_the_env_guard_itself_may_change_the_environment(self):
        src = '#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        std::env::set_var("K", "v");\n        assert!(ok());\n    }\n}\n'
        self.assertEqual(rust(src, path="tumult-test-utils/src/env.rs"), [])
        self.assertEqual(rust(src, path="other/src/env.rs"), [("raw-env", "t")])


class Js(unittest.TestCase):
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

    def test_a_method_called_fit_is_not_a_focused_test(self):
        src = "chart.fit();\nthis.fit({ padding: 4 });\nmap?.fit(bounds);\n"
        self.assertEqual(guard.check_js(src), [])

    def test_focused_tests_in_comments_and_strings_are_not_reported(self):
        src = (
            "// test.only('a', () => {}) was used while debugging\n"
            "/* it.only('b', () => {});\n   fit('c', () => {}); */\n"
            "const hint = \"use test.only('x') locally\";\n"
            "const other = 'describe.only(';\n"
            "const tpl = `never commit it.only(`;\n"
            "test('real', () => {});\n"
        )
        self.assertEqual(guard.check_js(src), [])

    def test_code_after_a_comment_or_string_is_still_checked(self):
        src = (
            "const url = 'http://x'; // note\n"
            "/* block */ test.only('a', () => {});\n"
            "const s = \"it's\"; it.only('b', () => {});\n"
        )
        found = [(name, line) for _rule, name, line, _ in guard.check_js(src)]
        self.assertEqual(found, [("test.only", 2), ("it.only", 3)])


class Python(unittest.TestCase):
    def offences(self, src):
        return [(rule, name) for rule, name, _line, _msg in guard.check_python(src)]

    def test_skip_needs_a_reason(self):
        for decorator in (
            "@pytest.mark.skip",
            "@pytest.mark.skip()",
            '@pytest.mark.skip("")',
            '@pytest.mark.skip(reason="")',
            "@pytest.mark.skip(reason='  ')",
            "@unittest.skip",
            '@unittest.skip("")',
        ):
            with self.subTest(decorator=decorator):
                src = f"{decorator}\ndef test_a():\n    pass\n"
                self.assertEqual(self.offences(src), [("bare-skip", "test_a")])

    def test_skip_with_a_reason_passes(self):
        for decorator in (
            '@pytest.mark.skip(reason="needs the lab image")',
            '@pytest.mark.skip("needs the lab image")',
            "@unittest.skip('needs the lab image')",
            '@pytest.mark.skipif(sys.platform == "win32", reason="posix signals")',
            '@pytest.mark.xfail(reason="tracked in the lab backlog", strict=True)',
            '@pytest.mark.skipif(\n    not HAVE_DOCKER,\n    reason="needs docker",\n)',
            '@unittest.skipUnless(HAVE_DOCKER, "needs docker")',
        ):
            with self.subTest(decorator=decorator):
                src = f"{decorator}\ndef test_a():\n    pass\n"
                self.assertEqual(self.offences(src), [])

    def test_xfail_and_conditional_skip_need_a_reason(self):
        for decorator in (
            "@pytest.mark.xfail",
            "@pytest.mark.xfail()",
            "@pytest.mark.xfail(strict=True)",
            '@pytest.mark.xfail(reason="")',
            '@pytest.mark.skipif(sys.platform == "win32")',
            "@unittest.skipIf(WINDOWS, '')",
        ):
            with self.subTest(decorator=decorator):
                src = f"{decorator}\ndef test_a():\n    pass\n"
                self.assertEqual(self.offences(src), [("bare-skip", "test_a")])

    def test_skip_calls_need_a_reason(self):
        bare = 'def test_a():\n    if not ready():\n        pytest.skip("")\n'
        empty = "def test_b(self):\n    pytest.skip()\n"
        reasoned = 'def test_c():\n    pytest.skip("needs the lab image")\n'
        commented = "def test_d():\n    # pytest.skip()\n    assert ready()\n"
        self.assertEqual(self.offences(bare), [("bare-skip", "test_a")])
        self.assertEqual(self.offences(empty), [("bare-skip", "test_b")])
        self.assertEqual(self.offences(reasoned), [])
        self.assertEqual(self.offences(commented), [])


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
        self.write("crate/src/lib.rs", "#[test]\nfn t() { assert!(ok()); }\n")
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
        self.allow.write_text(
            "crate/src/lib.rs::known::no-assert  # deferred: crate in flight\n"
        )
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("fresh", out)
        self.assertNotIn("known", out)

    def test_an_allowlist_entry_covers_only_its_own_rule(self):
        self.write(
            "crate/src/lib.rs",
            "#[test]\n#[ignore]\nfn known() { run(); }\n",
        )
        self.allow.write_text("crate/src/lib.rs::known::no-assert  # deferred\n")
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("crate/src/lib.rs:2: bare-ignore: known:", out)
        self.assertNotIn("no-assert", out)

    def test_stale_allowlist_entry_fails_and_names_its_line(self):
        self.write("crate/src/lib.rs", "#[test]\nfn t() { assert!(ok()); }\n")
        self.allow.write_text(
            "# header\n\ncrate/src/lib.rs::gone::no-assert  # fixed since\n"
        )
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn(
            "allow.txt:3: stale-allow: crate/src/lib.rs::gone::no-assert", out
        )

    def test_allowlist_entry_without_a_known_rule_fails(self):
        self.write("crate/src/lib.rs", "#[test]\nfn known() { run(); }\n")
        for entry in ("crate/src/lib.rs::known", "crate/src/lib.rs::known::typo"):
            with self.subTest(entry=entry):
                self.allow.write_text(f"{entry}  # old format\n")
                code, out = self.run_guard()
                self.assertEqual(code, 1)
                self.assertIn(f"allow.txt:1: bad-allow: {entry}", out)

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

    def test_sleep_and_raw_env_in_a_tests_directory_fail(self):
        self.write(
            "crate/tests/it.rs",
            "fn settle() {\n    std::thread::sleep(D);\n}\n"
            'fn configure() {\n    std::env::set_var("K", "v");\n}\n',
        )
        code, out = self.run_guard()
        self.assertEqual(code, 1)
        self.assertIn("crate/tests/it.rs:2: bare-sleep: settle:", out)
        self.assertIn("crate/tests/it.rs:5: raw-env: configure:", out)

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
        self.assertEqual(listing.stdout, "crate/src/lib.rs::empty::no-assert\n")


class Repository(unittest.TestCase):
    def test_repository_passes_with_its_allowlist(self):
        out = io.StringIO()
        code = guard.run(guard.ROOT, guard.ALLOWLIST, out)
        self.assertEqual(code, 0, out.getvalue())


if __name__ == "__main__":
    unittest.main()
