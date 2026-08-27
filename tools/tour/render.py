"""Fills ``template.html``: CSS custom properties from the theme, data from content.

The template holds every byte of markup and behavioural JavaScript. This module
supplies only what is derived — which is why a layout change is a template edit
and a palette or wording change is a re-run.

The emitted JavaScript shapes are a CONTRACT with the template's own code and
cannot be changed here alone: ``T`` is a flat key to language map, ``MODES`` is
keyed by mode id with per-mode ``zones`` maps of ``[title, body]`` pairs, and so
on.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

from . import emit, map as map_mod
from .content import Content
from .errors import OutputError, Problems, TemplateError
from .theme import Theme, Themes

SLOT = re.compile(r"\{\{(\w+)\}\}")


@dataclass(frozen=True)
class Rendered:
    page: str
    problems: Problems


# --------------------------------------------------------------------------
# CSS
# --------------------------------------------------------------------------


def _palette_block(theme: Theme, css: dict, problems: Problems, where: str) -> list[str]:
    """One ``--token: #rrggbb`` line per ALLOWED palette entry.

    The allowlist is the point: MoonUI ships 43 palette tokens and this page uses
    31. Emitting the rest would add CSS variables nothing reads.
    """
    lines: list[str] = []
    for var in css.get("palette_vars", []):
        token = var.replace("-", "_")
        if token not in theme.palette:
            problems.add(where, f"palette token {token!r} (--{var}) is not in the theme")
            continue
        lines.append(f"  --{var}:{theme.palette[token]};")

    accent = theme.palette.get("accent", "#000000")
    alpha = theme.metrics.get("accent_tint_a", 0.1)
    ring = (css.get("selection_ring_alpha") or {}).get(where, 0.5)
    lines.append(f"  --accent-tint:{_rgba(accent, alpha)};")
    lines.append(f"  --sel-ring:{_rgba(accent, ring)};")
    return lines


def _rgba(hex_colour: str, alpha: float) -> str:
    r = int(hex_colour[1:3], 16)
    g = int(hex_colour[3:5], 16)
    b = int(hex_colour[5:7], 16)
    return f"rgba({r},{g},{b},{alpha:g})"


def _metric_block(theme: Theme, css: dict, problems: Problems) -> list[str]:
    lines: list[str] = []
    for var, token in (css.get("metric_vars") or {}).items():
        if token not in theme.metrics:
            problems.add("css.yml", f"metric {token!r} (--{var}) is not in the theme")
            continue
        value = theme.metrics[token]
        if var.startswith("h-"):
            # Heights scale with the web-only --k; radii do not.
            lines.append(f"  --{var}:calc({value:g}px * var(--k));")
        else:
            lines.append(f"  --{var}:{value:g}px;")
    return lines


def _typography_block(theme: Theme, css: dict) -> list[str]:
    fonts = css.get("font_stacks") or {}
    ui = theme.typography.get("font_family", "Inter")
    mono = theme.typography.get("mono_font_family", "monospace")
    ui_tail = fonts.get("ui", "system-ui,sans-serif")
    mono_tail = fonts.get("mono", "monospace")
    return [f"  --ui:'{ui}',{ui_tail};", f"  --mono:'{mono}',{mono_tail};"]


def css_tokens(themes: Themes, css: dict, problems: Problems) -> str:
    """The whole token layer: light default, both dark overrides, scale steps.

    Light sits on the bare ``:root`` and dark is applied twice — once behind
    ``prefers-color-scheme`` guarded against an explicit light choice, once
    behind ``[data-theme="dark"]``. A viewer who never chose a theme matches
    neither stamp, so the bare block must be complete on its own.
    """
    scale = css.get("scale") or {}
    base = scale.get("base", 1.0)

    light = _palette_block(themes.light, css, problems, "light")
    dark = _palette_block(themes.dark, css, problems, "dark")
    metrics = _metric_block(themes.light, css, problems)
    fonts = _typography_block(themes.light, css)

    out: list[str] = []
    out.append("/* GENERATED from MoonUI's moon-terminal theme — see tools/tour/. */")
    out.append(":root{")
    out.extend(light)
    out.append(f"  --k:{base:g};")
    out.extend(metrics)
    out.extend(fonts)
    out.append("}")

    out.append('@media(prefers-color-scheme:dark){')
    out.append('  :root:not([data-theme="light"]){')
    out.extend("  " + line for line in dark)
    out.append("  }")
    out.append("}")

    out.append(':root[data-theme="dark"]{')
    out.extend(dark)
    out.append("}")

    for step in scale.get("steps", []):
        out.append(
            f"@media(max-width:{step['max_width']}px){{:root{{--k:{step['k']:g}}}}}"
        )

    return "\n".join(out)


# --------------------------------------------------------------------------
# data
# --------------------------------------------------------------------------


def _langs(content: Content) -> list[str]:
    return content.codes


def data_page(content: Content) -> str:
    table = {key: dict(text.values) for key, text in content.page.items()}
    return f"const T = {emit.js_literal(table)};"


def data_modes(content: Content) -> str:
    """Per-mode labels, leads and zone tables consumed by the template's JS."""
    table: dict[str, object] = {}
    default = next((m["id"] for m in content.modes if m.get("default")), "classic")
    for mode in content.modes:
        zones: dict[str, object] = {}
        order = []
        for zone in (z for z in content.zones if z["mode"] == mode["id"]):
            entry: dict[str, object] = {"n": zone["n"]}
            for code in _langs(content):
                entry[code] = [zone["title"].get(code), zone["body"].get(code)]
            entry["src"] = _src_line(zone)
            zones[zone["id"]] = entry
            order.append(zone["id"])
        table[mode["id"]] = {
            "label": dict(mode["label"].values),
            "tip": dict(mode["tip"].values),
            "lead": dict(mode["lead"].values),
            "order": order,
            "zones": zones,
        }
    return (
        f"const DEFAULT_MODE = {emit.js_literal(default)};\n"
        f"const MODES = {emit.js_literal(table)};"
    )


