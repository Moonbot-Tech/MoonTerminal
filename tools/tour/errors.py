"""Failure reporting for the tour generator.

Every check accumulates into one :class:`Problems` collector and the whole set is
reported at once. Failing fast on the first missing translation would mean one
run per missing string during a language pass — the collector turns that into a
single worklist.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from difflib import get_close_matches


class TourError(Exception):
    """Base for every generator failure."""


class ContentError(TourError):
    """The authored content under ``tools/tour/content/`` is wrong."""


class LocaleError(TourError):
    """``locales/*.yml`` could not be loaded, or a referenced key is absent."""


class ThemeError(TourError):
    """No usable theme: neither a MoonUI checkout nor the committed snapshot."""


class TemplateError(TourError):
    """``template.html`` and the render producers disagree."""


class OutputError(TourError):
    """The rendered page failed a post-render check."""


@dataclass(frozen=True)
class Problem:
    """One failure, addressed the way the author will fix it."""

    where: str
    """Where the author must look — ``content/zones.yml: zone 'live'``."""

    what: str
    """What is wrong, in one sentence."""

    hint: str | None = None
    """How to fix it, when that is not obvious from ``what``."""

    def render(self) -> str:
        line = f"  {self.where}: {self.what}"
        if self.hint:
            line += f"\n      -> {self.hint}"
        return line


@dataclass
class Problems:
    """Accumulates failures so one run reports the whole worklist."""

    items: list[Problem] = field(default_factory=list)

    def add(self, where: str, what: str, hint: str | None = None) -> None:
        self.items.append(Problem(where=where, what=what, hint=hint))

    def add_unknown_key(
        self, where: str, key: str, known: list[str], universe: str
    ) -> None:
        """Record an unknown identifier, suggesting the nearest known one.

        A typo in a locale key is the most common authoring mistake here, and
        without a suggestion the author is left grepping 1600 keys by hand.
        """
        near = get_close_matches(key, known, n=3, cutoff=0.7)
        hint = f"nearest: {', '.join(near)}" if near else f"no near match among {universe}"
        self.add(where, f"unknown key {key!r}", hint)

    def __bool__(self) -> bool:
        return bool(self.items)

    def __len__(self) -> int:
        return len(self.items)

    def report(self, headline: str) -> str:
        body = "\n".join(p.render() for p in self.items)
        count = len(self.items)
        noun = "problem" if count == 1 else "problems"
        return f"{headline} ({count} {noun}):\n{body}"

    def raise_if_any(self, headline: str, kind: type[TourError] = TourError) -> None:
        if self.items:
            raise kind(self.report(headline))
