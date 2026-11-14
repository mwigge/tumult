#!/usr/bin/env python3
"""Unit tests for check-log-messages.py (the structured-logging guard)."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-log-messages.py")


def load_checker():
    spec = importlib.util.spec_from_file_location("check_log_messages", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


checker = load_checker()


def rules(source):
    """The rule ids reported for `source`, in order."""
    return [finding.rule for finding in checker.check_source(source)]


class MaskTests(unittest.TestCase):
    def test_mask_keeps_length_and_line_structure(self):
        source = 'let a = "x(y"; // info!("{}", z)\n/* b\n c */ let d = 1;\n'
        masked = checker.mask_source(source)
        self.assertEqual(len(masked), len(source))
        self.assertEqual(masked.count("\n"), source.count("\n"))
        self.assertNotIn("info!", masked)
        self.assertNotIn("x(y", masked)

    def test_mask_handles_raw_strings_chars_and_lifetimes(self):
        source = 'fn f<\'a>(x: &\'a str) { let q = \'"\'; let r = r#"a "quoted" ("#; }'
        masked = checker.mask_source(source)
        self.assertEqual(len(masked), len(source))
        self.assertIn("fn f<'a>(x: &'a str)", masked)
        self.assertNotIn("quoted", masked)
        self.assertEqual(masked.count("("), 1)

    def test_mask_handles_nested_block_comments_and_escapes(self):
        source = '/* a /* b */ warn!("{}", c) */ let s = "a\\"b{}";'
        masked = checker.mask_source(source)
        self.assertNotIn("warn!", masked)
        self.assertNotIn("{}", masked)


class InterpolationTests(unittest.TestCase):
    def test_constant_message_with_fields_passes(self):
        source = """
        tracing::warn!(
            name: "auth.login.failed",
            { user.name = %user, "error.type" = "denied" },
            "login failed"
        );
        """
        self.assertEqual(rules(source), [])

    def test_positional_format_arguments_are_reported(self):
        source = 'tracing::info!(name: "a.b", "server done: {:?}", result);'
        self.assertEqual(rules(source), ["interpolated-message"])

    def test_inline_capture_is_reported(self):
        source = 'tracing::info!(name: "a.b", "server done: {result:?}");'
        self.assertEqual(rules(source), ["interpolated-message"])

    def test_escaped_braces_are_not_interpolation(self):
        source = 'tracing::info!(name: "a.b", "literal {{braces}} only");'
        self.assertEqual(rules(source), [])

    def test_braces_inside_field_values_are_not_interpolation(self):
        source = """
        tracing::info!(name: "a.b", { url.full = %format!("http://{host}") }, "ready");
        """
        self.assertEqual(rules(source), [])

    def test_bare_macros_and_event_macro_are_checked(self):
        source = """
        warn!(name: "a.b", "retry {attempt}");
        tracing::event!(name: "a.c", tracing::Level::INFO, "took {}ms", elapsed);
        """
        self.assertEqual(
            rules(source), ["interpolated-message", "interpolated-message"]
        )

    def test_message_split_over_lines_is_one_literal(self):
        source = """
        tracing::warn!(
            name: "tls.plaintext",
            "TLS is off: tokens cross the wire unencrypted; \\
             set the certificate"
        );
        """
        self.assertEqual(rules(source), [])

    def test_finding_reports_the_line_of_the_macro(self):
        source = '\n\n    tracing::error!(name: "a.b", "failed: {e}");\n'
        findings = checker.check_source(source)
        self.assertEqual(findings[0].line, 3)

    def test_other_macros_are_ignored(self):
        source = """
        compile_error!("not a log {}");
        my_error!("{}", x);
        tracing::info_span!("span {}", x);
        println!("{}", x);
        anyhow::bail!("bad {thing}");
        """
        self.assertEqual(rules(source), [])


class MacroFormTests(unittest.TestCase):
    def test_brace_delimited_macro_is_checked(self):
        self.assertEqual(
            rules('tracing::info! { name: "a.b", "done {}", n }'),
            ["interpolated-message"],
        )
        self.assertEqual(rules('info! { "started" }'), ["missing-event-name"])

    def test_bracket_delimited_macro_is_checked(self):
        self.assertEqual(
            rules('tracing::warn![name: "a.b", "retry {attempt}"];'),
            ["interpolated-message"],
        )
        self.assertEqual(rules('error! ["failed"];'), ["missing-event-name"])

    def test_delimiters_do_not_change_a_passing_event(self):
        for source in (
            'info!(name: "a.b", { rows = 3 }, "export complete");',
            'info! { name: "a.b", { rows = 3 }, "export complete" }',
            'info![name: "a.b", rows = 3, "export complete"];',
        ):
            self.assertEqual(rules(source), [], source)

    def test_computed_message_field_is_reported(self):
        for value in (
            '%format!("done {n}")',
            'format!("done {}", n)',
            "%text",
            "?failure",
            "text",
        ):
            source = f'tracing::info!(name: "a.b", message = {value});'
            self.assertEqual(rules(source), ["interpolated-message"], source)
        self.assertEqual(
            rules('tracing::info!(name: "a.b", { "message" = %format!("x {n}") });'),
            ["interpolated-message"],
        )
        self.assertEqual(
            rules('tracing::info!(name: "a.b", message = %format!("x {n}"), "done");'),
            ["interpolated-message"],
        )

    def test_constant_message_field_passes(self):
        self.assertEqual(
            rules('tracing::info!(name: "a.b", message = "export complete");'), []
        )

    def test_other_fields_may_be_computed(self):
        source = 'tracing::info!(name: "a.b", error.message = %format!("{e:#}"), "failed");'
        self.assertEqual(rules(source), [])


class StrictTests(unittest.TestCase):
    def test_missing_event_name_is_reported(self):
        self.assertEqual(rules('tracing::info!("started");'), ["missing-event-name"])

    def test_malformed_event_name_is_reported(self):
        source = 'tracing::info!(name: "Started", "started");'
        self.assertEqual(rules(source), ["bad-event-name"])

    def test_dotted_lowercase_event_names_pass(self):
        source = 'tracing::info!(name: "otel.exporter.empty_key", "started");'
        self.assertEqual(rules(source), [])

    def test_legacy_field_names_are_reported(self):
        source = """
        tracing::warn!(name: "a.b", error = %e, "failed");
        tracing::warn!(name: "a.c", %error, "failed");
        tracing::warn!(name: "a.d", { run_id = %id, path = %p.display() }, "failed");
        tracing::warn!(name: "a.e", ?e, "failed");
        """
        self.assertEqual(rules(source), ["legacy-field"] * 5)

    def test_vocabulary_field_names_pass(self):
        source = """
        tracing::warn!(
            name: "a.b",
            { error.message = %err(&e), "error.type" = "io", file.path = %p.display() },
            "failed"
        );
        tracing::info!(name: "a.c", { rows, files = n, attempt }, "done");
        """
        self.assertEqual(rules(source), [])

    def test_legacy_finding_names_the_replacement(self):
        finding = checker.check_source('warn!(name: "a.b", error = %e, "failed");')[0]
        self.assertIn("error.message", finding.detail)

    def test_non_strict_mode_only_reports_interpolation(self):
        source = 'tracing::info!(error = %e, "failed {x}");'
        found = [f.rule for f in checker.check_source(source, strict=False)]
        self.assertEqual(found, ["interpolated-message"])


class TestCodeTests(unittest.TestCase):
    def test_cfg_test_modules_are_skipped(self):
        source = """
        fn real() { tracing::info!(name: "a.b", "ok"); }

        #[cfg(test)]
        mod tests {
            #[test]
            fn t() { tracing::info!("fine in tests {}", 1); }
        }
        """
        self.assertEqual(rules(source), [])

    def test_cfg_test_items_are_skipped_but_following_code_is_not(self):
        source = """
        #[cfg(test)]
        fn helper() { tracing::info!("fine {}", 1); }

        #[cfg(test)]
        mod tests;

        fn real() { tracing::info!(name: "a.b", "bad {}", 1); }
        """
        self.assertEqual(rules(source), ["interpolated-message"])

    def test_test_paths_are_recognised(self):
        self.assertTrue(checker.is_test_path(Path("tumult-otel/tests/logging_init.rs")))
        self.assertTrue(checker.is_test_path(Path("tumultd/src/serve/tests.rs")))
        self.assertTrue(checker.is_test_path(Path("tumult-core/benches/run.rs")))
        self.assertFalse(checker.is_test_path(Path("tumultd/src/serve/mod.rs")))


class TreeTests(unittest.TestCase):
    def make_tree(self, root):
        for crate, body in {
            "good": 'fn f() { tracing::info!(name: "a.b", "ok"); }\n',
            "bad": 'fn f() { tracing::info!(name: "a.b", "no {}", 1); }\n',
        }.items():
            src = root / crate / "src"
            src.mkdir(parents=True)
            (root / crate / "Cargo.toml").write_text(f'[package]\nname = "{crate}"\n')
            (src / "lib.rs").write_text(body)
        (root / "target" / "debug").mkdir(parents=True)
        (root / "target" / "debug" / "x.rs").write_text('fn f() { info!("{}", 1); }\n')

    def test_check_tree_reports_only_non_deferred_crates(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_tree(root)
            findings = checker.check_tree(root, deferred={})
            self.assertEqual([f.path.as_posix() for f in findings], ["bad/src/lib.rs"])
            self.assertEqual(checker.check_tree(root, deferred={"bad": "later"}), [])

    def test_cli_exit_status_and_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.make_tree(root)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 1)
            self.assertIn("bad/src/lib.rs:1: interpolated-message", result.stdout)
            skipped = f"deferred, not checked ({len(checker.DEFERRED)}): "
            self.assertIn(skipped + ", ".join(sorted(checker.DEFERRED)), result.stdout)
            (root / "bad" / "src" / "lib.rs").write_text("fn f() {}\n")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout)
            self.assertIn(skipped, result.stdout)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), str(root / "good" / "src" / "lib.rs")],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout)
            self.assertIn(skipped, result.stdout)

    def test_self_test_passes(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--self-test"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_every_deferred_crate_names_its_follow_up(self):
        for crate, follow_up in checker.DEFERRED.items():
            self.assertTrue(follow_up.strip(), crate)


if __name__ == "__main__":
    unittest.main()
