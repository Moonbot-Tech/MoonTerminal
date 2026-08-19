"""Turning values into page bytes: escaping, determinism, and writing.

Everything that can silently corrupt the page lives here, in one place, so the
rules are auditable rather than scattered through the renderer.

Three hazards this module exists to close:

* **The page has two trust contexts.** Most content reaches the DOM through
  ``innerHTML``, a few slots through ``textContent``. A slot is plain text unless
  it is explicitly marked as carrying markup, and plain text is escaped.
* **``</script`` ends the script element.** The HTML parser looks for that byte
  sequence and does not care that it sits inside a JavaScript string literal, so
  no amount of JS quoting saves a value containing it.
* **Text-mode writing on Windows turns ``\\n`` into ``\\r\\n``.** The repository
  is checked out LF; a translated write makes every line of the generated file
  read as drift against a regeneration done anywhere else.
"""

from __future__ import annotations

import html
import json
from pathlib import Path

#: Escaped so the HTML parser cannot see a closing script tag. ``<\/`` is the
#: same string to JavaScript, which is why this is safe to do unconditionally.
_SCRIPT_CLOSE = "</"
_SCRIPT_CLOSE_SAFE = "<\\/"

#: Valid in JSON, fatal in a JavaScript source file: the parser treats these as
#: line terminators and a string literal cannot span a line. Spelled as escapes
#: on purpose — as literal characters they are invisible in an editor, and the
#: next person to touch this dict would delete one without ever seeing it.
_JS_LINE_BREAKS = {
    chr(0x2028): "\\u2028",
    chr(0x2029): "\\u2029",
}


def text(value: str) -> str:
    """Escape a value destined for an HTML text node or attribute."""
    return html.escape(value, quote=True)


def js_string_safe(rendered: str) -> str:
    """Make an already-serialised JS literal safe to sit inside ``<script>``."""
    out = rendered.replace(_SCRIPT_CLOSE, _SCRIPT_CLOSE_SAFE)
    for raw, escaped in _JS_LINE_BREAKS.items():
        out = out.replace(raw, escaped)
    return out


def js_literal(value: object, indent: int = 0) -> str:
    """Serialise a Python value as a deterministic JavaScript literal.

    Key order is whatever the caller built, never sorted here — the content
    files declare an order and the page's reading order should match it.
    ``ensure_ascii=False`` keeps Cyrillic and Spanish readable in the output, so
    a reviewer can diff the generated page by eye.
    """
    rendered = json.dumps(
        value,
        ensure_ascii=False,
        indent=2,
        separators=(",", ": "),
        sort_keys=False,
    )
    if indent:
        pad = " " * indent
        rendered = "\n".join(
            pad + line if i else line for i, line in enumerate(rendered.splitlines())
        )
    return js_string_safe(rendered)


def assert_no_script_break(page: str) -> list[str]:
    """Report any literal ``</script`` outside the one that closes the block.

    Returns the offending contexts rather than raising, so the caller can report
    it beside every other post-render failure.
    """
    needle = "</script"
    found: list[str] = []
    start = 0
    while True:
        at = page.lower().find(needle, start)
        if at == -1:
            return found
        # The single legitimate occurrence is the tag closing the script block.
        tail = page[at : at + len("</script>")]
        if tail == "</script>" and page.count("</script>") == 1:
            start = at + 1
            continue
        found.append(page[max(0, at - 60) : at + 20])
        start = at + 1


def write(path: Path, page: str) -> None:
    """Write the page as LF-only UTF-8 with exactly one trailing newline.

    ``newline=""`` disables the translation that would otherwise turn every
    ``\\n`` into ``\\r\\n`` on Windows.
    """
    body = page.replace("\r\n", "\n").replace("\r", "\n").rstrip("\n") + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write(body)


def normalise(page: str) -> str:
    """The exact bytes :func:`write` would produce, for in-memory comparison."""
    return page.replace("\r\n", "\n").replace("\r", "\n").rstrip("\n") + "\n"
