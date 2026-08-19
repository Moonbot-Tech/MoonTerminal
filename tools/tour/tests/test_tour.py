"""Checks for the tour generator. Run: ``python -m unittest discover tools/tour/tests``.

Deliberately plain ``unittest`` so the only third-party dependency stays PyYAML.

Every test here can genuinely fail; each was confirmed by mutating the thing it
guards and watching it redden.

The one that earns its keep most is *locale fidelity*: it compares the rendered
page against ``locales/*.yml`` rather than against another copy of the same
data, so it is the only assertion here whose two sides have independent origins.
*Theme fidelity* is weaker on purpose and says so in its own docstring.
"""

from __future__ import annotations

import re
import shutil
import sys
import tempfile
import unittest
from collections.abc import Callable
from contextlib import contextmanager
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tour import emit, paths, render as render_mod  # noqa: E402
from tour.content import load as load_content  # noqa: E402
from tour.errors import ContentError, TourError  # noqa: E402
from tour.locales import load as load_locales  # noqa: E402
from tour.theme import load as load_theme, resolve as resolve_theme  # noqa: E402

@contextmanager
def broken_content(filename: str, mutate: Callable[[dict], None]):
    """A copy of the real content with ONE deliberate defect.

    Built at test time rather than committed as fixture files: a committed copy
    of eight content files would drift the moment anyone edits the real ones,
    and these tests would go on passing against data nobody maintains.
    """
    with tempfile.TemporaryDirectory() as tmp:
        target = Path(tmp)
        for source in paths.CONTENT_DIR.glob("*.yml"):
            shutil.copy(source, target / source.name)

        path = target / filename
        head, body = path.read_text(encoding="utf-8").split("_schema:", 1)
        doc = yaml.safe_load("_schema:" + body)
        mutate(doc)
        path.write_text(
            head + yaml.safe_dump(doc, allow_unicode=True, sort_keys=False),
            encoding="utf-8",
        )
        yield target


def build() -> tuple[str, object, object]:
    themes = load_theme(resolve_theme())
    locales = load_locales(paths.LOCALES_DIR)
    content = load_content(paths.CONTENT_DIR, locales)
    content.problems.raise_if_any("content is not usable")
    template = paths.TEMPLATE.read_text(encoding="utf-8")
    return render_mod.render(template, content, themes).page, content, locales


class Determinism(unittest.TestCase):
    def test_two_renders_are_byte_identical(self):
        first, _, _ = build()
        second, _, _ = build()
        self.assertEqual(first, second)

    def test_output_is_lf_only_with_one_trailing_newline(self):
        page, _, _ = build()
        self.assertNotIn("\r", page)
        self.assertTrue(page.endswith("\n"))
        self.assertFalse(page.endswith("\n\n"))

    def test_committed_page_matches_a_fresh_render(self):
        page, _, _ = build()
        committed = paths.OUTPUT.read_text(encoding="utf-8")
        self.assertEqual(
            committed, page, "docs/tour/index.html is stale — run `make tour`"
        )


class Output(unittest.TestCase):
    def test_no_template_slot_survives(self):
        page, _, _ = build()
        self.assertEqual(re.findall(r"\{\{\w+\}\}", page), [])

    def test_no_interpolation_placeholder_survives(self):
        # 672 locale values carry %{...}; one reaching the page is always a bug.
        page, _, _ = build()
        self.assertEqual(re.findall(r"%\{\w+\}", page), [])

    def test_script_block_is_not_broken_by_content(self):
        page, _, _ = build()
        self.assertEqual(page.count("<script>"), 1)
        self.assertEqual(page.count("</script>"), 1)

    def test_page_is_self_contained(self):
        page, _, _ = build()
        self.assertEqual(re.findall(r"<script[^>]+src=", page), [])
        for href in re.findall(r'<link[^>]+href="([^"]+)"', page):
            self.assertTrue(
                href.startswith("https://fonts.g"),
                f"unexpected external stylesheet: {href}",
            )
        self.assertNotIn("http://", page)


class LocaleFidelity(unittest.TestCase):
    def test_every_locale_slot_matches_the_locale_file(self):
        _, content, locales = build()
        checked = 0
        for key, text in content.page.items():
            if not text.locale_key:
                continue
            for code in content.codes:
                self.assertEqual(
                    text.get(code),
                    locales.get(text.locale_key, code),
                    f"page key {key!r} drifted from {text.locale_key!r} [{code}]",
                )
                checked += 1
        for zone in content.zones:
            for field in ("title", "body"):
                text = zone[field]
                if not text.locale_key:
                    continue
                for code in content.codes:
                    self.assertEqual(
                        text.get(code), locales.get(text.locale_key, code)
                    )
                    checked += 1
        self.assertGreater(checked, 0, "no locale-backed slots — the wiring is dead")


