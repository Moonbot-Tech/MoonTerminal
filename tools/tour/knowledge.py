"""Render the tour's verified content as AI-friendly files and a human index.

The interactive tour and the knowledge bundle deliberately share one loaded
``Content`` object.  A wording or locale change therefore reaches the browser
page, the Markdown files, ``llms.txt`` and the JSONL index in the same build.
"""

from __future__ import annotations

import html
import json
import re
from dataclasses import dataclass
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import quote

from . import emit
from .content import Content, Text
from .errors import Problems, TemplateError
from .render import css_tokens
from .theme import Themes

PUBLIC_BASE = "https://moonbot-tech.github.io/MoonTerminal"


@dataclass(frozen=True)
class KnowledgeBundle:
    """Every generated path relative to the published site root."""

    files: dict[Path, str]


class _MarkdownParser(HTMLParser):
    """Convert the small, explicitly allowed quick-start HTML subset to Markdown."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.parts: list[str] = []
        self.links: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Emit Markdown delimiters for allowed opening tags."""
        if tag == "code":
            self.parts.append("`")
        elif tag in {"b", "strong"}:
            self.parts.append("**")
        elif tag == "a":
            href = dict(attrs).get("href") or ""
            self.links.append(href)
            self.parts.append("[")
        elif tag == "br":
            self.parts.append("\n")

    def handle_endtag(self, tag: str) -> None:
        """Emit Markdown delimiters for allowed closing tags."""
        if tag == "code":
            self.parts.append("`")
        elif tag in {"b", "strong"}:
            self.parts.append("**")
        elif tag == "a":
            href = self.links.pop() if self.links else ""
            self.parts.append(f"]({_markdown_destination(href)})")

    def handle_data(self, data: str) -> None:
        """Preserve visible text without allowing it to become Markdown syntax."""
        self.parts.append(_escape_markdown(data))

    def markdown(self) -> str:
        """Return normalised one-paragraph Markdown."""
        return re.sub(r"\s+", " ", "".join(self.parts)).strip()


def _markdown_text(value: str, has_markup: bool) -> str:
    """Return plain source text as safe, compact Markdown."""
    if not has_markup:
        return _escape_markdown(value)
    parser = _MarkdownParser()
    parser.feed(value)
    parser.close()
    return parser.markdown()


def _escape_markdown(value: str) -> str:
    """Escape source prose so only generator-authored Markdown stays active."""
    escaped = html.escape(value, quote=False).replace("\\", "\\\\")
    escaped = re.sub(r"([`*_{}\[\]()#+.!|>\-])", r"\\\1", escaped)
    return re.sub(r"\s+", " ", escaped).strip()


def _markdown_destination(href: str) -> str:
    """Percent-encode characters that could terminate a Markdown destination."""
    return quote(href, safe=":/?#@!$&'*+,;=%-._~")


def _code_span(value: str) -> str:
    """Wrap an exact literal in a code fence longer than any source backtick run."""
    literal = re.sub(r"\s+", " ", value).strip()
    longest = max((len(run) for run in re.findall(r"`+", literal)), default=0)
    fence = "`" * (longest + 1)
    if literal.startswith(("`", " ")) or literal.endswith(("`", " ")):
        literal = f" {literal} "
    return f"{fence}{literal}{fence}"


def _yaml_string(value: str) -> str:
    """Quote one scalar with JSON syntax, which is also valid YAML."""
    return json.dumps(value, ensure_ascii=False)


def _front(
    topic_id: str,
    heading: str,
    title_ru: str,
    title_en: str,
    description: str,
    languages: list[str],
    generated_from: list[str],
    source_types: list[str],
    coverage: tuple[str, int],
) -> list[str]:
    """Create deterministic YAML identity plus the shared coverage preamble."""
    canonical = f"{PUBLIC_BASE}/kb/{topic_id}.md"
    lines = [
        "---",
        "schema: moonterminal.tour.topic.v1",
        f"id: {_yaml_string(topic_id)}",
        f"document_kind: {'index' if topic_id == 'index' else 'topic'}",
        f"primary_language: {languages[0]}",
        f"languages: [{', '.join(languages)}]",
        "title:",
        f"  ru: {_yaml_string(title_ru)}",
        f"  en: {_yaml_string(title_en)}",
        f"description: {_yaml_string(description)}",
        f"canonical_url: {_yaml_string(canonical)}",
        "generated_from:",
    ]
    lines.extend(f"  - {_yaml_string(path)}" for path in generated_from)
    lines.append("source_types:")
    lines.extend(f"  - {source_type}" for source_type in source_types)
    lines.extend(
        [
            "coverage:",
            f"  kind: {coverage[0]}",
            f"  count: {coverage[1]}",
            "limitations:",
            "  - Covers only the entities listed in this generated bundle.",
            "  - Missing information means not documented here, not unsupported.",
            "  - The static files do not connect to a Moonbot core or verify exchange-specific availability.",
            "  - Code pointers are navigation hints unless explicitly marked as mechanically verified.",
            "---",
            "",
            f"# {heading}",
            "",
            f"> {description}",
            ">",
            "> Generated automatically from MoonTerminal's curated tour data and application locale strings.",
            "> Coverage is intentionally limited to the topics listed here; absence means **not documented here**, not **unsupported**.",
            "",
        ]
    )
    return lines


