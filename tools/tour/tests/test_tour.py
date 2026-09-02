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

import copy
import io
import json
import posixpath
import re
import shutil
import sys
import tempfile
import unittest
from collections.abc import Callable
from contextlib import contextmanager, redirect_stderr
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from tour import emit, knowledge, paths
from tour import map as map_mod
from tour import render as render_mod
from tour.__main__ import main as tour_main
from tour.content import Language
from tour.content import load as load_content
from tour.errors import ContentError, Problems, TourError
from tour.locales import load as load_locales
from tour.theme import load as load_theme
from tour.theme import resolve as resolve_theme


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


class KnowledgeBundle(unittest.TestCase):
    """Checks the complete artifact uploaded by the Pages workflow."""

    def bundle(self):
        """Build the knowledge artifact through the same inputs as Pages."""
        themes = load_theme(resolve_theme())
        locales = load_locales(paths.LOCALES_DIR)
        content = load_content(paths.CONTENT_DIR, locales)
        content.problems.raise_if_any("content is not usable")
        template = paths.KNOWLEDGE_TEMPLATE.read_text(encoding="utf-8")
        return knowledge.build(content, themes, template), content

    def test_two_bundles_are_byte_identical(self):
        """The same repository inputs must produce byte-identical files."""
        first, _ = self.bundle()
        second, _ = self.bundle()
        self.assertEqual(first.files, second.files)

    def test_configured_language_is_rendered_without_renderer_changes(self):
        """Adding a validated language must change content, not public paths or code."""
        _, original = self.bundle()
        content = copy.deepcopy(original)
        content.languages.append(Language(code="es", label="ES", html_lang="es"))
        texts = list(content.page.values())
        for mode in content.modes:
            texts.extend((mode["label"], mode["tip"], mode["lead"]))
        for zone in content.zones:
            texts.extend((zone["title"], zone["body"]))
        for step in content.steps:
            texts.extend((step["title"], step["body"]))
        for panel in content.panels:
            texts.append(panel["description"])
        for window in content.windows:
            texts.extend((window["title"], window["body"]))
        for group in content.hotkeys:
            texts.append(group["caption"])
            texts.extend(row["action"] for row in group["rows"])
        for text in texts:
            text.values["es"] = f"ES: {text.values['en']}"
        themes = load_theme(resolve_theme())
        template = paths.KNOWLEDGE_TEMPLATE.read_text(encoding="utf-8")
        bundle = knowledge.build(content, themes, template)
        for path in (
            Path("kb/getting-started.md"),
            Path("kb/interface.md"),
            Path("kb/panels.md"),
            Path("kb/windows.md"),
        ):
            self.assertIn("### es:", bundle.files[path], path)
        self.assertIn("**es:**", bundle.files[Path("kb/hotkeys.md")])
        self.assertIn("languages: [ru, en, es]", bundle.files[Path("kb/index.md")])

    def test_bundle_has_the_public_contract_files(self):
        """The URLs advertised to agents must remain a stable public contract."""
        bundle, _ = self.bundle()
        expected = {
            Path("knowledge/index.html"),
            Path("knowledge.jsonl"),
            Path("llms.txt"),
            Path("llms-full.txt"),
            Path("robots.txt"),
            Path("sitemap.xml"),
            Path("kb/index.md"),
            Path("kb/getting-started.md"),
            Path("kb/interface.md"),
            Path("kb/panels.md"),
            Path("kb/windows.md"),
            Path("kb/hotkeys.md"),
        }
        self.assertEqual(set(bundle.files), expected)

    def test_markdown_front_matter_declares_identity_and_limits(self):
        """Every topic must expose machine-readable identity without volatile metadata."""
        bundle, _ = self.bundle()
        for path, body in bundle.files.items():
            if path.suffix != ".md":
                continue
            self.assertTrue(body.startswith("---\n"), path)
            metadata = yaml.safe_load(body.split("---", 2)[1])
            self.assertEqual(metadata["schema"], "moonterminal.tour.topic.v1", path)
            self.assertEqual(metadata["primary_language"], "ru", path)
            self.assertEqual(metadata["languages"], ["ru", "en"], path)
            self.assertEqual(metadata["coverage"]["count"] > 0, True, path)
            self.assertNotIn("generated_at", metadata, path)
            self.assertNotIn("last_verified_at", metadata, path)
            self.assertTrue(
                any("not documented" in limit for limit in metadata["limitations"]),
                path,
            )

    def test_jsonl_contains_every_curated_fact(self):
        """Every authored tour fact must appear exactly once in the search corpus."""
        bundle, content = self.bundle()
        rows = [json.loads(line) for line in bundle.files[Path("knowledge.jsonl")].splitlines()]
        expected = (
            len(content.steps)
            + len(content.zones)
            + len(content.panels)
            + len(content.windows)
            + sum(len(group["rows"]) for group in content.hotkeys)
        )
        self.assertEqual(len(rows), expected)
        self.assertTrue(all(set(row["title"]) == set(content.codes) for row in rows))
        self.assertTrue(all("source" not in row for row in rows))
        self.assertTrue(
            all(
                row["provenance"][part]["source_type"]
                in {"authored-tour", "locale-literal"}
                for row in rows
                for part in ("title", "text")
            )
        )

    def test_llms_index_points_only_to_generated_or_human_pages(self):
        """An agent following llms.txt must not land on a missing local URL."""
        bundle, _ = self.bundle()
        llms = bundle.files[Path("llms.txt")]
        public_paths = re.findall(
            rf"{re.escape(knowledge.PUBLIC_BASE)}/([^\s)]+)", llms
        )
        generated = {path.as_posix() for path in bundle.files}
        generated.update({"", "knowledge/"})
        self.assertTrue(public_paths)
        self.assertEqual([path for path in public_paths if path not in generated], [])

    def test_every_published_link_resolves_from_its_actual_url(self):
        """Moving Markdown into llms-full must not leave relative links behind."""
        bundle, _ = self.bundle()
        generated = {path.as_posix() for path in bundle.files}
        generated.add("index.html")
        missing = []
        markdown_link = re.compile(r"\[[^\]]*\]\(([^)\s]+)")
        for source, body in bundle.files.items():
            links = markdown_link.findall(body)
            if source.suffix == ".html":
                links.extend(re.findall(r'href="([^"]+)"', body))
            for href in links:
                target = href.split("#", 1)[0].split("?", 1)[0]
                if href.startswith(knowledge.PUBLIC_BASE):
                    target = target.removeprefix(knowledge.PUBLIC_BASE).lstrip("/")
                elif re.match(r"^[a-z][a-z0-9+.-]*:", href, re.IGNORECASE):
                    continue
                else:
                    target = posixpath.normpath(
                        posixpath.join(source.parent.as_posix(), target)
                    )
                if not target or target.endswith("/") or target == ".":
                    target = posixpath.join(target, "index.html")
                target = posixpath.normpath(target)
                if target not in generated:
                    missing.append((source.as_posix(), href, target))
        self.assertEqual(missing, [])

    def test_markdown_does_not_leak_quickstart_html(self):
        """Allowed browser markup must become ordinary machine-readable Markdown."""
        bundle, _ = self.bundle()
        quickstart = bundle.files[Path("kb/getting-started.md")]
        self.assertNotIn("<code>", quickstart)
        self.assertNotIn("<a ", quickstart)
        self.assertIn("[Releases](https://github.com/Moonbot-Tech/MoonTerminal/releases/latest)", quickstart)

    def test_every_hotkey_code_span_preserves_its_source_literal(self):
        """Markdown escaping must not change the shortcut a user should press."""
        bundle, content = self.bundle()
        actual = re.findall(
            r"^\| `([^`]*)` \|", bundle.files[Path("kb/hotkeys.md")], re.MULTILINE
        )
        expected = [
            row["combo"] for group in content.hotkeys for row in group["rows"]
        ]
        self.assertEqual(actual, expected)

    def test_human_index_local_links_resolve_inside_the_site(self):
        """Every relative link in the human landing page must reach a generated file."""
        bundle, _ = self.bundle()
        page = bundle.files[Path("knowledge/index.html")]
        generated = {path.as_posix() for path in bundle.files}
        generated.add("index.html")
        missing = []
        for href in re.findall(r'href="([^"]+)"', page):
            if href.startswith(("http://", "https://", "#")):
                continue
            target = href.split("#", 1)[0]
            candidate = posixpath.normpath(posixpath.join("knowledge", target))
            if target.endswith("/"):
                candidate = posixpath.normpath(posixpath.join(candidate, "index.html"))
            if candidate not in generated:
                missing.append(href)
        self.assertEqual(missing, [])

    def test_every_generated_text_file_is_lf_only(self):
        """Cross-platform generation must preserve the repository's LF contract."""
        bundle, _ = self.bundle()
        for path, body in bundle.files.items():
            self.assertNotIn("\r", body, path)
            self.assertTrue(body.endswith("\n"), path)
            self.assertFalse(body.endswith("\n\n"), path)


