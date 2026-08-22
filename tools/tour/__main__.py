"""Command line for the tour generator.

    python -m tools.tour                 regenerate docs/tour/index.html
    python -m tools.tour --check         verify the committed file is up to date
    python -m tools.tour --out FILE      render only the interactive page elsewhere
    python -m tools.tour --site-out DIR  render the complete publishable site
    python -m tools.tour --upstream      read MoonUI's live theme instead of the snapshot

Exit codes: ``0`` fine, ``1`` the committed page is stale (``--check`` only),
``2`` the inputs are wrong.
"""

from __future__ import annotations

import argparse
import difflib
import sys
from pathlib import Path

MIN_PYTHON = (3, 11)


def _bootstrap() -> None:
    """Fail with a fixable message rather than a traceback.

    stdout is forced to UTF-8 first: a Windows console defaults to a legacy
    codepage, and without this the tool can die encoding its own error message
    — measured here, printing a Spanish string raised UnicodeEncodeError on
    cp1251 before any of the real work had a chance to run.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is not None:
            reconfigure(encoding="utf-8", errors="replace")

    if sys.version_info < MIN_PYTHON:
        have = ".".join(str(p) for p in sys.version_info[:3])
        want = ".".join(str(p) for p in MIN_PYTHON)
        sys.exit(f"tour: needs Python {want}+ for tomllib, running {have}")

    try:
        import yaml  # noqa: F401
    except ImportError:
        sys.exit(
            "tour: PyYAML is not installed\n"
            "  pip install -r tools/requirements.txt"
        )


_bootstrap()

from . import knowledge, paths, render as render_mod  # noqa: E402
from .content import load as load_content  # noqa: E402
from .errors import TourError  # noqa: E402
from .emit import write as write_page  # noqa: E402
from .locales import load as load_locales  # noqa: E402
from .theme import load as load_theme, resolve as resolve_theme  # noqa: E402


def build(theme_path: Path | None, upstream: bool) -> tuple[str, knowledge.KnowledgeBundle, str]:
    """Render the interactive page, knowledge bundle and theme description."""
    source = resolve_theme(theme_path, upstream=upstream)
    themes = load_theme(source)
    locales = load_locales(paths.LOCALES_DIR)
    content = load_content(paths.CONTENT_DIR, locales)
    content.problems.raise_if_any("content is not usable")

    template = paths.TEMPLATE.read_text(encoding="utf-8")
    result = render_mod.render(template, content, themes)
    knowledge_template = paths.KNOWLEDGE_TEMPLATE.read_text(encoding="utf-8")
    bundle = knowledge.build(content, themes, knowledge_template)
    return result.page, bundle, source.describe()


def _write_site(root: Path, page: str, bundle: knowledge.KnowledgeBundle) -> None:
    """Write the site only when an existing root contains no orphan files."""
    expected = {Path("index.html"), *bundle.files}
    if root.exists() and not root.is_dir():
        raise TourError(f"site output exists and is not a directory: {root}")
    if root.is_dir():
        existing = {
            path.relative_to(root)
            for path in root.rglob("*")
            if path.is_file() or path.is_symlink()
        }
        unexpected = sorted(existing - expected)
        if unexpected:
            names = ", ".join(path.as_posix() for path in unexpected)
            raise TourError(f"site output contains unexpected files: {names}")
    write_page(root / "index.html", page)
    knowledge.write(root, bundle)


def _diff(committed: str, fresh: str, path: Path) -> str:
    return "".join(
        difflib.unified_diff(
            committed.splitlines(keepends=True),
            fresh.splitlines(keepends=True),
            fromfile=f"{path} (committed)",
            tofile=f"{path} (regenerated)",
            n=2,
        )
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="tour", description="Generate docs/tour/index.html from locales and content."
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="do not write; fail if the committed page differs from a fresh render",
    )
    parser.add_argument("--out", type=Path, help="write somewhere other than docs/tour/")
    parser.add_argument(
        "--site-out",
        type=Path,
        help="write a complete deployable site to this directory",
    )
    parser.add_argument("--theme", type=Path, help="read this moon-terminal.toml")
    parser.add_argument(
        "--upstream",
        action="store_true",
        help="read MoonUI's own theme instead of the committed snapshot",
    )
    args = parser.parse_args(argv)

    if args.out and args.site_out:
        parser.error("--out and --site-out cannot be used together")
    if args.check and args.site_out:
        parser.error("--check applies to one committed page; it cannot be combined with --site-out")

    try:
        page, bundle, theme_desc = build(args.theme, args.upstream)
    except TourError as exc:
        print(f"tour: {exc}", file=sys.stderr)
        return 2

    if args.site_out:
        try:
            _write_site(args.site_out, page, bundle)
        except TourError as exc:
            print(f"tour: {exc}", file=sys.stderr)
            return 2
        print(f"tour: wrote complete site to {args.site_out}  ({len(bundle.files) + 1} files)")
        print(f"      theme: {theme_desc}")
        return 0

    target = args.out or paths.OUTPUT

    if args.check:
        if not target.is_file():
            print(f"tour: {target} does not exist — run `make tour`", file=sys.stderr)
            return 1
        committed = target.read_text(encoding="utf-8")
        if committed == page:
            print(f"tour: {target.name} is up to date  [theme: {theme_desc}]")
            return 0
        print(_diff(committed, page, target), file=sys.stderr)
        print(
            f"tour: {target} is out of date.\n"
            "  run: make tour   and commit the regenerated page",
            file=sys.stderr,
        )
        return 1

    write_page(target, page)
    print(f"tour: wrote {target}  ({len(page.splitlines())} lines)")
    print(f"      theme: {theme_desc}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