def _panel_title(content: Content, panel: dict, code: str) -> str:
    """Resolve a stable panel key to its user-facing localized label."""
    text = _panel_title_text(content, panel)
    return text.get(code) if text is not None else str(panel["panel_name"])


def _panel_title_text(content: Content, panel: dict) -> Text | None:
    """Return the resolved label object so its provenance remains available."""
    label = panel.get("label") or {}
    key = label.get("ref") or f"tab.{str(panel['panel_name']).lower()}"
    return content.page.get(key)


def index_markdown(content: Content) -> str:
    """Render the Markdown table of contents and honest coverage counts."""
    hotkeys = sum(len(group["rows"]) for group in content.hotkeys)
    lines = _front(
        "index",
        "MoonTerminal knowledge base",
        "База знаний MoonTerminal",
        "MoonTerminal knowledge base",
        "A machine-readable map of the currently documented MoonTerminal interface.",
        content.codes,
        ["tools/tour/content/*.yml", "locales/*.yml"],
        ["authored-tour", "locale-literal", "advisory-code-pointer"],
        ("topic", 5),
    )
    lines.extend(
        [
            "## Available topics",
            "",
            f"- [Quick start]({PUBLIC_BASE}/kb/getting-started.md) — {len(content.steps)} steps",
            f"- [Main interface]({PUBLIC_BASE}/kb/interface.md) — {len(content.zones)} documented zones"
            + _mode_zone_clause(content),
            f"- [Dock panels]({PUBLIC_BASE}/kb/panels.md) — {len(content.panels)} panels",
            f"- [Separate windows]({PUBLIC_BASE}/kb/windows.md) — {len(content.windows)} windows",
            f"- [Hotkeys]({PUBLIC_BASE}/kb/hotkeys.md) — {hotkeys} shortcuts",
            "",
            "## How an assistant should answer",
            "",
            "1. Prefer the exact navigation, label and shortcut written in these files.",
            "2. Mention the documented source when one is included.",
            "3. If the requested control is absent, say that it is not documented in this bundle and do not invent a path.",
            "4. Trading actions are operationally risky; explain them, but do not imply that they are safe to trigger without user confirmation.",
            "",
            f"Human tour: [{PUBLIC_BASE}/]({PUBLIC_BASE}/)",
            "",
        ]
    )
    return emit.normalise("\n".join(lines))


def getting_started_markdown(content: Content) -> str:
    """Render the bilingual quick-start guide."""
    lines = _front(
        "getting-started",
        "MoonTerminal quick start",
        "Быстрый старт MoonTerminal",
        "MoonTerminal quick start",
        "Five source-backed steps from download to a connected core.",
        content.codes,
        ["tools/tour/content/quickstart.yml"],
        ["authored-tour"],
        ("step", len(content.steps)),
    )
    for number, step in enumerate(content.steps, start=1):
        lines.extend([f"## step-{number}", ""])
        lines.extend(_language_sections(step["title"], step["body"], content.codes))
    return emit.normalise("\n".join(lines))


def _mode_zone_clause(content: Content) -> str:
    """Spell Classic vs AutoTrading zone counts when both maps are present."""
    counts = []
    for mode in content.modes:
        n = sum(1 for zone in content.zones if zone["mode"] == mode["id"])
        counts.append(f"{n} {mode['id']}")
    if len(counts) < 2:
        return ""
    return " (" + ", ".join(counts) + ")"