def _src_line(zone: dict) -> str:
    """Where the zone's text came from, rendered for the panel's footnote."""
    src = zone.get("src") or {}
    if "locale_ref" in src:
        return f"locales/{src['file']} — {src['locale_ref']}"
    if "code" in src:
        return str(src["code"])
    return ""


def data_steps(content: Content) -> str:
    rows = [
        {code: [s["title"].get(code), s["body"].get(code)] for code in _langs(content)}
        for s in content.steps
    ]
    return f"const STEPS = {emit.js_literal(rows)};"


def data_panels(content: Content) -> str:
    rows = []
    for panel in content.panels:
        label = panel.get("label") or {}
        rows.append(
            {
                "k": panel["panel_name"],
                "t": label.get("ref") or _label_key(panel),
                **{code: panel["description"].get(code) for code in _langs(content)},
            }
        )
    return f"const PANELS = {emit.js_literal(rows)};"


def _label_key(panel: dict) -> str:
    """Panels name their tab label by the page-dictionary key that carries it."""
    label = panel.get("label") or {}
    if "ref" in label:
        return str(label["ref"])
    return f"tab.{str(panel['panel_name']).lower()}"


def data_windows(content: Content) -> str:
    rows = [
        {code: [w["title"].get(code), w["body"].get(code)] for code in _langs(content)}
        for w in content.windows
    ]
    return f"const WINDOWS = {emit.js_literal(rows)};"


def data_hotkeys(content: Content) -> str:
    rows = [
        {
            "cat": dict(group["caption"].values),
            "rows": [[row["combo"], dict(row["action"].values)] for row in group["rows"]],
        }
        for group in content.hotkeys
    ]
    return f"const HOTKEYS = {emit.js_literal(rows)};"


def data_stats(content: Content) -> str:
    """The header figures, COUNTED rather than typed.

    Hand-written counts are the first thing to go stale — adding a panel used to
    mean remembering to bump a number in a second place.
    """
    figures = [
        (str(len(content.panels)), "stats.panels"),
        (str(len(content.windows)), "stats.windows"),
        (str(sum(len(g["rows"]) for g in content.hotkeys)), "stats.hotkeys"),
        # The terminal's languages, not the page's: the caption says "interface
        # languages", and the page shipping fewer does not make the app speak fewer.
        (str(content.locale_languages), "stats.languages"),
    ]
    rows = [
        [value, dict(content.page[key].values)]
        for value, key in figures
        if key in content.page
    ]
    return f"const STATS = {emit.js_literal(rows)};"


