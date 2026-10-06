"""Keep unparseable inline JavaScript out of the tour and Pages artifact."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from html.parser import HTMLParser
from pathlib import Path
from unittest.mock import patch

from tools.tour import paths


class InlineScripts(HTMLParser):
    """Collect script bodies without decoding entities or including external scripts."""

    def __init__(self) -> None:
        """Initialize an independent body buffer for each inline script."""
        super().__init__(convert_charrefs=False)
        self.scripts: list[tuple[str, bool]] = []
        self.body: list[str] | None = None
        self.module = False

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Select inline scripts by presence of src, including an empty src attribute."""
        if tag == "script":
            attributes = dict(attrs)
            self.body = [] if "src" not in attributes else None
            self.module = attributes.get("type", "") == "module"

    def handle_data(self, data: str) -> None:
        """Retain script text verbatim for Node's parser."""
        if self.body is not None:
            self.body.append(data)

    def handle_endtag(self, tag: str) -> None:
        """Finish a script without combining its declarations with another script."""
        if tag == "script" and self.body is not None:
            self.scripts.append(("".join(self.body), self.module))
            self.body = None


def require_node() -> str:
    """Find Node; skip locally when absent, but fail whenever CI is set."""
    node = shutil.which("node")
    if node is not None:
        return node
    reason = "node is required to check inline JavaScript syntax"
    if "CI" in os.environ:
        raise AssertionError(reason + " in CI")
    raise unittest.SkipTest(reason + "; install Node.js to run this guard locally")


def check_inline_scripts(page: str, label: str, node: str) -> int:
    """Parse every inline script independently; report the page and script on failure."""
    parser = InlineScripts()
    parser.feed(page)
    parser.close()
    with tempfile.TemporaryDirectory() as tmp:
        for index, (body, module) in enumerate(parser.scripts, start=1):
            suffix = "mjs" if module else "cjs"
            script = Path(tmp) / f"inline-{index}.{suffix}"
            script.write_text(body, encoding="utf-8")
            result = subprocess.run(
                [node, "--check", str(script)],
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
            )
            if result.returncode != 0:
                raise AssertionError(
                    f"{label}: inline script {index} failed node --check\n"
                    f"{result.stdout}{result.stderr}"
                )
    return len(parser.scripts)


class ScriptSyntax(unittest.TestCase):
    """Use the real JS parser to guard the exact build entry used by Pages."""

    def test_built_site_and_committed_tour_parse(self) -> None:
        """A malformed template comment must fail before empty lists reach Pages."""
        node = require_node()
        with tempfile.TemporaryDirectory() as tmp:
            site = Path(tmp) / "site"
            result = subprocess.run(
                [sys.executable, "-m", "tools.tour", "--site-out", str(site)],
                check=False,
                cwd=paths.REPO_ROOT,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=120,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            pages = [paths.OUTPUT, *sorted(site.rglob("*.html"))]
            self.assertIn(site / "index.html", pages)
            self.assertIn(site / "knowledge" / "index.html", pages)
            for page in pages:
                with self.subTest(page=str(page)):
                    count = check_inline_scripts(
                        page.read_text(encoding="utf-8"), str(page), node
                    )
                    if page in (paths.OUTPUT, site / "index.html"):
                        self.assertGreater(
                            count, 0, "tour inline script was not checked"
                        )

    def test_broken_dictionary_comment_is_rejected(self) -> None:
        """The premature comment terminator from #900 must trigger the same guard."""
        node = require_node()
        page = (
            "<script>/* Dictionary source: locales/*/*.yml */ const data = {};</script>"
        )
        with self.assertRaisesRegex(
            AssertionError, "fixture: inline script 1"
        ) as caught:
            check_inline_scripts(page, "fixture", node)
        self.assertIn("SyntaxError", str(caught.exception))

    def test_multiple_scripts_and_external_src(self) -> None:
        """Skipping external scripts must not hide a broken later inline script."""
        node = require_node()
        page = (
            '<SCRIPT SRC="">invalid JavaScript *</SCRIPT>'
            '<script>const text = "&amp;"; throw new Error("must not execute");</script>'
            '<script type="module">export const value = 1;</script>'
        )
        self.assertEqual(check_inline_scripts(page, "fixture", node), 2)
        with self.assertRaisesRegex(AssertionError, "fixture: inline script 3"):
            check_inline_scripts(
                page + "<script>const broken = ;</script>", "fixture", node
            )

    def test_missing_node_skips_locally_but_fails_in_ci(self) -> None:
        """An absent parser must never turn the CI deployment guard into a skip."""
        with patch("shutil.which", return_value=None):
            with (
                patch.dict(os.environ, {}, clear=True),
                self.assertRaisesRegex(unittest.SkipTest, "install Node.js"),
            ):
                require_node()
            with (
                patch.dict(os.environ, {"CI": ""}, clear=True),
                self.assertRaisesRegex(AssertionError, "required.*in CI"),
            ):
                require_node()
