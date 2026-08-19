"""The authored content under ``tools/tour/content/``, and the rules it must obey.

A *text slot* is one of exactly two shapes, and anything else is an error:

``{ru: …, en: …}``
    Authored here. Every configured language must be present — a missing one is
    reported, never silently dropped, because a dropped language renders as an
    empty heading and nothing tells you.

``{locale: some.key}``
    Pulled from ``locales/*.yml``: the application's own string, in every
    language it ships. Never retyped, so it cannot drift from what the terminal
    actually shows.

Two shapes rather than a tagged union because a one-key ``locale`` mapping can
never be mistaken for a language triple, so the reader needs no convention.

**Markup is opt-in.** Most content reaches the DOM through ``innerHTML``, so a
plain ``&`` or ``<`` in authored text would corrupt the page. Slots are escaped
by default; the few that genuinely carry markup say ``html: true``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

import yaml

from .errors import ContentError, Problems
from .locales import Locales

#: ``%{name}`` — rust-i18n's interpolation marker.
INTERPOLATION = re.compile(r"%\{(\w+)\}")

#: Keys allowed beside the languages in an authored slot.
AUTHORED_EXTRAS = {"html"}

#: Keys allowed beside ``locale`` in a reference slot.
LOCALE_EXTRAS = {"args", "html"}


@dataclass(frozen=True)
class Language:
    code: str
    label: str
    html_lang: str


@dataclass(frozen=True)
class Text:
    """One slot, resolved to a string per language."""

    values: dict[str, str]
    html: bool = False
    locale_key: str | None = None
    """Set when the text came from ``locales/`` — used to render its ``src`` line."""

    def get(self, lang: str) -> str:
        return self.values[lang]


@dataclass
class Content:
    languages: list[Language]
    page: dict[str, Text]
    zones: list[dict]
    steps: list[dict]
    panels: list[dict]
    windows: list[dict]
    hotkeys: list[dict]
    css: dict
    locale_languages: int = 0
    """How many languages the TERMINAL ships, read from locales/ — not the
    page's own language list, which is a separate and usually smaller set."""
    problems: Problems = field(default_factory=Problems)

    @property
    def codes(self) -> list[str]:
        return [lang.code for lang in self.languages]


class _Resolver:
    """Turns raw YAML slots into :class:`Text`, accumulating every failure."""

    def __init__(self, locales: Locales, codes: list[str], problems: Problems) -> None:
        self.locales = locales
        self.codes = codes
        self.problems = problems

    def text(self, where: str, raw: object) -> Text:
        if not isinstance(raw, dict):
            self.problems.add(where, f"expected a text slot mapping, got {type(raw).__name__}")
            return Text(values={code: "" for code in self.codes})

        if "locale" in raw:
            return self._from_locale(where, raw)
        return self._authored(where, raw)

    # -- the two shapes ----------------------------------------------------

    def _from_locale(self, where: str, raw: dict) -> Text:
        extra = set(raw) - {"locale"} - LOCALE_EXTRAS
        if extra:
            self.problems.add(
                where,
                f"a locale slot cannot also carry {sorted(extra)}",
                "either reference a locale key or author the text — not both",
            )

        key = raw["locale"]
        if not isinstance(key, str):
            self.problems.add(where, f"locale key must be a string, got {key!r}")
            return Text(values={code: "" for code in self.codes})

        if key not in self.locales:
            self.problems.add_unknown_key(
                where, key, self.locales.keys, "1600 keys in locales/*.yml"
            )
            return Text(values={code: "" for code in self.codes})

        have = self.locales.languages_of(key)
        missing = [code for code in self.codes if code not in have]
        if missing:
            self.problems.add(
                where,
                f"locale key {key!r} has no {', '.join(missing)} translation",
                f"add it to locales/{self.locales.file_of(key)}",
            )
            return Text(values={code: "" for code in self.codes}, locale_key=key)

        args = raw.get("args") or {}
        values = {}
        for code in self.codes:
            value = self.locales.get(key, code)
            for name, replacement in args.items():
                value = value.replace("%{" + name + "}", str(replacement))
            values[code] = value

        return Text(values=values, html=bool(raw.get("html")), locale_key=key)

    def _authored(self, where: str, raw: dict) -> Text:
        extra = set(raw) - set(self.codes) - AUTHORED_EXTRAS
        if extra:
            self.problems.add(
                where,
                f"unknown key(s) {sorted(extra)} in an authored slot",
                f"languages are {', '.join(self.codes)}; use `html: true` for markup",
            )

        values = {}
        for code in self.codes:
            value = raw.get(code)
            if not isinstance(value, str) or not value.strip():
                self.problems.add(
                    where, f"missing the {code!r} text", "every configured language is required"
                )
                values[code] = ""
            else:
                values[code] = value

        return Text(values=values, html=bool(raw.get("html")))


def _load_yaml(path: Path, schema: str, problems: Problems) -> dict:
    if not path.is_file():
        problems.add(path.name, "is missing")
        return {}
    try:
        doc = yaml.safe_load(path.read_text(encoding="utf-8"))
    except yaml.YAMLError as exc:
        problems.add(path.name, f"is not valid YAML: {exc}")
        return {}
    if not isinstance(doc, dict):
        problems.add(path.name, "top level is not a mapping")
        return {}
    if doc.get("_schema") != schema:
        problems.add(
            path.name,
            f"_schema is {doc.get('_schema')!r}, expected {schema!r}",
            "the loader and the file disagree about the format",
        )
    return doc