class CommandLine(unittest.TestCase):
    """Checks mutually exclusive modes and fail-closed site directories."""

    def test_conflicting_site_modes_fail_before_creating_a_target(self):
        """Ambiguous write modes must exit with argparse status 2 and write nothing."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "site"
            cases = (["--out", str(Path(tmp) / "one.html"), "--site-out", str(root)], ["--check", "--site-out", str(root)])
            for args in cases:
                with self.subTest(args=args), redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as caught:
                        tour_main(args)
                    self.assertEqual(caught.exception.code, 2)
                    self.assertFalse(root.exists())

    def test_site_output_refuses_an_unknown_existing_file(self):
        """A stale orphan must never ride inside the uploaded Pages artifact."""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "site"
            root.mkdir()
            sentinel = root / "private.txt"
            sentinel.write_text("do not publish", encoding="utf-8")
            with redirect_stderr(io.StringIO()):
                result = tour_main(["--site-out", str(root)])
            self.assertEqual(result, 2)
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "do not publish")


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
                source = locales.get(text.locale_key, code)
                if "%{" in source:
                    self.assertNotIn("%{", text.get(code), f"page key {key!r} left a placeholder")
                    checked += 1
                    continue
                self.assertEqual(
                    text.get(code),
                    source,
                    f"page key {key!r} drifted from {text.locale_key!r} [{code}]",
                )
                checked += 1
        for zone in content.zones:
            for field in ("title", "body"):
                text = zone[field]
                if not text.locale_key:
                    continue
                for code in content.codes:
                    source = locales.get(text.locale_key, code)
                    if "%{" in source:
                        self.assertNotIn("%{", text.get(code))
                        checked += 1
                        continue
                    self.assertEqual(
                        text.get(code), source
                    )
                    checked += 1
        self.assertGreater(checked, 0, "no locale-backed slots — the wiring is dead")


class LanguageCompleteness(unittest.TestCase):
    def test_every_slot_has_every_configured_language(self):
        _, content, _ = build()
        for key, text in content.page.items():
            for code in content.codes:
                self.assertTrue(text.get(code).strip(), f"page.{key} has no {code}")
        for mode in content.modes:
            for field in ("label", "tip", "lead"):
                for code in content.codes:
                    self.assertTrue(
                        mode[field].get(code).strip(),
                        f"mode.{mode['id']}.{field} has no {code}",
                    )

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
            for block in doc.get("maps") or []:
                if block.get("mode") != "classic":
                    continue
                for zone in block.get("zones") or []:
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
        for mode in content.modes:
            numbers = sorted(z["n"] for z in content.zones if z["mode"] == mode["id"])
            self.assertEqual(numbers, list(range(1, len(numbers) + 1)), mode["id"])

    def test_zone_ids_are_unique(self):
        _, content, _ = build()
        for mode in content.modes:
            ids = [z["id"] for z in content.zones if z["mode"] == mode["id"]]
            self.assertEqual(len(ids), len(set(ids)), mode["id"])

    def test_every_clickable_region_has_a_zone(self):
        page, content, _ = build()
        markup = re.sub(r"<script>.*?</script>", "", page, flags=re.DOTALL)
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

    def test_active_or_malformed_quickstart_html_is_rejected(self):
        """A content edit must not turn the public Pages origin into an XSS surface."""
        attacks = [
            "<script>alert(1)</script>",
            "<svg onload=alert(1)></svg>",
            "<img src=x onerror=alert(1)>",
            "<iframe src=https://example.com></iframe>",
            '<a href="javascript:alert(1)" target="_blank" rel="noopener">x</a>',
            '<a href="data:text/html,x" target="_blank" rel="noopener">x</a>',
            '<a href="https://example.com" style="color:red" target="_blank" rel="noopener">x</a>',
            '<a href="https://safe.example/) [evil](javascript:alert(1))" target="_blank" rel="noopener">x</a>',
            '<a href="https://safe.example/) &lt;img src=x onerror=alert(1)&gt;" target="_blank" rel="noopener">x</a>',
            '<a href="https://user:secret@example.com/path" target="_blank" rel="noopener">x</a>',
            "<b><code>out of order</b></code>",
        ]
        locales = load_locales(paths.LOCALES_DIR)
        for payload in attacks:
            with self.subTest(payload=payload):
                def inject(doc, value=payload):
                    """Put one adversarial fragment into an allowed markup slot."""
                    doc["steps"][0]["body"]["ru"] = value

                with broken_content("quickstart.yml", inject) as content_dir:
                    with self.assertRaises(ContentError):
                        content = load_content(content_dir, locales)
                        content.problems.raise_if_any("content is not usable", ContentError)

    def test_markdown_source_text_cannot_create_active_markup(self):
        """Plain and allowed-HTML text may not inject Markdown or raw HTML."""
        payload = (
            '<img src=x onerror=alert(1)> [js](javascript:alert(1)) '
            '[data](data:text/html,x) `code` # heading | cell\n- item'
        )
        for value, has_markup in ((payload, False), (f"<b>{payload}</b>", True)):
            with self.subTest(has_markup=has_markup):
                rendered = knowledge._markdown_text(value, has_markup)
                self.assertNotIn("<img", rendered)
                self.assertNotIn("](javascript:", rendered)
                self.assertNotIn("](data:", rendered)
                self.assertNotIn("`code`", rendered)
                self.assertNotIn("\n", rendered)
                self.assertIn(r"\[js\]\(javascript:alert\(1\)\)", rendered)

    def test_markdown_link_destination_encodes_delimiters(self):
        """A second safety layer keeps even pre-encoded URLs inside one link."""
        href = "https://safe.example/%29%20%5Bevil%5D%28javascript:alert%281%29%29"
        rendered = knowledge._markdown_text(
            f'<a href="{href}" target="_blank" rel="noopener">safe</a>', True
        )
        self.assertEqual(rendered, f"[safe]({href})")
        unsafe = "https://safe.example/) [evil](javascript:alert(1))"
        destination = knowledge._markdown_destination(unsafe)
        self.assertNotIn(" ", destination)
        self.assertNotIn("[", destination)
        self.assertNotIn("(", destination)
        self.assertNotIn(")", destination)


def _app_markup(page: str, mode_id: str) -> str:
    """Return one generated window replica by matching its opening/closing divs."""
    token = f'id="app-{mode_id}"'
    at = page.find(token)
    if at < 0:
        return ""
    start = page.rfind("<div", 0, at)
    depth = 0
    i = start
    while i < len(page):
        if page.startswith("<div", i):
            depth += 1
            i += 4
            continue
        if page.startswith("</div>", i):
            depth -= 1
            i += 6
            if depth == 0:
                return page[start:i]
            continue
        i += 1
    return page[start:]


class WindowModes(unittest.TestCase):
    """Classic and AutoTrading must both be generated from the content model."""

    def test_classic_and_auto_are_first_class_modes(self):
        _, content, _ = build()
        ids = [mode["id"] for mode in content.modes]
        self.assertEqual(ids, ["classic", "auto"])
        self.assertFalse(content.modes[0]["default"])
        self.assertTrue(content.modes[1]["default"])
        self.assertEqual(content.modes[0]["label"].get("ru"), "MANUAL режим")
        self.assertEqual(content.modes[1]["label"].get("ru"), "AUTO режим")
        classic = [z for z in content.zones if z["mode"] == "classic"]
        auto = [z for z in content.zones if z["mode"] == "auto"]
        self.assertEqual(len(classic), 19)
        self.assertGreaterEqual(len(auto), 8)
        self.assertTrue(any(z["id"] == "overview" for z in auto))
        self.assertTrue(any(z["id"] == "cores" for z in auto))
        self.assertFalse(any(z["id"] == "detects" for z in auto))
        self.assertFalse(any(z["id"] == "alerts" for z in auto))

    def test_window_replicas_are_not_hardcoded_in_the_template(self):
        template = paths.TEMPLATE.read_text(encoding="utf-8")
        markup = re.sub(r"<script>.*?</script>", "", template, flags=re.DOTALL)
        self.assertNotIn('data-zone="', markup)
        self.assertNotIn('id="app-classic"', markup)
        self.assertNotIn('id="app-auto"', markup)
        self.assertIn("{{window_maps}}", template)
        self.assertIn("{{mode_switch}}", template)
        self.assertIn("{{data_modes}}", template)

    def test_each_mode_map_exposes_exactly_its_zones(self):
        page, content, _ = build()
        markup = re.sub(r"<script>.*?</script>", "", page, flags=re.DOTALL)
        for mode in content.modes:
            html = _app_markup(markup, mode["id"])
            self.assertTrue(html, mode["id"])
            found = set(re.findall(r'data-zone="([^"]+)"', html))
            expected = {z["id"] for z in content.zones if z["mode"] == mode["id"]}
            self.assertEqual(found, expected, mode["id"])
        classic_html = _app_markup(markup, "classic")
        auto_html = _app_markup(markup, "auto")
        self.assertIn('data-i18n="tab.news"', classic_html)
        self.assertIn('data-i18n="tab.alerts"', classic_html)
        self.assertNotIn('data-i18n="tab.news"', auto_html)
        self.assertNotIn('data-i18n="tab.alerts"', auto_html)
        self.assertIn('data-i18n="tab.charts"', auto_html)
        self.assertIn("auto-rail", auto_html)

    def test_dropping_auto_mode_is_reported(self):
        def drop_auto(doc):
            doc["modes"] = [item for item in doc["modes"] if item["id"] != "auto"]

        locales = load_locales(paths.LOCALES_DIR)
        with broken_content("modes.yml", drop_auto) as content_dir:
            with self.assertRaises(TourError) as caught:
                content = load_content(content_dir, locales)
                content.problems.raise_if_any("content is not usable", ContentError)
        self.assertIn("auto", str(caught.exception))

    def test_mode_switch_is_keyboard_operable(self):
        page, content, _ = build()
        self.assertIn('role="radiogroup"', page)
        self.assertIn('name="tour-mode"', page)
        self.assertIn("prefers-reduced-motion", page)
        self.assertIn("@keyframes map-in", page)
        for mode in content.modes:
            self.assertIn(f'id="mode-{mode["id"]}"', page)
            self.assertIn('type="radio"', page)
            self.assertIn(mode["label"].get("ru"), page)
            self.assertIn(mode["label"].get("en"), page)

    def test_auto_is_the_no_hash_and_invalid_hash_default(self):
        page, _, _ = build()
        classic_input = re.search(r'<input[^>]+id="mode-classic"[^>]*>', page).group(0)
        auto_input = re.search(r'<input[^>]+id="mode-auto"[^>]*>', page).group(0)
        self.assertNotIn(" checked", classic_input)
        self.assertIn(" checked", auto_input)
        self.assertIn('const DEFAULT_MODE = "auto";', page)
        self.assertIn('if(h === "map-classic" || h === "classic") return "classic";', page)
        self.assertIn('if(h === "map-auto" || h === "auto") return "auto";', page)
        self.assertIsNotNone(
            re.search(
                r'function modeFromHash\(\)\{.*?return DEFAULT_MODE;\s*\}',
                page,
                flags=re.DOTALL,
            )
        )

    def test_mode_change_uses_the_generated_radio_and_updates_inactive_maps(self):
        page, _, _ = build()
        self.assertIn(
            "const checked = $('.tour-mode-input[name=\"tour-mode\"]:checked');",
            page,
        )
        self.assertNotIn('.input[name="tour-mode"]:checked', page)
        self.assertRegex(
            page,
            r'const on = app\.dataset\.mode === mode;\s*'
            r'if\("inert" in app\) app\.inert = !on;\s*'
            r'app\.setAttribute\("aria-hidden", String\(!on\)\);',
        )
        self.assertIn(
            'input.addEventListener("change", ()=> applyMode(currentMode(), true));',
            page,
        )

    def test_no_script_default_hides_classic_and_keeps_auto_first(self):
        page, _, _ = build()
        self.assertIn('.app-map[data-mode="classic"]{display:none}', page)
        self.assertIsNone(
            re.search(r'^\.app-map\[data-mode="auto"\]\{display:none\}$', page, re.MULTILINE)
        )
        self.assertIn(
            'body:has(#mode-classic:checked) .app-map[data-mode="auto"]{display:none}',
            page,
        )
        self.assertLess(page.index('id="mode-auto"'), page.index('id="app-auto"'))

    def test_narrow_auto_map_keeps_a_compact_left_rail(self):
        template = paths.TEMPLATE.read_text(encoding="utf-8")
        narrow = re.search(
            r'@media\(max-width:760px\)\{\s*'
            r'(?P<body>\.auto-rail\{width:calc\(92px \* var\(--k\)\).*?)\n\}',
            template,
            re.DOTALL,
        )
        self.assertIsNotNone(narrow)
        css = narrow.group("body")
        self.assertNotIn("flex-direction:column", css)
        self.assertNotIn("width:100%", css)
        self.assertNotIn("border-right:0", css)

    def test_noscript_lists_every_mode_annotation(self):
        page, content, _ = build()
        noscript = page[page.rfind("<noscript>") :]
        for zone in content.zones:
            self.assertIn(zone["title"].get("ru"), noscript, zone["id"])

    def test_generated_js_keeps_modes_in_the_content_model(self):
        page, content, _ = build()
        self.assertIn("const DEFAULT_MODE", page)
        self.assertIn("const MODES", page)
        self.assertNotIn("const ZONES =", page)
        for mode in content.modes:
            self.assertIn(f'"{mode["id"]}"', page[page.index("const MODES") :])

    def test_renderer_rejects_an_unknown_layout_region(self):
        _, content, _ = build()
        content.layouts["auto"] = [{"id": "not-a-region", "zones": ["brand"]}]
        problems = Problems()
        map_mod.window_maps(content, problems)
        self.assertTrue(any("not-a-region" in item.what for item in problems.items))


if __name__ == "__main__":
    unittest.main()
