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
from collections import defaultdict
from dataclasses import dataclass, field
from html import escape as html_escape
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urlsplit

import yaml

from .errors import ContentError, Problems
from .locales import Locales

#: ``%{name}`` — rust-i18n's interpolation marker.
INTERPOLATION = re.compile(r"%\{(\w+)\}")

#: Keys allowed beside the languages in an authored slot.
AUTHORED_EXTRAS = {"html"}

#: Keys allowed beside ``locale`` in a reference slot.
LOCALE_EXTRAS = {"args", "html"}

ALLOWED_MARKUP = {"a", "b", "code"}


class _MarkupValidator(HTMLParser):
    """Reject browser-active HTML outside the tour's tiny formatting subset."""

    def __init__(self, where: str, problems: Problems) -> None:
        """Remember the content slot that owns any reported markup defect."""
        super().__init__(convert_charrefs=True)
        self.where = where
        self.problems = problems
        self.stack: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Validate an opening tag, its attributes and any link destination."""
        if tag not in ALLOWED_MARKUP:
            self.problems.add(self.where, f"HTML tag <{tag}> is not allowed")
            return
        self.stack.append(tag)
        values = dict(attrs)
        if len(values) != len(attrs):
            self.problems.add(self.where, f"HTML tag <{tag}> repeats an attribute")
        if tag != "a":
            if attrs:
                self.problems.add(self.where, f"HTML tag <{tag}> cannot carry attributes")
            return
        extra = set(values) - {"href", "target", "rel"}
        if extra:
            self.problems.add(self.where, f"HTML link has unsupported attributes {sorted(extra)}")
        href = values.get("href") or ""
        if not _is_safe_https_href(href):
            self.problems.add(
                self.where,
                "HTML links must use an absolute https:// URL without credentials, whitespace or markup delimiters",
            )
        if values.get("target") != "_blank":
            self.problems.add(self.where, 'HTML links must use target="_blank"')
        rel = set((values.get("rel") or "").split())
        if "noopener" not in rel:
            self.problems.add(self.where, 'HTML links must include rel="noopener"')

    def handle_endtag(self, tag: str) -> None:
        """Reject a closing tag that is outside the same formatting subset."""
        if tag not in ALLOWED_MARKUP:
            self.problems.add(self.where, f"HTML closing tag </{tag}> is not allowed")
            return
        if not self.stack or self.stack[-1] != tag:
            self.problems.add(self.where, f"HTML closing tag </{tag}> is out of order")
            return
        self.stack.pop()

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        """Reject self-closing markup because the current subset needs none."""
        self.problems.add(self.where, f"self-closing HTML tag <{tag}/> is not allowed")

    def handle_comment(self, data: str) -> None:
        """Reject comments so they cannot hide unreviewed browser markup."""
        self.problems.add(self.where, "HTML comments are not allowed")

    def close(self) -> None:
        """Report any formatting tag left open at the end of the fragment."""
        super().close()
        if self.stack:
            self.problems.add(self.where, f"unclosed HTML tag(s): {self.stack}")


def _is_safe_https_href(href: str) -> bool:
    """Accept a complete web URL that is safe in both HTML and Markdown output."""
    if any(character.isspace() or ord(character) < 0x20 for character in href):
        return False
    if any(character in href for character in "()<>[]`\\"):
        return False
    try:
        parsed = urlsplit(href)
        return (
            parsed.scheme == "https"
            and bool(parsed.hostname)
            and parsed.username is None
            and parsed.password is None
        )
    except ValueError:
        return False


def _validate_markup(where: str, values: dict[str, str], problems: Problems) -> None:
    """Validate every translated fragment before it can reach ``innerHTML``."""
    for code, value in values.items():
        parser = _MarkupValidator(f"{where}.{code}", problems)
        parser.feed(value)
        parser.close()


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
    modes: list[dict]
    layouts: dict[str, list]
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

        markup = self._markup_flag(where, raw)
        if markup:
            _validate_markup(where, values, self.problems)
        return Text(values=values, html=markup, locale_key=key)

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

        markup = self._markup_flag(where, raw)
        if markup:
            _validate_markup(where, values, self.problems)
        return Text(values=values, html=markup)

    def _markup_flag(self, where: str, raw: dict) -> bool:
        """Require the optional markup flag to be a real boolean."""
        value = raw.get("html", False)
        if not isinstance(value, bool):
            self.problems.add(where, f"html must be true or false, got {value!r}")
            return False
        return value


#: Quick-start block kinds. A text kind carries one slot, a list kind a list of
#: slots; ``code`` is a language-neutral literal and ``terms`` a list of
#: ``{term, text}`` pairs.
STEP_TEXT_BLOCKS = {"group", "sub", "p", "note", "warn"}
STEP_LIST_BLOCKS = {"path", "status"}
STEP_BLOCKS = STEP_TEXT_BLOCKS | STEP_LIST_BLOCKS | {"code", "terms"}


def _load_step(where: str, raw: object, r: _Resolver, problems: Problems, codes: list[str]) -> dict:
    """Resolve one quick-start step: title, short body and its optional blocks."""
    if not isinstance(raw, dict):
        problems.add(where, "a step must be a mapping")
        empty = Text(values={code: "" for code in codes})
        return {"title": empty, "body": empty, "blocks": []}
    extra = set(raw) - {"title", "body", "blocks"}
    if extra:
        problems.add(where, f"unknown step key(s) {sorted(extra)}")
    title = r.text(f"{where} title", raw.get("title"))
    body = r.text(f"{where} body", raw.get("body"))
    blocks: list[dict] = []
    raw_blocks = raw.get("blocks") or []
    if not isinstance(raw_blocks, list):
        problems.add(where, "blocks must be a list")
        raw_blocks = []
    for bi, block in enumerate(raw_blocks, 1):
        at = f"{where} block {bi}"
        if not isinstance(block, dict) or len(block) != 1:
            problems.add(at, f"a block is a one-key mapping of one of {sorted(STEP_BLOCKS)}")
            continue
        ((kind, value),) = block.items()
        if kind not in STEP_BLOCKS:
            problems.add(at, f"unknown block kind {kind!r}", f"use one of {sorted(STEP_BLOCKS)}")
        elif kind == "code":
            if not isinstance(value, str) or not value.strip():
                problems.add(at, "code must be a non-empty string")
            else:
                blocks.append({"kind": kind, "code": value.strip()})
        elif kind in STEP_TEXT_BLOCKS:
            blocks.append({"kind": kind, "text": r.text(f"{at} {kind}", value)})
        elif not isinstance(value, list) or not value:
            problems.add(at, f"{kind} must be a non-empty list")
        elif kind == "terms":
            pairs = []
            for ti, item in enumerate(value, 1):
                if not isinstance(item, dict) or set(item) != {"term", "text"}:
                    problems.add(f"{at} item {ti}", "a terms item has exactly `term` and `text`")
                    continue
                pairs.append(
                    {
                        "term": r.text(f"{at} item {ti} term", item["term"]),
                        "text": r.text(f"{at} item {ti} text", item["text"]),
                    }
                )
            blocks.append({"kind": kind, "items": pairs})
        else:
            items = [r.text(f"{at} item {ti}", item) for ti, item in enumerate(value, 1)]
            blocks.append({"kind": kind, "items": items})
    return {"title": title, "body": body, "blocks": blocks}


def step_texts(step: dict) -> list[Text]:
    """Every slot one quick-start step carries, blocks included."""
    texts = [step["title"], step["body"]]
    for block in step["blocks"]:
        if "text" in block:
            texts.append(block["text"])
        for item in block.get("items", []):
            texts.extend((item["term"], item["text"]) if isinstance(item, dict) else (item,))
    return texts


def as_html(text: Text, code: str) -> str:
    """One language of a slot as safe HTML: escaped unless it declared markup."""
    value = text.get(code)
    return value if text.html else html_escape(value, quote=False)


def step_digest(step: dict, codes: list[str]) -> Text:
    """The whole step flattened to one HTML paragraph per language — the
    knowledge bundle's one-fact-per-step view of the structured page."""
    body, blocks = step["body"], step["blocks"]
    values = {}
    for code in codes:
        parts = [as_html(body, code)]
        for block in blocks:
            kind = block["kind"]
            if kind == "code":
                parts.append(f"<code>{html_escape(block['code'], quote=False)}</code>")
            elif kind == "group":
                parts.append(f"<b>{as_html(block['text'], code)}:</b>")
            elif "text" in block:
                parts.append(as_html(block["text"], code))
            elif kind == "terms":
                parts.append(
                    "; ".join(
                        f"<b>{as_html(i['term'], code)}</b> — {as_html(i['text'], code)}"
                        for i in block["items"]
                    )
                    + "."
                )
            else:
                parts.append(" → ".join(as_html(i, code) for i in block["items"]) + ".")
        values[code] = " ".join(part for part in parts if part)
    return Text(values=values, html=True)


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

    modes_doc = _load_yaml(content_dir / "modes.yml", "tour.modes.v1", problems)
    modes = _load_modes(modes_doc, r, problems)

    layouts_doc = _load_yaml(content_dir / "layouts.yml", "tour.layouts.v1", problems)
    layouts = _load_layouts(layouts_doc, problems)

    zones_doc = _load_yaml(content_dir / "zones.yml", "tour.zones.v2", problems)
    zones = _load_zones(zones_doc, r, locales, problems)

    steps_doc = _load_yaml(content_dir / "quickstart.yml", "tour.quickstart.v2", problems)
    steps = [
        _load_step(f"quickstart.yml: step {i}", s, r, problems, codes)
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
        modes=modes,
        layouts=layouts,
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


def _load_modes(doc: dict, r: _Resolver, problems: Problems) -> list[dict]:
    """Read the first-class window-map modes and their switcher copy."""
    modes = []
    for index, raw in enumerate(doc.get("modes") or [], 1):
        if not isinstance(raw, dict):
            problems.add("modes.yml", f"mode {index} is not a mapping")
            continue
        mid = raw.get("id")
        if not isinstance(mid, str) or not mid.strip():
            problems.add("modes.yml", f"mode {index} is missing an id")
            mid = f"mode-{index}"
        where = f"modes.yml: mode {mid!r}"
        default = raw.get("default", False)
        if not isinstance(default, bool):
            problems.add(where, f"default must be true or false, got {default!r}")
            default = False
        modes.append(
            {
                "id": mid,
                "default": default,
                "label": r.text(f"{where} label", raw.get("label")),
                "tip": r.text(f"{where} tip", raw.get("tip")),
                "lead": r.text(f"{where} lead", raw.get("lead")),
            }
        )
    return modes


def _load_layouts(doc: dict, problems: Problems) -> dict[str, list]:
    """Read the per-mode region lists that the map renderer consumes."""
    layouts: dict[str, list] = {}
    for index, raw in enumerate(doc.get("maps") or [], 1):
        if not isinstance(raw, dict):
            problems.add("layouts.yml", f"map {index} is not a mapping")
            continue
        mode_id = raw.get("mode")
        if not isinstance(mode_id, str) or not mode_id.strip():
            problems.add("layouts.yml", f"map {index} is missing mode")
            continue
        if mode_id in layouts:
            problems.add("layouts.yml", f"duplicate layout for mode {mode_id!r}")
        regions = []
        for ri, region in enumerate(raw.get("regions") or [], 1):
            if not isinstance(region, dict):
                problems.add("layouts.yml", f"mode {mode_id} region {ri} is not a mapping")
                continue
            rid = region.get("id")
            zone_ids = region.get("zones")
            if not isinstance(rid, str) or not rid.strip():
                problems.add("layouts.yml", f"mode {mode_id} region {ri} is missing an id")
                continue
            if not isinstance(zone_ids, list) or not all(isinstance(z, str) for z in zone_ids):
                problems.add("layouts.yml", f"mode {mode_id} region {rid!r} zones must be a list of ids")
                zone_ids = []
            regions.append({"id": rid, "zones": list(zone_ids)})
        layouts[mode_id] = regions
    return layouts


def _load_zones(doc: dict, r: _Resolver, locales: Locales, problems: Problems) -> list[dict]:
    """Read per-mode zone lists into one flattened table tagged with ``mode``."""
    zones: list[dict] = []
    blocks = doc.get("maps")
    if not isinstance(blocks, list):
        problems.add("zones.yml", "maps must be a list of mode blocks")
        return zones
    for index, block in enumerate(blocks, 1):
        if not isinstance(block, dict):
            problems.add("zones.yml", f"map {index} is not a mapping")
            continue
        mode_id = block.get("mode")
        if not isinstance(mode_id, str) or not mode_id.strip():
            problems.add("zones.yml", f"map {index} is missing mode")
            continue
        for entry in block.get("zones") or []:
            if not isinstance(entry, dict):
                problems.add("zones.yml", f"mode {mode_id} has a non-mapping zone")
                continue
            zid = entry.get("id", "?")
            where = f"zones.yml: mode {mode_id} zone {zid!r}"
            body_text = r.text(f"{where} body", entry.get("body"))
            zones.append(
                {
                    "id": zid,
                    "mode": mode_id,
                    "n": entry.get("n"),
                    "title": r.text(f"{where} title", entry.get("title")),
                    "body": body_text,
                    "src": _zone_src(entry, body_text, locales),
                }
            )
    return zones


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
    mode_ids = [m["id"] for m in content.modes]
    if len(mode_ids) != len(set(mode_ids)):
        problems.add("modes.yml", f"duplicate mode id(s): {sorted({i for i in mode_ids if mode_ids.count(i) > 1})}")
    if "classic" not in mode_ids or "auto" not in mode_ids:
        problems.add(
            "modes.yml",
            f"mode ids are {mode_ids}, expected classic and auto as first-class peers",
            "the live Pages tour must ship both the Classic and AutoTrading window maps",
        )
    defaults = [m["id"] for m in content.modes if m.get("default")]
    if len(defaults) != 1:
        problems.add("modes.yml", f"exactly one mode must be default, found {defaults}")

    by_mode: dict[str, list] = defaultdict(list)
    for zone in content.zones:
        by_mode[zone["mode"]].append(zone)

    for mode in content.modes:
        mid = mode["id"]
        mz = by_mode.get(mid, [])
        ids = [z["id"] for z in mz]
        duplicates = {i for i in ids if ids.count(i) > 1}
        if duplicates:
            problems.add("zones.yml", f"mode {mid}: duplicate zone id(s): {sorted(duplicates)}")
        numbers = sorted(z["n"] for z in mz if isinstance(z["n"], int))
        expected = list(range(1, len(mz) + 1))
        if numbers != expected:
            problems.add(
                "zones.yml",
                f"mode {mid}: badge numbers are {numbers}, expected {expected}",
                'the panel renders "Zone N / total", so the set must be 1..N with no gaps',
            )
        regions = content.layouts.get(mid)
        if not regions:
            problems.add("layouts.yml", f"mode {mid!r} has no layout")
        else:
            laid: list[str] = []
            for region in regions:
                laid.extend(region["zones"])
            if len(laid) != len(set(laid)):
                problems.add("layouts.yml", f"mode {mid}: a zone id is listed in more than one region")
            if sorted(laid) != sorted(ids):
                problems.add(
                    "layouts.yml",
                    f"mode {mid}: layout zones {sorted(laid)} do not match zones.yml {sorted(ids)}",
                    "every annotated zone must appear in exactly one rendered region",
                )

    extra_modes = sorted(set(by_mode) - set(mode_ids))
    if extra_modes:
        problems.add("zones.yml", f"zones declared for unknown mode(s) {extra_modes}")
    extra_layouts = sorted(set(content.layouts) - set(mode_ids))
    if extra_layouts:
        problems.add("layouts.yml", f"layouts declared for unknown mode(s) {extra_layouts}")

    for zone in content.zones:
        for field_name in ("title", "body"):
            text = zone[field_name]
            for code, value in text.values.items():
                if INTERPOLATION.search(value):
                    problems.add(
                        f"zones.yml: mode {zone['mode']} zone {zone['id']!r} {field_name} [{code}]",
                        "still contains a %{…} placeholder",
                        "supply it via `args:` — the tour has no runtime values",
                    )
