"""Loads ``locales/*.yml`` into one flat table of key -> language -> string.

The files are rust-i18n ``_version: 2`` documents: a flat mapping of dotted key
to a language map. The terminal itself reads them through the ``i18n!`` proc
macro at build time, so this loader is a second reader of the same source of
truth, never a copy of it.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import yaml

from .errors import LocaleError, Problems

#: rust-i18n's own version marker; not a translatable key.
VERSION_KEY = "_version"


@dataclass(frozen=True)
class Locales:
    """Every translatable string in the repository, and which file each came from."""

    strings: dict[str, dict[str, str]]
    origin: dict[str, str]
    """Key -> the ``locales/`` filename it was defined in, for ``src`` lines."""

    def __contains__(self, key: str) -> bool:
        return key in self.strings

    @property
    def keys(self) -> list[str]:
        return sorted(self.strings)

    def get(self, key: str, lang: str) -> str:
        """Return one translation, raising rather than returning a placeholder.

        Callers that want to accumulate failures check membership first; this
        raise is the last line of defence, not the reporting path.
        """
        try:
            return self.strings[key][lang]
        except KeyError as exc:
            raise LocaleError(f"locale key {key!r} has no {lang!r} translation") from exc

    def languages_of(self, key: str) -> set[str]:
        return set(self.strings[key])

    def file_of(self, key: str) -> str:
        return self.origin[key]


def load(locales_dir: Path) -> Locales:
    """Read every ``*.yml`` under ``locales_dir``.

    Refuses a key defined in two files. rust-i18n would silently keep one of
    them, which makes the winner depend on file order — and the tour would then
    quote a string the application does not actually show.
    """
    if not locales_dir.is_dir():
        raise LocaleError(f"no locales directory at {locales_dir}")

    files = sorted(locales_dir.glob("*.yml"))
    if not files:
        raise LocaleError(f"no *.yml files under {locales_dir}")

    strings: dict[str, dict[str, str]] = {}
    origin: dict[str, str] = {}
    problems = Problems()

    for path in files:
        try:
            doc = yaml.safe_load(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as exc:
            problems.add(path.name, f"is not valid YAML: {exc}")
            continue

        if not isinstance(doc, dict):
            problems.add(path.name, "top level is not a mapping")
            continue

        for key, value in doc.items():
            if key == VERSION_KEY:
                continue

            if not isinstance(value, dict):
                problems.add(f"{path.name}: {key}", "value is not a language mapping")
                continue

            if key in origin:
                problems.add(
                    f"{path.name}: {key}",
                    f"already defined in {origin[key]}",
                    "rust-i18n would keep only one of the two — rename or remove one",
                )
                continue

            translations = {
                lang: text for lang, text in value.items() if isinstance(text, str)
            }
            if not translations:
                problems.add(f"{path.name}: {key}", "has no string translations")
                continue

            strings[key] = translations
            origin[key] = path.name

    problems.raise_if_any(f"cannot load {locales_dir}", LocaleError)
    return Locales(strings=strings, origin=origin)
