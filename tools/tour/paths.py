"""Every path the tour generator reads or writes, resolved from this file's location.

Anchoring on ``__file__`` rather than the working directory is deliberate: the
generator is run from the repository root by ``make tour``, from ``tools/`` by
hand, and from a temporary directory by the tests, and all three must agree on
which ``locales/`` they mean.
"""

from __future__ import annotations

import os
from pathlib import Path

# tools/tour/paths.py -> tools/tour -> tools -> <repo>
PKG_DIR: Path = Path(__file__).resolve().parent
TOOLS_DIR: Path = PKG_DIR.parent
REPO_ROOT: Path = TOOLS_DIR.parent

# --- inputs -----------------------------------------------------------------

LOCALES_DIR: Path = REPO_ROOT / "locales"
CONTENT_DIR: Path = PKG_DIR / "content"
TEMPLATE: Path = PKG_DIR / "template.html"
KNOWLEDGE_TEMPLATE: Path = PKG_DIR / "knowledge_template.html"

#: Committed copy of MoonUI's theme, used when no sibling checkout is available.
#: Refreshed only by ``make tour-theme`` — never as a side effect of ``make tour``.
THEME_SNAPSHOT: Path = PKG_DIR / "theme.snapshot.toml"

#: Path of the theme inside a MoonUI checkout, relative to that checkout's root.
THEME_IN_MOONUI = Path("crates") / "moon-ui-components" / "themes" / "moon-terminal.toml"

#: Default sibling MoonUI checkout, the layout a local dev already has for the
#: `.cargo/config.toml` patch override.
SIBLING_MOONUI: Path = REPO_ROOT.parent / "MoonUI"

# --- output -----------------------------------------------------------------

OUTPUT: Path = REPO_ROOT / "docs" / "tour" / "index.html"


def moonui_theme_candidates() -> list[Path]:
    """Theme locations to try, most explicit first.

    ``MOONUI_DIR`` lets a checkout that does not sit beside this one still be
    used without passing ``--theme`` on every invocation.
    """
    candidates: list[Path] = []

    env = os.environ.get("MOONUI_DIR")
    if env:
        candidates.append(Path(env).expanduser() / THEME_IN_MOONUI)

    candidates.append(SIBLING_MOONUI / THEME_IN_MOONUI)
    return candidates
