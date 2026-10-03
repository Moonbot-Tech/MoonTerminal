"""Loads ``locales/<lang>/<area>.<lang>.yml`` into one flat table of key -> language -> string.

The files are rust-i18n ``_version: 1`` documents: one language per folder, flat
dotted keys, string values. The filename's last dot-segment is the locale, so
the stem must end in that folder's code. The terminal itself reads them through
the ``i18n!`` proc macro at build time, so this loader is a second reader of the
same source of truth, never a copy of it.
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
    """Key -> ``en/<area>.en.yml``. English is the reference file the citations name."""

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
    """Read every ``*/*.yml`` under ``locales_dir``.

    The folder name is the language. A file whose stem does not end in that
    code is reported and skipped: rust-i18n would register the wrong locale.
    The same key in two areas of one language is refused. rust-i18n would
    silently keep one of them, which makes the winner depend on file order —
    and the tour would then quote a string the application does not show.
    ``origin`` records ``en/<area>.en.yml`` for every key. English is the reference
    language, so the citation does not follow whichever folder was read first.
    """
    if not locales_dir.is_dir():
        raise LocaleError(f"no locales directory at {locales_dir}")

    files = sorted(locales_dir.glob("*/*.yml"))
    if not files:
        raise LocaleError(f"no */*.yml files under {locales_dir}")

    strings: dict[str, dict[str, str]] = {}
    origin: dict[str, str] = {}
    problems = Problems()

    for path in files:
        folder = path.parent.name
        stem = path.stem
        where = f"{folder}/{path.name}"
        suffix = stem.rsplit(".", 1)
        if len(suffix) != 2 or suffix[1] != folder:
            problems.add(
                where,
                f"file stem must end in .{folder}",
                "rust-i18n reads the locale from the last dot-segment of the filename",
            )
            continue
        area = suffix[0]

        try:
            doc = yaml.safe_load(path.read_text(encoding="utf-8"))
        except yaml.YAMLError as exc:
            problems.add(where, f"is not valid YAML: {exc}")
            continue

        if not isinstance(doc, dict):
            problems.add(where, "top level is not a mapping")
            continue

        for key, value in doc.items():
            if key == VERSION_KEY:
                continue
            if not isinstance(key, str):
                problems.add(where, f"key {key!r} is not a string")
                continue
            if not isinstance(value, str):
                problems.add(f"{where}: {key}", "value is not a string")
                continue

            have = strings.setdefault(key, {})
            if folder in have:
                problems.add(
                    f"{where}: {key}",
                    f"already defined in {origin[key]} for {folder}",
                    "rust-i18n would keep only one of the two — rename or remove one",
                )
                continue

            have[folder] = value
            origin.setdefault(key, f"en/{area}.en.yml")

    problems.raise_if_any(f"cannot load {locales_dir}", LocaleError)
    return Locales(strings=strings, origin=origin)