def interface_markdown(content: Content) -> str:
    """Render the documented main-window zones with source references."""
    lines = _front(
        "interface",
        "MoonTerminal main interface",
        "Главный интерфейс MoonTerminal",
        "MoonTerminal main interface",
        "The currently documented clickable zones of the Classic and AutoTrading window maps.",
        content.codes,
        ["tools/tour/content/zones.yml", "tools/tour/content/modes.yml", "locales/*.yml"],
        ["authored-tour", "locale-literal", "advisory-code-pointer"],
        ("zone", len(content.zones)),
    )
    for mode in content.modes:
        lines.extend([f"## mode-{mode['id']}", ""])
        for zone in (z for z in content.zones if z["mode"] == mode["id"]):
            source = _zone_source(zone)
            heading = f"zone-{zone['id']}" if mode["id"] == "classic" else f"zone-{mode['id']}-{zone['id']}"
            lines.extend([f"## {heading}", ""])
            lines.extend(_language_sections(zone["title"], zone["body"], content.codes))
            if zone["body"].locale_key:
                lines.extend(["", f"**Locale key:** `{zone['body'].locale_key}`"])
            elif source:
                lines.extend(["", f"**Advisory code pointer:** `{source}`"])
            lines.append("")
    return emit.normalise("\n".join(lines))


def _zone_source(zone: dict) -> str:
    """Return the authored source citation for a main-window zone."""
    source = zone.get("src") or {}
    if "locale_ref" in source:
        return f"locales/{source['file']} — {source['locale_ref']}"
    return str(source.get("code") or "")


def panels_markdown(content: Content) -> str:
    """Render the known dock panels and stable persistence keys."""
    lines = _front(
        "panels",
        "MoonTerminal dock panels",
        "Панели дока MoonTerminal",
        "MoonTerminal dock panels",
        "Panels currently represented in the generated tour data.",
        content.codes,
        ["tools/tour/content/panels.yml", "tools/tour/content/page.yml", "locales/*.yml"],
        ["authored-tour", "locale-literal"],
        ("panel", len(content.panels)),
    )
    for panel in content.panels:
        lines.extend([f"## panel-{panel['panel_name']}", ""])
        for code in content.codes:
            lines.extend(
                [
                    f"### {code}: {_markdown_text(_panel_title(content, panel, code), False)}",
                    "",
                    _markdown_text(panel["description"].get(code), panel["description"].html),
                    "",
                ]
            )
        lines.extend([f"**Stable key:** `{panel['panel_name']}`", ""])
    return emit.normalise("\n".join(lines))


def windows_markdown(content: Content) -> str:
    """Render the currently documented separate application windows."""
    lines = _front(
        "windows",
        "MoonTerminal separate windows",
        "Отдельные окна MoonTerminal",
        "MoonTerminal separate windows",
        "Windows launched from the main trading toolbar.",
        content.codes,
        ["tools/tour/content/windows.yml"],
        ["authored-tour"],
        ("window", len(content.windows)),
    )
    for number, window in enumerate(content.windows, start=1):
        lines.extend([f"## window-{number}", ""])
        lines.extend(_language_sections(window["title"], window["body"], content.codes))
    return emit.normalise("\n".join(lines))


def hotkeys_markdown(content: Content) -> str:
    """Render hotkeys as compact bilingual Markdown tables."""
    hotkey_count = sum(len(group["rows"]) for group in content.hotkeys)
    lines = _front(
        "hotkeys",
        "MoonTerminal hotkeys",
        "Горячие клавиши MoonTerminal",
        "MoonTerminal hotkeys",
        "Documented shortcuts grouped by purpose.",
        content.codes,
        ["tools/tour/content/hotkeys.yml"],
        ["authored-tour"],
        ("hotkey", hotkey_count),
    )
    for number, group in enumerate(content.hotkeys, start=1):
        lines.extend([f"## hotkey-group-{number}", ""])
        for code in content.codes:
            lines.append(f"**{code}:** {_markdown_text(group['caption'].get(code), group['caption'].html)}")
        lines.extend(
            [
                "",
                "| Shortcut | " + " | ".join(content.codes) + " |",
                "|---|" + "---|" * len(content.codes),
            ]
        )
        for row in group["rows"]:
            combo = _code_span(row["combo"])
            actions = " | ".join(_table_cell(row["action"].get(code)) for code in content.codes)
            lines.append(f"| {combo} | {actions} |")
        lines.append("")
    return emit.normalise("\n".join(lines))


