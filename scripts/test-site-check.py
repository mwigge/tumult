#!/usr/bin/env python3
"""Tests for scripts/site-check.py. Each test builds a small site in a temp dir."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("site-check.py")
KNOWN = Path(__file__).with_name("site-known.txt")
BASELINE = Path(__file__).with_name("site-baseline.txt")
_spec = importlib.util.spec_from_file_location("site_check", SCRIPT)
site_check = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(site_check)

MERMAID_BLOCK = (
    '<div class="language-mermaid highlighter-rouge"><div class="highlight">'
    '<pre class="highlight"><code>flowchart LR\n  a --&gt; b\n</code></pre>'
    "</div></div>"
)
MERMAID_SCRIPT = (
    '<script src="https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js">'
    "</script>"
)
# What just-the-docs writes for `mermaid: version` 10 or later.
MERMAID_MODULE = (
    '<script type="module">\n'
    "  import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/"
    "mermaid.esm.min.mjs';\n"
    "  var config = {};\n"
    "  mermaid.initialize(config);\n"
    "  mermaid.run({\n    querySelector: '.language-mermaid',\n  });\n"
    "</script>"
)


def page(body, head=""):
    return f"<!DOCTYPE html><html><head>{head}</head><body>{body}</body></html>"


def redirect(url):
    return (
        '<!DOCTYPE html><html lang="en-US"><meta charset="utf-8">'
        f'<link rel="canonical" href="{url}">'
        f'<meta http-equiv="refresh" content="0; url={url}"></html>'
    )


def markdown(body, front="title: A page"):
    return f"---\n{front}\n---\n\n{body}"


class SiteTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.site = self.root / "_site"
        self.source = self.root / "docs"
        self.site.mkdir()

    def write(self, base, files):
        for name, text in files.items():
            path = base / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")

    def failures(self, files, source=None, baseline=()):
        self.write(self.site, files)
        if source is not None:
            self.write(self.source, source)
        found = site_check.check_site(
            self.site, self.source if source is not None else None, baseline
        )
        return [(failure.page, failure.rule) for failure in found]

    def messages(self, files, source=None, baseline=()):
        self.write(self.site, files)
        if source is not None:
            self.write(self.source, source)
        found = site_check.check_site(
            self.site, self.source if source is not None else None, baseline
        )
        return [failure.message for failure in found]


class RawMermaid(SiteTest):
    def test_a_mermaid_block_on_a_page_without_a_mermaid_script_fails(self):
        self.assertEqual(
            self.failures({"guides/flow.html": page(MERMAID_BLOCK)}),
            [("guides/flow.html", "raw-mermaid")],
        )

    def test_a_mermaid_block_on_a_page_that_loads_mermaid_passes(self):
        self.assertEqual(
            self.failures({"guides/flow.html": page(MERMAID_BLOCK, MERMAID_SCRIPT)}),
            [],
        )

    def test_a_mermaid_block_on_a_page_that_imports_mermaid_in_a_module_passes(self):
        self.assertEqual(
            self.failures({"guides/flow.html": page(MERMAID_BLOCK + MERMAID_MODULE)}),
            [],
        )

    def test_a_script_that_only_names_mermaid_does_not_load_it(self):
        script = '<script>console.log("mermaid is not here");</script>'
        self.assertEqual(
            self.failures({"guides/flow.html": page(MERMAID_BLOCK + script)}),
            [("guides/flow.html", "raw-mermaid")],
        )

    def test_a_page_with_no_mermaid_block_needs_no_mermaid_script(self):
        self.assertEqual(self.failures({"index.html": page("<p>Hello</p>")}), [])


class LiquidLeft(SiteTest):
    def test_liquid_output_left_in_the_text_fails(self):
        self.assertEqual(
            self.failures({"index.html": page("<p>Read {{ site.title }} now.</p>")}),
            [("index.html", "liquid-left")],
        )

    def test_a_liquid_tag_left_in_an_attribute_fails(self):
        html = page('<p><img src="{% link images/a.png %}" alt="a"></p>')
        self.assertIn(
            ("index.html", "liquid-left"), self.failures({"index.html": html})
        )

    def test_braces_inside_code_are_not_read(self):
        html = page("<pre><code>docker ps --format {{.Names}}</code></pre>")
        self.assertEqual(self.failures({"index.html": html}), [])


class LiquidAteBraces(SiteTest):
    GRAFANA = markdown(
        '```\n{service_name="tumult"} | line_format "{{.event}} {{.status}}"\n```\n'
    )

    def test_a_go_template_in_a_rendered_page_fails_with_its_source_line(self):
        site = {"guides/grafana.html": page('<pre><code>line_format " "</code></pre>')}
        source = {"guides/grafana.md": self.GRAFANA}
        self.assertEqual(
            self.failures(site, source),
            [
                ("guides/grafana.html", "liquid-ate-braces"),
                ("guides/grafana.html", "liquid-ate-braces"),
            ],
        )
        self.assertIn("guides/grafana.md:6", self.messages({}, {})[0])
        self.assertIn("{{.event}}", self.messages({}, {})[0])

    def test_a_go_template_between_raw_tags_passes(self):
        source = {
            "guides/grafana.md": markdown(
                '{% raw %}\nline_format "{{.event}}"\n{% endraw %}\n'
            )
        }
        site = {"guides/grafana.html": page("<p>x</p>")}
        self.assertEqual(self.failures(site, source), [])

    def test_jekyll_variables_and_names_the_page_sets_pass(self):
        body = (
            "{{ site.title }} {{- page.title | escape -}} {{ 'x' | upcase }}\n"
            "{% assign n = 3 %}{{ n }}\n"
            "{% for p in site.pages %}{{ p.url }}{% endfor %}\n"
        )
        site = {"index.html": page("<p>x</p>")}
        self.assertEqual(self.failures(site, {"index.md": markdown(body)}), [])

    def test_a_source_with_no_built_page_is_not_read(self):
        source = {"architecture/a.md": markdown("{{.Name}}")}
        self.assertEqual(self.failures({}, source), [])

    def test_a_markdown_page_without_front_matter_is_read(self):
        source = {"testprotocol.md": "# Protocol\n\ndocker ps --format {{.Names}}\n"}
        site = {"testprotocol.html": page("<p>docker ps --format</p>")}
        self.assertEqual(
            self.failures(site, source), [("testprotocol.html", "liquid-ate-braces")]
        )

    def test_an_html_page_without_front_matter_is_not_read(self):
        source = {"watch/index.html": "<p>{{.Name}}</p>"}
        site = {"watch/index.html": page("<pre>{{.Name}}</pre>")}
        self.assertEqual(self.failures(site, source), [])

    def test_a_permalink_names_the_built_page(self):
        source = {"start.md": markdown("{{ .Name }}", "permalink: /get-started/")}
        site = {"get-started/index.html": page("<p>x</p>")}
        self.assertEqual(
            self.failures(site, source),
            [("get-started/index.html", "liquid-ate-braces")],
        )


class BrokenLinks(SiteTest):
    def test_a_link_to_a_missing_page_fails_with_the_link(self):
        html = page('<a href="../reference/gone.html">gone</a>')
        self.assertEqual(
            self.failures({"guides/a.html": html}),
            [("guides/a.html", "broken-link")],
        )
        self.assertIn("../reference/gone.html", self.messages({})[0])

    def test_a_missing_image_fails(self):
        html = page('<img src="/images/gone.png" alt="gone">')
        self.assertEqual(
            self.failures({"index.html": html}), [("index.html", "broken-link")]
        )

    def test_links_that_github_pages_serves_pass(self):
        html = page(
            '<a href="/guides/">a</a> <a href="/guides">b</a> <a href="b">c</a>'
            ' <a href="b.html#top">d</a> <a href="./b.html?x=1">e</a>'
            ' <a href="#here">f</a> <a href="">g</a> <a href="/images/a%20b.png">h</a>'
            ' <a href="https://example.com/x">i</a> <a href="//cdn.example.com/y">j</a>'
            ' <a href="mailto:a@example.com">k</a> <img src="data:image/png;base64,AA">'
            ' <a href="https://tumult.rs/guides/b.html">l</a>',
            '<link rel="stylesheet" href="/assets/main.css">',
        )
        files = {
            "CNAME": "tumult.rs\n",
            "guides/index.html": page("<p>home</p>"),
            "guides/a.html": html,
            "guides/b.html": page("<p>b</p>"),
            "images/a b.png": "png",
            "assets/main.css": "body{}",
        }
        self.assertEqual(self.failures(files), [])

    def test_a_link_to_a_missing_page_of_the_site_host_fails(self):
        html = page('<a href="https://tumult.rs/gone.html">gone</a>')
        files = {"CNAME": "tumult.rs\n", "index.html": html}
        self.assertEqual(self.failures(files), [("index.html", "broken-link")])

    def test_a_link_out_of_the_site_dir_fails(self):
        (self.root / "secret.html").write_text("x", encoding="utf-8")
        html = page('<a href="/%2e%2e/secret.html">out</a>')
        self.assertEqual(
            self.failures({"index.html": html}), [("index.html", "broken-link")]
        )


class Baseline(SiteTest):
    def test_a_page_of_the_baseline_that_is_gone_fails(self):
        files = {"index.html": page("<p>x</p>")}
        self.assertEqual(
            self.failures(files, baseline=["index.html", "guides/old.html"]),
            [("guides/old.html", "missing-page")],
        )

    def test_a_page_of_the_baseline_that_redirects_passes(self):
        files = {
            "CNAME": "tumult.rs\n",
            "guides/old.html": redirect("https://tumult.rs/get-started/"),
            "get-started/index.html": page("<p>new</p>"),
        }
        self.assertEqual(self.failures(files, baseline=["guides/old.html"]), [])

    def test_a_redirect_to_a_missing_page_fails(self):
        files = {
            "CNAME": "tumult.rs\n",
            "guides/old.html": redirect("https://tumult.rs/get-started/"),
        }
        self.assertCountEqual(
            self.failures(files, baseline=["guides/old.html"]),
            [("guides/old.html", "missing-page"), ("guides/old.html", "broken-link")],
        )


class Known(unittest.TestCase):
    FAILURES = [
        site_check.Failure("a.html", "raw-mermaid", "raw"),
        site_check.Failure("b.html", "broken-link", "broken"),
    ]

    def split(self, text):
        return site_check.split_known(self.FAILURES, text, "known.txt")

    def test_a_known_failure_is_allowed_and_the_others_are_not(self):
        new, allowed = self.split("a.html::raw-mermaid  # mermaid is not set yet\n")
        self.assertEqual([f.page for f in new], ["b.html"])
        self.assertEqual([f.page for f in allowed], ["a.html"])

    def test_an_entry_that_matches_no_failure_fails_as_stale(self):
        new, _allowed = self.split("c.html::raw-mermaid  # fixed long ago\n")
        self.assertIn(("known.txt:1", "stale-known"), [(f.page, f.rule) for f in new])

    def test_an_entry_without_a_reason_fails(self):
        new, allowed = self.split("# a comment\n\na.html::raw-mermaid\n")
        self.assertIn(("known.txt:3", "bad-known"), [(f.page, f.rule) for f in new])
        self.assertEqual(allowed, [])

    def test_an_entry_with_an_unknown_rule_fails(self):
        new, _allowed = self.split("a.html::raw-mermiad  # typo\n")
        self.assertIn(("known.txt:1", "bad-known"), [(f.page, f.rule) for f in new])

    def test_an_overflow_entry_is_left_to_the_picture_script(self):
        new, _allowed = self.split("index.html::overflow-390  # the install line\n")
        self.assertEqual([f.rule for f in new], ["raw-mermaid", "broken-link"])


class Command(SiteTest):
    def run_check(self, *args):
        return subprocess.run(
            [sys.executable, "-B", str(SCRIPT), str(self.site), *map(str, args)],
            capture_output=True,
            text=True,
            check=False,
        )

    def test_a_failure_is_printed_with_its_page_and_reason_and_exits_one(self):
        self.write(self.site, {"guides/flow.html": page(MERMAID_BLOCK)})
        result = self.run_check()
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("guides/flow.html: raw-mermaid: ", result.stdout)

    def test_a_known_failure_is_printed_and_exits_zero(self):
        self.write(self.site, {"guides/flow.html": page(MERMAID_BLOCK)})
        known = self.root / "known.txt"
        known.write_text("guides/flow.html::raw-mermaid  # set later\n", "utf-8")
        result = self.run_check("--known", known)
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertIn("known: guides/flow.html: raw-mermaid: ", result.stdout)

    def test_a_baseline_page_that_is_gone_exits_one(self):
        self.write(self.site, {"index.html": page("<p>x</p>")})
        baseline = self.root / "baseline.txt"
        baseline.write_text("# pages\nindex.html\nreleases/index.html\n", "utf-8")
        result = self.run_check("--baseline", baseline)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("releases/index.html: missing-page: ", result.stdout)

    def test_a_site_dir_that_does_not_exist_exits_two(self):
        result = subprocess.run(
            [sys.executable, "-B", str(SCRIPT), str(self.root / "none")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)


class RepositoryFiles(unittest.TestCase):
    def test_every_entry_of_the_known_file_has_a_rule_and_a_reason(self):
        text = KNOWN.read_text(encoding="utf-8")
        new, _allowed = site_check.split_known([], text, KNOWN.name)
        problems = [f for f in new if f.rule == "bad-known"]
        self.assertEqual(problems, [])

    def test_the_baseline_names_html_pages_of_the_site(self):
        entries = site_check.read_baseline(BASELINE.read_text(encoding="utf-8"))
        self.assertGreater(len(entries), 80)
        self.assertEqual([e for e in entries if not e.endswith(".html")], [])


if __name__ == "__main__":
    unittest.main()