def lang_buttons(content: Content) -> str:
    first = content.languages[0].code
    return "\n    ".join(
        f'<button type="button" data-lang="{lang.code}" '
        f'aria-pressed="{"true" if lang.code == first else "false"}">'
        f"{emit.text(lang.label)}</button>"
        for lang in content.languages
    )


# --------------------------------------------------------------------------
# assembly
# --------------------------------------------------------------------------


def render(template: str, content: Content, themes: Themes) -> Rendered:
    problems = Problems()

    producers = {
        "css_tokens": lambda: css_tokens(themes, content.css, problems),
        "lang_buttons": lambda: lang_buttons(content),
        "map_leads": lambda: map_mod.map_leads(content),
        "mode_switch": lambda: map_mod.mode_switch(content),
        "window_maps": lambda: map_mod.window_maps(content, problems),
        "map_annotations": lambda: map_mod.map_annotations(content),
        "data_page": lambda: data_page(content),
        "data_modes": lambda: data_modes(content),
        "data_steps": lambda: data_steps(content),
        "data_panels": lambda: data_panels(content),
        "data_windows": lambda: data_windows(content),
        "data_hotkeys": lambda: data_hotkeys(content),
        "data_stats": lambda: data_stats(content),
    }

    wanted = set(SLOT.findall(template))
    missing = sorted(wanted - set(producers))
    unused = sorted(set(producers) - wanted)
    if missing:
        problems.add("template.html", f"slot(s) with no producer: {missing}")
    if unused:
        problems.add("render.py", f"producer(s) with no slot in the template: {unused}")
    problems.raise_if_any("template and renderer disagree", TemplateError)

    page = SLOT.sub(lambda m: producers[m.group(1)](), template)
    _post_checks(page, content, problems)
    return Rendered(page=emit.normalise(page), problems=problems)


def _post_checks(page: str, content: Content, problems: Problems) -> None:
    """What can only be judged once the whole page exists."""
    for leftover in SLOT.findall(page):
        problems.add("output", f"slot {{{{{leftover}}}}} survived rendering")

    for hit in re.findall(r"%\{\w+\}", page):
        problems.add("output", f"unresolved interpolation {hit} reached the page")

    for context in emit.assert_no_script_break(page):
        problems.add("output", f"a literal </script survived escaping, near: {context!r}")

    # Attribute scans read MARKUP only. The behavioural JS builds a selector
    # string that contains `data-zone="`, and counting that as an attribute
    # would fail the render on a false positive.
    markup = re.sub(r"<script>.*?</script>", "", page, flags=re.S)

    # Every key the template asks for by name must exist in the page dictionary.
    for key in sorted(set(re.findall(r'data-i18n="([^"]+)"', markup))):
        if key not in content.page:
            problems.add(
                "template.html",
                f'data-i18n="{key}" has no entry in content/page.yml',
                "the heading would render empty with no other symptom",
            )

    # Every generated replica must exist; per-mode data-zone integrity is
    # checked while map.py builds each replica, because nested divs make a
    # whole-page regex split on </div> unsound.
    for mode in content.modes:
        if f'data-mode="{mode["id"]}"' not in markup:
            problems.add("output", f"mode {mode['id']!r} has no generated window map")
    all_ids = {z["id"] for z in content.zones}
    for zid in sorted(set(re.findall(r'data-zone="([^"]+)"', markup))):
        if zid not in all_ids:
            problems.add(
                "template.html",
                f'data-zone="{zid}" has no entry in content/zones.yml',
                "clicking it would fall back to the placeholder instead of failing",
            )

    if "\r" in page:
        problems.add("output", "contains a CR — the page must be LF only")

    problems.raise_if_any("the rendered page failed its checks", OutputError)