def load(content_dir: Path, locales: Locales) -> Content:
    """Read every content file, resolve every slot, and report all failures at once."""
    problems = Problems()

    langs_doc = _load_yaml(content_dir / "languages.yml", "tour.languages.v1", problems)
    languages = [
        Language(code=item["code"], label=item["label"], html_lang=item["html_lang"])
        for item in langs_doc.get("languages", [])
        if isinstance(item, dict) and {"code", "label", "html_lang"} <= set(item)
    ]
    if not languages:
        raise ContentError(
            problems.report("cannot read languages.yml")
            if problems
            else "languages.yml declares no languages"
        )

    codes = [lang.code for lang in languages]
    r = _Resolver(locales, codes, problems)

    page_doc = _load_yaml(content_dir / "page.yml", "tour.page.v1", problems)
    page = {
        key: r.text(f"page.yml: {key}", raw)
        for key, raw in (page_doc.get("strings") or {}).items()
    }

    zones_doc = _load_yaml(content_dir / "zones.yml", "tour.zones.v1", problems)
    zones = []
    for entry in zones_doc.get("zones", []):
        zid = entry.get("id", "?")
        where = f"zones.yml: zone {zid!r}"
        body_text = r.text(f"{where} body", entry.get("body"))
        zones.append(
            {
                "id": zid,
                "n": entry.get("n"),
                "title": r.text(f"{where} title", entry.get("title")),
                "body": body_text,
                "src": _zone_src(entry, body_text, locales),
            }
        )

    steps_doc = _load_yaml(content_dir / "quickstart.yml", "tour.quickstart.v1", problems)
    steps = [
        {
            "title": r.text(f"quickstart.yml: step {i} title", s.get("title")),
            "body": r.text(f"quickstart.yml: step {i} body", s.get("body")),
        }
        for i, s in enumerate(steps_doc.get("steps", []), 1)
    ]

    panels_doc = _load_yaml(content_dir / "panels.yml", "tour.panels.v1", problems)
    panels = [
        {
            "panel_name": p.get("panel_name"),
            "label": p.get("label"),
            "description": r.text(
                f"panels.yml: {p.get('panel_name')!r}", p.get("description")
            ),
        }
        for p in panels_doc.get("panels", [])
    ]

    windows_doc = _load_yaml(content_dir / "windows.yml", "tour.windows.v1", problems)
    windows = [
        {
            "title": r.text(f"windows.yml: window {i} title", w.get("title")),
            "body": r.text(f"windows.yml: window {i} body", w.get("body")),
        }
        for i, w in enumerate(windows_doc.get("windows", []), 1)
    ]

    hk_doc = _load_yaml(content_dir / "hotkeys.yml", "tour.hotkeys.v1", problems)
    hotkeys = []
    for gi, group in enumerate(hk_doc.get("groups", []), 1):
        rows = [
            {
                "combo": row.get("combo"),
                "action": r.text(
                    f"hotkeys.yml: group {gi} row {row.get('combo')!r}", row.get("action")
                ),
            }
            for row in group.get("rows", [])
        ]
        hotkeys.append(
            {
                "caption": r.text(f"hotkeys.yml: group {gi} caption", group.get("caption")),
                "rows": rows,
            }
        )

    css = _load_yaml(content_dir / "css.yml", "tour.css.v1", problems)

    content = Content(
        languages=languages,
        page=page,
        zones=zones,
        steps=steps,
        panels=panels,
        windows=windows,
        hotkeys=hotkeys,
        css=css,
        locale_languages=max(
            (len(locales.languages_of(k)) for k in locales.strings), default=0
        ),
        problems=problems,
    )
    _check_structure(content, problems)
    return content


def _zone_src(entry: dict, body: Text, locales: Locales) -> dict:
    """Where a zone's text came from, for the footnote under the explanation.

    A locale-backed zone derives it, so the line stays correct when a key moves
    between locale files — the one thing a hand-typed path cannot survive.
    """
    if body.locale_key:
        return {"locale_ref": body.locale_key, "file": locales.file_of(body.locale_key)}
    return entry.get("src") or {}


def _check_structure(content: Content, problems: Problems) -> None:
    """Invariants the page's own rendering depends on."""
    ids = [z["id"] for z in content.zones]
    duplicates = {i for i in ids if ids.count(i) > 1}
    if duplicates:
        problems.add("zones.yml", f"duplicate zone id(s): {sorted(duplicates)}")

    numbers = sorted(z["n"] for z in content.zones if isinstance(z["n"], int))
    expected = list(range(1, len(content.zones) + 1))
    if numbers != expected:
        problems.add(
            "zones.yml",
            f"badge numbers are {numbers}, expected {expected}",
            'the panel renders "Zone N / total", so the set must be 1..N with no gaps',
        )

    for zone in content.zones:
        for field_name in ("title", "body"):
            text = zone[field_name]
            for code, value in text.values.items():
                if INTERPOLATION.search(value):
                    problems.add(
                        f"zones.yml: zone {zone['id']!r} {field_name} [{code}]",
                        "still contains a %{…} placeholder",
                        "supply it via `args:` — the tour has no runtime values",
                    )
