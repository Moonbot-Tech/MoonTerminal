"""Loads MoonUI's ``moon-terminal.toml`` — the terminal's real palette.

The theme lives in a SEPARATE repository (``Moonbot-Tech/MoonUI``), consumed here
as a Cargo git dependency, so there is no file path to it in CI and none in a
fresh clone either. Hence the fallback chain in :func:`load` and the committed
snapshot beside this module.

What that buys, stated plainly: the palette used to be hand-copied into the page
with no provenance at all. A snapshot with a recorded upstream SHA is not
"current by construction", but it is a tracked file that shows up in diffs.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path

from . import paths
from .errors import ThemeError

#: Recorded by ``make tour-theme`` at the top of the snapshot.
PROVENANCE_PREFIX = "# moonui-rev:"


@dataclass(frozen=True)
class ThemeSource:
    """Where the loaded theme came from, so the CLI can say so."""

    path: Path
    is_snapshot: bool
    moonui_rev: str | None = None

    def describe(self) -> str:
        if not self.is_snapshot:
            return f"MoonUI checkout: {self.path}"
        rev = self.moonui_rev or "unknown revision"
        return f"committed snapshot ({rev})"


@dataclass(frozen=True)
class Theme:
    """One mode's tokens, already converted to what CSS wants."""

    palette: dict[str, str]
    """Token -> ``#rrggbb``."""

    metrics: dict[str, float]
    typography: dict[str, object]


@dataclass(frozen=True)
class Themes:
    dark: Theme
    light: Theme
    source: ThemeSource

    def mode(self, name: str) -> Theme:
        if name == "dark":
            return self.dark
        if name == "light":
            return self.light
        raise ThemeError(f"unknown theme mode {name!r}")


def _hex(value: object, where: str) -> str:
    """Convert a TOML ``0xRRGGBB`` integer to a CSS hex colour."""
    if isinstance(value, bool) or not isinstance(value, int):
        raise ThemeError(f"{where}: expected an integer colour, got {value!r}")
    if not 0 <= value <= 0xFFFFFF:
        raise ThemeError(f"{where}: colour {value:#x} is outside 0x000000..0xFFFFFF")
    return f"#{value:06x}"


def _mode(doc: dict, name: str) -> Theme:
    section = doc.get(name)
    if not isinstance(section, dict):
        raise ThemeError(f"theme has no [{name}] section")

    raw_palette = section.get("palette")
    if not isinstance(raw_palette, dict):
        raise ThemeError(f"theme has no [{name}.palette] table")

    palette: dict[str, str] = {}
    for token, value in raw_palette.items():
        # A few palette entries are alpha factors, not colours; keep them numeric
        # so the CSS layer can decide how to spell them.
        if isinstance(value, float):
            continue
        palette[token] = _hex(value, f"[{name}.palette] {token}")

    alphas = {
        token: float(value)
        for token, value in raw_palette.items()
        if isinstance(value, float)
    }

    metrics = {
        token: float(value)
        for token, value in (section.get("metrics") or {}).items()
        if isinstance(value, (int, float)) and not isinstance(value, bool)
    }
    metrics.update(alphas)

    typography = dict(section.get("typography") or {})

    return Theme(palette=palette, metrics=metrics, typography=typography)


def _read_provenance(path: Path) -> str | None:
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith(PROVENANCE_PREFIX):
            return line[len(PROVENANCE_PREFIX) :].strip()
        if not line.startswith("#"):
            break
    return None


def resolve(explicit: Path | None = None, upstream: bool = False) -> ThemeSource:
    """Pick a theme file. **The committed snapshot is the default, deliberately.**

    The obvious design — prefer a live MoonUI checkout and fall back to the
    snapshot — produces a failure nobody can diagnose. CI has no sibling
    checkout, so it always renders from the snapshot; a contributor who does
    have one would render from HIS checkout's revision, commit that, and hand CI
    a full palette diff unrelated to whatever the pull request changed.

    So generation is snapshot-only and therefore identical everywhere. Reading
    upstream is an explicit act (``--upstream``, which is what ``make tour-theme``
    uses to refresh the snapshot), never something that happens to a normal run
    because of what else is on the developer's disk.
    """
    if explicit is not None:
        if not explicit.is_file():
            raise ThemeError(f"--theme {explicit} does not exist")
        return ThemeSource(path=explicit, is_snapshot=False)

    if upstream:
        for candidate in paths.moonui_theme_candidates():
            if candidate.is_file():
                return ThemeSource(path=candidate, is_snapshot=False)
        tried = "\n    ".join(str(p) for p in paths.moonui_theme_candidates())
        raise ThemeError(
            "--upstream asked for MoonUI's own theme, but no checkout was found.\n"
            f"  tried:\n    {tried}\n"
            "  set MOONUI_DIR, or pass --theme <path to moon-terminal.toml>"
        )

    snapshot = paths.THEME_SNAPSHOT
    if not snapshot.is_file():
        raise ThemeError(
            f"no committed theme snapshot at {snapshot}\n"
            "  run: make tour-theme   (needs a MoonUI checkout)"
        )
    return ThemeSource(
        path=snapshot, is_snapshot=True, moonui_rev=_read_provenance(snapshot)
    )


def load(source: ThemeSource) -> Themes:
    try:
        doc = tomllib.loads(source.path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise ThemeError(f"{source.path}: not valid TOML: {exc}") from exc

    return Themes(dark=_mode(doc, "dark"), light=_mode(doc, "light"), source=source)