def _language_sections(title: Text, body: Text, codes: list[str]) -> list[str]:
    """Render one stable, labeled Markdown subsection per configured language."""
    lines: list[str] = []
    for code in codes:
        lines.extend(
            [
                f"### {code}: {_markdown_text(title.get(code), title.html)}",
                "",
                _markdown_text(body.get(code), body.html),
                "",
            ]
        )
    return lines


def _table_cell(value: str) -> str:
    """Escape characters that would split a Markdown table cell."""
    return _markdown_text(value, False)


def llms_text() -> str:
    """Render the small discovery index described by the llms.txt convention."""
    lines = [
        "# MoonTerminal",
        "",
        "> Desktop trading terminal for the Moonbot core. This index points to deterministic documentation generated from the repository.",
        "",
        "## Start here",
        "",
        f"- [Knowledge-base index]({PUBLIC_BASE}/kb/index.md): scope, rules and topic list",
        f"- [Complete knowledge base]({PUBLIC_BASE}/llms-full.txt): all current topics in one text file",
        f"- [Human knowledge-base view]({PUBLIC_BASE}/knowledge/): visual overview and direct file links",
        "",
        "## Topics",
        "",
        f"- [Quick start]({PUBLIC_BASE}/kb/getting-started.md)",
        f"- [Main interface]({PUBLIC_BASE}/kb/interface.md)",
        f"- [Dock panels]({PUBLIC_BASE}/kb/panels.md)",
        f"- [Separate windows]({PUBLIC_BASE}/kb/windows.md)",
        f"- [Hotkeys]({PUBLIC_BASE}/kb/hotkeys.md)",
        "",
        "## Scope",
        "",
        "This bundle is generated from the curated tour and application locale strings. Missing information means not documented here, not unsupported.",
        "",
    ]
    return emit.normalise("\n".join(lines))


def llms_full(documents: dict[Path, str]) -> str:
    """Concatenate topic Markdown in a stable order for one-file ingestion."""
    order = [
        Path("kb/index.md"),
        Path("kb/getting-started.md"),
        Path("kb/interface.md"),
        Path("kb/panels.md"),
        Path("kb/windows.md"),
        Path("kb/hotkeys.md"),
    ]
    parts = ["# MoonTerminal complete knowledge base\n"]
    for path in order:
        parts.extend([f"\n---\n\nSource file: `{path.as_posix()}`\n\n", documents[path]])
    return emit.normalise("".join(parts))


def knowledge_jsonl(content: Content) -> str:
    """Render one searchable bilingual JSON object per documented fact."""
    rows: list[dict[str, object]] = []
    for index, step in enumerate(content.steps, start=1):
        rows.append(_entry("quickstart", str(index), step["title"], step["body"], "tools/tour/content/quickstart.yml"))
    for zone in content.zones:
        zone_id = str(zone["id"]) if zone["mode"] == "classic" else f"{zone['mode']}.{zone['id']}"
        rows.append(
            _entry(
                "interface",
                zone_id,
                zone["title"],
                zone["body"],
                "tools/tour/content/zones.yml",
                advisory_pointer=str((zone.get("src") or {}).get("code") or ""),
            )
        )
    for panel in content.panels:
        label = _panel_title_text(content, panel)
        rows.append(
            {
                "id": str(panel["panel_name"]),
                "kind": "panel",
                "title": {code: _panel_title(content, panel, code) for code in content.codes},
                "text": dict(panel["description"].values),
                "provenance": {
                    "title": _text_provenance(label, "tools/tour/content/page.yml"),
                    "text": _text_provenance(panel["description"], "tools/tour/content/panels.yml"),
                },
            }
        )
    for index, window in enumerate(content.windows, start=1):
        rows.append(_entry("window", str(index), window["title"], window["body"], "tools/tour/content/windows.yml"))
    for group_index, group in enumerate(content.hotkeys, start=1):
        for row_index, row in enumerate(group["rows"], start=1):
            rows.append(
                {
                    "id": f"{group_index}.{row_index}",
                    "kind": "hotkey",
                    "shortcut": row["combo"],
                    "title": dict(group["caption"].values),
                    "text": dict(row["action"].values),
                    "provenance": {
                        "title": _text_provenance(group["caption"], "tools/tour/content/hotkeys.yml"),
                        "text": _text_provenance(row["action"], "tools/tour/content/hotkeys.yml"),
                    },
                }
            )
    return emit.normalise("\n".join(json.dumps(row, ensure_ascii=False, sort_keys=True) for row in rows))