class LanguageCompleteness(unittest.TestCase):
    def test_every_slot_has_every_configured_language(self):
        _, content, _ = build()
        for key, text in content.page.items():
            for code in content.codes:
                self.assertTrue(text.get(code).strip(), f"page.{key} has no {code}")

    def test_a_missing_language_is_reported_not_dropped(self):
        """The negative case. Without it the positive one proves nothing."""

        def drop_english(doc):
            doc["strings"]["hero.h1"].pop("en")

        locales = load_locales(paths.LOCALES_DIR)
        with broken_content("page.yml", drop_english) as content_dir:
            with self.assertRaises(ContentError) as caught:
                content = load_content(content_dir, locales)
                content.problems.raise_if_any("content is not usable", ContentError)
        message = str(caught.exception)
        self.assertIn("hero.h1", message)
        self.assertIn("'en'", message)

    def test_an_unknown_locale_key_is_reported_with_a_suggestion(self):
        def typo_the_key(doc):
            for zone in doc["zones"]:
                if zone["id"] == "live":
                    zone["body"] = {"locale": "toolbar.live_tipp"}
                    return
            raise AssertionError("zone 'live' is gone — update this test")

        locales = load_locales(paths.LOCALES_DIR)
        with broken_content("zones.yml", typo_the_key) as content_dir:
            with self.assertRaises(TourError) as caught:
                content = load_content(content_dir, locales)
                content.problems.raise_if_any("content is not usable", ContentError)
        message = str(caught.exception)
        self.assertIn("unknown key", message)
        self.assertIn("toolbar.live_tip", message)   # the suggestion, not the typo


class ThemeFidelity(unittest.TestCase):
    """Proves the CSS emitter, NOT that the snapshot is current.

    Both sides of these assertions come from the same snapshot, so a stale
    snapshot passes. What they do catch is an emitter bug — the dark palette
    written into the light block, a token mapped to the wrong variable, a
    metric emitted without its scale factor. Whether the snapshot still
    matches MoonUI is checked by refreshing it (`make tour-theme`), and
    nothing inside this repository can check it, because MoonUI is a git
    dependency with no file path here.
    """

    def test_css_variables_match_the_theme_snapshot(self):
        page, content, _ = build()
        themes = load_theme(resolve_theme())

        light_block = page[page.index(":root{") : page.index("@media(prefers-color-scheme")]
        dark_block = page[page.index(':root[data-theme="dark"]{') :]
        dark_block = dark_block[: dark_block.index("\n}")]

        checked = 0
        for var in content.css["palette_vars"]:
            token = var.replace("-", "_")
            for block, theme in ((light_block, themes.light), (dark_block, themes.dark)):
                found = re.search(rf"--{re.escape(var)}:(#[0-9a-f]{{6}});", block)
                self.assertIsNotNone(found, f"--{var} missing from the CSS")
                self.assertEqual(
                    found.group(1), theme.palette[token], f"--{var} drifted from the theme"
                )
                checked += 1
        self.assertEqual(checked, len(content.css["palette_vars"]) * 2)

    def test_metrics_match_the_theme_snapshot(self):
        page, content, _ = build()
        themes = load_theme(resolve_theme())
        for var, token in content.css["metric_vars"].items():
            found = re.search(rf"--{re.escape(var)}:(?:calc\()?([0-9.]+)px", page)
            self.assertIsNotNone(found, f"--{var} missing")
            self.assertEqual(float(found.group(1)), themes.light.metrics[token])


class Structure(unittest.TestCase):
    def test_zone_numbers_cover_the_range_with_no_gaps(self):
        _, content, _ = build()
        numbers = sorted(z["n"] for z in content.zones)
        self.assertEqual(numbers, list(range(1, len(content.zones) + 1)))

    def test_zone_ids_are_unique(self):
        _, content, _ = build()
        ids = [z["id"] for z in content.zones]
        self.assertEqual(len(ids), len(set(ids)))

    def test_every_clickable_region_has_a_zone(self):
        page, content, _ = build()
        markup = re.sub(r"<script>.*?</script>", "", page, flags=re.S)
        ids = {z["id"] for z in content.zones}
        for zid in set(re.findall(r'data-zone="([^"]+)"', markup)):
            self.assertIn(zid, ids)

    def test_header_figures_are_counted_not_typed(self):
        page, content, _ = build()
        block = page[page.index("const STATS") : page.index("$(\"#stats\")")]
        figures = re.findall(r'"(\d+)",', block)
        self.assertEqual(figures[0], str(len(content.panels)))
        self.assertEqual(figures[1], str(len(content.windows)))
        self.assertEqual(
            figures[2], str(sum(len(g["rows"]) for g in content.hotkeys))
        )


class Escaping(unittest.TestCase):
    def test_a_script_close_in_content_cannot_end_the_block(self):
        literal = emit.js_literal({"k": "</script><img onerror=x>"})
        self.assertNotIn("</script", literal)

    def test_a_javascript_line_separator_is_escaped(self):
        literal = emit.js_literal({"k": "a" + chr(0x2028) + "b"})
        self.assertNotIn(chr(0x2028), literal)
        self.assertIn("u2028", literal)

    def test_markup_is_opt_in(self):
        """Only the quick-start bodies may carry raw markup."""
        _, content, _ = build()
        raw = [k for k, t in content.page.items() if t.html]
        self.assertEqual(raw, [], "page chrome must not carry markup")
        self.assertTrue(all(s["body"].html for s in content.steps))


if __name__ == "__main__":
    unittest.main()