def _entry(
    kind: str,
    entry_id: str,
    title: Text,
    body: Text,
    source: str,
    advisory_pointer: str = "",
) -> dict[str, object]:
    """Build one JSONL entry from resolved text objects."""
    entry = {
        "id": entry_id,
        "kind": kind,
        "title": dict(title.values),
        "text": {
            code: _markdown_text(value, body.html)
            for code, value in body.values.items()
        },
        "provenance": {
            "title": _text_provenance(title, source),
            "text": _text_provenance(body, source),
        },
    }
    if advisory_pointer:
        entry["advisory_code_pointer"] = advisory_pointer
    return entry


def _text_provenance(text: Text | None, authored_source: str) -> dict[str, str]:
    """Distinguish literal locale reuse from prose authored for the tour."""
    if text is not None and text.locale_key:
        return {"source_type": "locale-literal", "locale_key": text.locale_key}
    return {"source_type": "authored-tour", "source_path": authored_source}


def knowledge_html(template: str, content: Content, themes: Themes) -> str:
    """Render the human landing page for the generated knowledge files."""
    problems = Problems()
    tokens = css_tokens(themes, content.css, problems)
    problems.raise_if_any("knowledge theme is not usable")
    hotkeys = sum(len(group["rows"]) for group in content.hotkeys)
    cards = [
        ("kb/getting-started.md", "start", "Быстрый старт", "Quick start", f"{len(content.steps)} шагов"),
        ("kb/interface.md", "map", "Главный интерфейс", "Main interface", f"{len(content.zones)} зон"),
        ("kb/panels.md", "panels", "Панели дока", "Dock panels", f"{len(content.panels)} панелей"),
        ("kb/windows.md", "windows", "Отдельные окна", "Separate windows", f"{len(content.windows)} окон"),
        ("kb/hotkeys.md", "keys", "Горячие клавиши", "Hotkeys", f"{hotkeys} сочетаний"),
    ]
    card_html = "\n".join(
        f'<article class="card"><span>{emit.text(count)}</span><h2>{emit.text(ru)}</h2>'
        f'<p>{emit.text(en)}</p><div class="actions"><a href="../#{emit.text(anchor)}">Открыть тур</a>'
        f'<a href="../{emit.text(path)}">Markdown</a></div></article>'
        for path, anchor, ru, en, count in cards
    )
    slots = {
        "css_tokens": tokens,
        "cards": card_html,
        "public_base": PUBLIC_BASE,
    }
    wanted = set(re.findall(r"\{\{(\w+)\}\}", template))
    if wanted != set(slots):
        problems.add("knowledge_template.html", f"slot mismatch: template={sorted(wanted)}, renderer={sorted(slots)}")
        problems.raise_if_any("knowledge template and renderer disagree", TemplateError)
    page = re.sub(r"\{\{(\w+)\}\}", lambda match: slots[match.group(1)], template)
    return emit.normalise(page)


def sitemap() -> str:
    """Render a minimal sitemap for the human entry points."""
    return emit.normalise(
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        f"  <url><loc>{PUBLIC_BASE}/</loc></url>\n"
        f"  <url><loc>{PUBLIC_BASE}/knowledge/</loc></url>\n"
        "</urlset>\n"
    )


def build(content: Content, themes: Themes, template: str) -> KnowledgeBundle:
    """Build the complete deterministic knowledge bundle."""
    documents = {
        Path("kb/index.md"): index_markdown(content),
        Path("kb/getting-started.md"): getting_started_markdown(content),
        Path("kb/interface.md"): interface_markdown(content),
        Path("kb/panels.md"): panels_markdown(content),
        Path("kb/windows.md"): windows_markdown(content),
        Path("kb/hotkeys.md"): hotkeys_markdown(content),
    }
    files = dict(documents)
    files.update(
        {
            Path("knowledge/index.html"): knowledge_html(template, content, themes),
            Path("knowledge.jsonl"): knowledge_jsonl(content),
            Path("llms.txt"): llms_text(),
            Path("llms-full.txt"): llms_full(documents),
            Path("robots.txt"): emit.normalise(f"User-agent: *\nAllow: /\nSitemap: {PUBLIC_BASE}/sitemap.xml\n"),
            Path("sitemap.xml"): sitemap(),
        }
    )
    return KnowledgeBundle(files=files)


def write(root: Path, bundle: KnowledgeBundle) -> None:
    """Write a complete bundle below one explicitly selected site root."""
    for relative, body in bundle.files.items():
        emit.write(root / relative, body)
