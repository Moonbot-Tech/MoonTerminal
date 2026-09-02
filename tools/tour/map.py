"""Generate the MANUAL and AUTO window replicas from the content model.

The template holds page chrome, CSS and behavioural JavaScript. This module is
the only place that turns ``modes.yml`` / ``layouts.yml`` / ``zones.yml`` into
the clickable schematic, so a second mode cannot exist as a hand-copied HTML
island.
"""

from __future__ import annotations

import re
from collections.abc import Callable

from . import emit
from .content import Content
from .errors import Problems

MOON_PATH = "M20.5 14.6A8.6 8.6 0 1 1 9.9 3.9a6.9 6.9 0 0 0 10.6 10.7Z"

BOOK_SVG = """<svg viewBox="0 0 60 200" preserveAspectRatio="none" aria-hidden="true">
                  <g fill="var(--red)" opacity=".85">
                    <rect x="26" y="6"  width="34" height="3.4"/><rect x="14" y="12" width="46" height="3.4"/>
                    <rect x="32" y="18" width="28" height="3.4"/><rect x="8"  y="24" width="52" height="3.4"/>
                    <rect x="38" y="30" width="22" height="3.4"/><rect x="20" y="36" width="40" height="3.4"/>
                    <rect x="30" y="42" width="30" height="3.4"/><rect x="42" y="48" width="18" height="3.4"/>
                    <rect x="24" y="54" width="36" height="3.4"/><rect x="36" y="60" width="24" height="3.4"/>
                    <rect x="16" y="66" width="44" height="3.4"/><rect x="44" y="72" width="16" height="3.4"/>
                    <rect x="30" y="78" width="30" height="3.4"/><rect x="40" y="84" width="20" height="3.4"/>
                    <rect x="34" y="90" width="26" height="3.4"/>
                  </g>
                  <g fill="var(--green)" opacity=".85">
                    <rect x="34" y="106" width="26" height="3.4"/><rect x="22" y="112" width="38" height="3.4"/>
                    <rect x="42" y="118" width="18" height="3.4"/><rect x="12" y="124" width="48" height="3.4"/>
                    <rect x="30" y="130" width="30" height="3.4"/><rect x="38" y="136" width="22" height="3.4"/>
                    <rect x="18" y="142" width="42" height="3.4"/><rect x="26" y="148" width="34" height="3.4"/>
                    <rect x="40" y="154" width="20" height="3.4"/><rect x="10" y="160" width="50" height="3.4"/>
                    <rect x="32" y="166" width="28" height="3.4"/><rect x="24" y="172" width="36" height="3.4"/>
                    <rect x="36" y="178" width="24" height="3.4"/><rect x="16" y="184" width="44" height="3.4"/>
                    <rect x="28" y="190" width="32" height="3.4"/>
                  </g>
                </svg>"""

RegionFn = Callable[[str, dict[str, dict], Problems], str]


def _n(zones: dict[str, dict], zid: str, problems: Problems) -> str:
    """Return the badge number for one zone, or mark the layout as broken."""
    zone = zones.get(zid)
    if zone is None:
        problems.add("layouts.yml", f"region asks for zone {zid!r} which is not in this mode")
        return "?"
    return str(zone["n"])


def _btn(zid: str, n: str, class_name: str, inner: str, extra: str = "") -> str:
    """One clickable schematic region with its badge."""
    cls = f"z {class_name}".strip()
    return (
        f'<button class="{cls}" data-zone="{emit.text(zid)}" type="button"{extra}>'
        f'<i class="zn">{emit.text(n)}</i>{inner}</button>'
    )


def _brand(n: str) -> str:
    return _btn(
        "brand",
        n,
        "brand",
        f'<svg class="moon" viewBox="0 0 24 24" aria-hidden="true">'
        f'<path d="{MOON_PATH}" fill="currentColor"/></svg>',
    )


def _toolbar(zones: dict[str, dict], problems: Problems) -> str:
    n = lambda zid: _n(zones, zid, problems)
    return f"""        <div class="tb">
          {_btn("size", n("size"), "grp", '''
            <span class="k lab mono" data-i18n="ui.size">size</span>
            <span class="k mono">50</span><span class="k mono">100</span>
            <span class="k on mono">250</span><span class="k mono">500</span>
            <span class="k mono">1000</span><span class="k mono">2500</span>
          ''')}
          <span class="vr"></span>
          {_btn("lev", n("lev"), "grp", '<span class="k on mono">Lev ×5</span>')}
          <span class="vr"></span>
          {_btn("risk", n("risk"), "grp", '''
            <span class="k lab mono">SL</span><span class="sw"></span>
            <span class="k mono" style="color:var(--red-text)">−2.25%</span>
          ''')}
          <span class="vr"></span>
          {_btn("exit", n("exit"), "grp", '''
            <span class="k on mono">TP 2.00%</span>
            <span class="k mono">+1%</span><span class="k mono">+3%</span>
            <span class="k mono">+5%</span><span class="k mono">+10%</span><span class="k mono">+100%</span>
          ''')}
          <span class="vr"></span>
          {_btn("live", n("live"), "grp", '<span class="k mono" style="color:var(--green-text)">● Live</span>')}
          <span class="grow"></span>
          {_btn("launch", n("launch"), "grp", '''
            <span class="pill ghost" data-i18n="ui.pm">Монитор прибыли</span>
            <span class="pill ghost" data-i18n="ui.scr">Скринер</span>
            <span class="pill ghost" data-i18n="ui.str">Стратегии</span>
            <span class="pill ghost" data-i18n="ui.an">Аналитика</span>
            <span class="pill ghost" data-i18n="ui.set">Настройки</span>
          ''')}
        </div>"""


def _chart_canvas(zones: dict[str, dict], problems: Problems) -> str:
    n = lambda zid: _n(zones, zid, problems)
    return f"""            <div class="canvas">
              {_btn("chart", n("chart"), "plot", '<svg class="chartsvg" viewBox="0 0 420 200" preserveAspectRatio="none" aria-hidden="true"></svg>', ' aria-label="Chart"')}
              {_btn("book", n("book"), "book", BOOK_SVG, ' aria-label="Order book"')}
            </div>"""


def _status(zones: dict[str, dict], problems: Problems) -> str:
    n = _n(zones, "status", problems)
    return f"""        {_btn("status", n, "sb", '''
          <span><span data-i18n="ui.conn">Соединение:</span> <b>OK</b></span>
          <span>Binance Futures</span><span>ping 32ms</span>
          <span data-i18n="ui.mode">Режим: Demo</span><span>PRO</span>
          <span>book 1384</span><span>25 fps</span>
          <span>CPU 2%/5%</span><span>GPU 6%</span><span>RAM 6363 MB</span>
        ''', ' style="width:100%"')}"""


def _orders_table(extra_rows: bool) -> str:
    rows = """
                  <tr><td>BinF1</td><td class="buy">BUY</td><td>ACT</td><td>42593</td><td class="on">ON</td><td>—</td></tr>
                  <tr><td>BB1</td><td class="sell">Short-B</td><td>TA</td><td>840</td><td class="on">ON</td><td class="neg">−0.05%</td></tr>
                  <tr><td>BinF3</td><td class="sell">SELL</td><td>PYTH</td><td>9933</td><td class="on">ON</td><td class="neg">−1.88%</td></tr>
                  <tr><td>Bitget1</td><td class="sell">SELL</td><td>MAPO</td><td>35483</td><td class="off">OFF</td><td class="neg">−0.61%</td></tr>
                  <tr><td>F5</td><td class="buy">BUY</td><td>SYN</td><td>2681</td><td class="on">ON</td><td>—</td></tr>"""
    if extra_rows:
        rows += """
                  <tr><td>BinF1</td><td class="buy">BUY</td><td>DOGE</td><td>1200</td><td class="on">ON</td><td>—</td></tr>
                  <tr><td>BB1</td><td class="sell">SELL</td><td>SOL</td><td>44</td><td class="on">ON</td><td class="neg">−0.12%</td></tr>
                  <tr><td>BinF3</td><td class="buy">BUY</td><td>ATOM</td><td>310</td><td class="off">OFF</td><td>—</td></tr>
                  <tr><td>Bitget1</td><td class="sell">SELL</td><td>NEAR</td><td>880</td><td class="on">ON</td><td class="neg">−0.40%</td></tr>"""
    return f"""              <table class="t">
                <thead><tr>
                  <th data-i18n="col.core">Ядро</th><th data-i18n="col.side">Сторона</th>
                  <th data-i18n="col.token">Токен</th><th data-i18n="col.size">Size</th>
                  <th>SL</th><th data-i18n="col.pnl">PNL %</th>
                </tr></thead>
                <tbody>{rows}
                </tbody>
              </table>"""


def render_header(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """Shared header chrome; Auto pins the core pill and hides Overview balance."""
    n = lambda zid: _n(zones, zid, problems)
    auto = mode_id == "auto"
    if auto:
        ws_inner = '<span data-i18n="ui.ws.auto">AUTO режим</span><span class="caret">▾</span>'
        core_inner = (
            '<span class="dot accent"></span>'
            '<span data-i18n="ui.overview">Полная сводка</span>'
            '<span style="opacity:.5">⚙</span>'
        )
        core_class = "pill is-pinned"
        balance = ""
    else:
        ws_inner = '<span data-i18n="ui.ws.manual">MANUAL режим</span><span class="caret">▾</span>'
        core_inner = (
            '<span class="dot"></span><span class="mono">BinF1</span>'
            '<span class="caret">▾</span><span style="opacity:.5">⚙</span>'
        )
        core_class = "pill"
        balance = _btn(
            "balance",
            n("balance"),
            "",
            '<span style="color:var(--text-muted)" data-i18n="ui.balance">Баланс:</span>'
            '<span class="mono" style="font-weight:600"> 975.45</span>'
            '<span class="mono" style="color:var(--text-muted)">/2492 USDT</span>',
            ' style="padding:0 6px"',
        ) + "\n          "
    return f"""        <div class="hdr">
          {_brand(n("brand"))}
          {_btn("ws", n("ws"), "pill ghost", ws_inner)}
          {_btn("core", n("core"), core_class, core_inner)}
          {balance}<span class="grow"></span>
          {_btn("ticker", n("ticker"), "", '''
            <span class="mono">1 BTC = <b>62 773$</b></span>
            <span class="mono" style="color:var(--green-text)"> +0.1%</span>
            <span class="mono" style="color:var(--red-text)"> −0.2%</span>
          ''', ' style="padding:0 6px"')}
          <span class="vr"></span>
          {_btn("clock", n("clock"), "", '<span class="mono">15:16:35</span> <span class="mono" style="color:var(--text-muted)">(WAW)</span>', ' style="padding:0 6px"')}
          <span class="wctl mono">− □ ✕</span>
        </div>"""


def render_toolbar(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """Trading toolbar is the same strip in both workspaces."""
    del mode_id
    return _toolbar(zones, problems)


def render_classic_mid(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """Classic centre: chart tabs, plot, book, and the Detects ribbon."""
    del mode_id
    n = lambda zid: _n(zones, zid, problems)
    return f"""        <div class="mid">
          <div class="chartcol">
            <div class="ctabs">
              {_btn("ctabs", n("ctabs"), "ctab sel", "Main")}
              <span class="grow"></span>
              <span class="pill ghost mono" data-i18n="ui.coin">Монета…</span>
              <span class="pill ghost mono">🔍 20%</span>
            </div>
{_chart_canvas(zones, problems)}
          </div>
          {_btn("detects", n("detects"), "rail", '''
            <span class="dcard"><b>NAKA_</b><span class="mono">52s</span><span class="badge">Gate</span></span>
            <span class="dcard"><b>ES</b><span class="mono">18s</span><span class="badge">BybitSlava</span></span>
            <span class="dcard"><b>ULTIMA_</b><span class="mono">10s</span><span class="badge">Gate</span></span>
            <span class="dcard"><b>TLM</b><span class="mono">46s</span><span class="badge">BybitSlava</span></span>
          ''')}
        </div>"""


def render_classic_dock(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """Classic lower dock, including Classic-only News and Alerts tabs."""
    del mode_id
    n = lambda zid: _n(zones, zid, problems)
    return f"""        <div class="dock">
          {_btn("dtabs", n("dtabs"), "dtabs", '''
            <span class="dtab sel" data-i18n="tab.orders">Ордера</span>
            <span class="dtab" data-i18n="tab.assets">Активы</span>
            <span class="dtab" data-i18n="tab.report">Отчёт</span>
            <span class="dtab" data-i18n="tab.alerts">Фигуры</span>
            <span class="dtab" data-i18n="tab.news">Новости</span>
            <span class="dtab" data-i18n="tab.core_status">Статус ядер</span>
            <span class="dtab" data-i18n="tab.log">Лог</span>
            <span class="dtab" data-i18n="tab.detects">Детекты</span>
          ''', ' style="width:100%"')}
          <div class="z dbody" data-zone="table" role="button" tabindex="0"><i class="zn">{n("table")}</i>
            <div class="dpane">
              <span class="filters">
                <span class="pill ghost" data-i18n="ui.allcores">Все ядра ▾</span>
                <span class="pill ghost" data-i18n="ui.real">Реальные ▾</span>
                <span class="pill ghost" data-i18n="ui.fields">Поля ▾</span>
                <span class="mono" style="color:var(--text-muted)">852</span>
              </span>
{_orders_table(False)}
            </div>
            <div class="dpane">
              <span class="filters">
                <span class="mono" style="font-weight:600" data-i18n="tab.assets">Активы</span>
                <span class="grow"></span>
                <span class="mono" style="color:var(--text-muted)">Σ 10 244,63$</span>
              </span>
              <table class="t">
                <thead><tr><th data-i18n="col.core">Ядро</th><th data-i18n="col.asset">Актив</th><th data-i18n="col.qty">Кол-во</th><th data-i18n="col.val">Стоим.$</th></tr></thead>
                <tbody>
                  <tr><td>BinKEAUSDC</td><td>BTC</td><td>0.162</td><td>10 185,83$</td></tr>
                  <tr><td>BinKEAUSDC</td><td>BNB</td><td>0.077</td><td>45,39$</td></tr>
                  <tr><td>Bitget1</td><td>BGB</td><td>7.797</td><td>13,4$</td></tr>
                </tbody>
              </table>
            </div>
          </div>
        </div>"""


def render_auto_body(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """AutoTrading body: recessed core rail beside the shared operational dock."""
    del mode_id
    n = lambda zid: _n(zones, zid, problems)
    return f"""        <div class="mid auto-mid">
          <div class="auto-rail">
            {_btn("summary", n("summary"), "rail-summary", '<span class="mono" data-i18n="ui.summary">Ядер: 5 · готово: 4 · проблем: 1</span>')}
            {_btn("overview", n("overview"), "rail-overview sel", '''
              <span class="dot accent"></span>
              <span data-i18n="ui.overview">Полная сводка</span>
            ''')}
            {_btn("cores", n("cores"), "rail-cores", '''
              <span class="exch">Binance Futures</span>
              <span class="core-row"><span class="dot"></span><span class="mono">BinF1</span></span>
              <span class="core-row"><span class="dot"></span><span class="mono">BinF3</span></span>
              <span class="exch">Bitget</span>
              <span class="core-row"><span class="dot warn"></span><span class="mono">Bitget1</span><span class="st" data-i18n="ui.status.problem">Проблема</span></span>
              <span class="exch">Bybit</span>
              <span class="core-row"><span class="dot"></span><span class="mono">BB1</span></span>
              <span class="core-row off"><span class="dot muted"></span><span class="mono">F5</span><span class="st" data-i18n="ui.status.disabled">Отключено</span></span>
            ''')}
          </div>
          <div class="auto-dock">
            {_btn("atabs", n("atabs"), "atabs", '''
              <span class="ctab sel pin" data-i18n="tab.charts">Графики</span>
              <span class="pin-sep" aria-hidden="true"></span>
              <span class="ctab" data-i18n="tab.report">Отчёт</span>
              <span class="ctab" data-i18n="tab.assets">Активы</span>
              <span class="ctab" data-i18n="tab.core_status">Статус ядер</span>
              <span class="ctab" data-i18n="tab.log">Лог</span>
              <span class="ctab" data-i18n="tab.detects">Детекты</span>
            ''')}
            <div class="chartcol">
{_chart_canvas(zones, problems)}
            </div>
            <div class="z auto-orders" data-zone="orders" role="button" tabindex="0"><i class="zn">{n("orders")}</i>
              <span class="filters">
                <span class="mono" style="font-weight:600" data-i18n="tab.orders">Ордера</span>
                <span class="pill ghost" data-i18n="ui.allcores">Все ядра ▾</span>
                <span class="grow"></span>
                <span class="mono" style="color:var(--text-muted)">852</span>
              </span>
{_orders_table(True)}
            </div>
          </div>
        </div>"""


def render_status(mode_id: str, zones: dict[str, dict], problems: Problems) -> str:
    """Status bar strip shared by both workspaces."""
    del mode_id
    return _status(zones, problems)


RENDERERS: dict[str, RegionFn] = {
    "header": render_header,
    "toolbar": render_toolbar,
    "classic_mid": render_classic_mid,
    "classic_dock": render_classic_dock,
    "auto_body": render_auto_body,
    "status": render_status,
}


def _zones_for(content: Content, mode_id: str) -> dict[str, dict]:
    return {z["id"]: z for z in content.zones if z["mode"] == mode_id}


def _check_app(html: str, mode_id: str, zones: dict[str, dict], problems: Problems) -> None:
    """Every layout zone must appear as a clickable region, and no extras."""
    found = set(re.findall(r'data-zone="([^"]+)"', html))
    expected = set(zones)
    missing = sorted(expected - found)
    extra = sorted(found - expected)
    if missing:
        problems.add(
            f"map.py: mode {mode_id}",
            f"layout did not render zone(s) {missing}",
            "the annotation rail would have nowhere to point",
        )
    if extra:
        problems.add(
            f"map.py: mode {mode_id}",
            f"rendered unknown data-zone(s) {extra}",
            "every clickable region must exist in zones.yml for this mode",
        )


def render_app(content: Content, mode: dict, problems: Problems) -> str:
    """One window replica for a single workspace mode."""
    mode_id = mode["id"]
    zones = _zones_for(content, mode_id)
    regions = content.layouts.get(mode_id) or []
    parts = []
    for region in regions:
        rid = region["id"]
        renderer = RENDERERS.get(rid)
        if renderer is None:
            problems.add("layouts.yml", f"unknown region {rid!r} in mode {mode_id}")
            continue
        parts.append(renderer(mode_id, zones, problems))
    body = "\n".join(parts)
    html = (
        f'<div class="app app-map reveal" data-mode="{emit.text(mode_id)}" '
        f'id="app-{emit.text(mode_id)}">\n'
        f"{body}\n"
        f"      </div>"
    )
    _check_app(html, mode_id, zones, problems)
    return html


def map_leads(content: Content) -> str:
    """Per-mode map leads; CSS shows the one matching the selected radio."""
    first = content.codes[0]
    lines = []
    for mode in content.modes:
        lines.append(
            f'<p class="slead" data-mode-lead="{emit.text(mode["id"])}">'
            f"{emit.text(mode['lead'].get(first))}</p>"
        )
    return "\n      ".join(lines)


def mode_switch(content: Content) -> str:
    """Accessible Classic / AutoTrading switch driven by the modes content file."""
    first = content.codes[0]
    group = emit.text(content.page["map.mode_group"].get(first))
    options = []
    for mode in content.modes:
        mid = emit.text(mode["id"])
        checked = " checked" if mode.get("default") else ""
        label = emit.text(mode["label"].get(first))
        tip = emit.text(mode["tip"].get(first))
        options.append(
            f'<label class="mode-opt" title="{tip}">'
            f'<input class="tour-mode-input" type="radio" name="tour-mode" '
            f'id="mode-{mid}" value="{mid}"{checked}>'
            f'<span class="mode-label">{label}</span></label>'
        )
    inner = "\n          ".join(options)
    return (
        f'<div class="seg mode-switch" role="radiogroup" '
        f'aria-label="{group}">\n          {inner}\n        </div>'
    )


def window_maps(content: Content, problems: Problems) -> str:
    """Stacked window replicas, one per first-class mode."""
    apps = "\n\n      ".join(render_app(content, mode, problems) for mode in content.modes)
    return f'<div class="mapstage">\n      {apps}\n      </div>'


def map_annotations(content: Content) -> str:
    """No-JavaScript readable copy of every zone, grouped by mode."""
    first = content.codes[0]
    blocks = []
    for mode in content.modes:
        items = []
        for zone in (z for z in content.zones if z["mode"] == mode["id"]):
            title = emit.text(zone["title"].get(first))
            body = emit.text(zone["body"].get(first))
            items.append(f"<li><strong>{title}</strong> — {body}</li>")
        label = emit.text(mode["label"].get(first))
        blocks.append(
            f"<h3>{label}</h3>\n    <ol>\n      " + "\n      ".join(items) + "\n    </ol>"
        )
    inner = "\n    ".join(blocks)
    return f'<noscript>\n    <div class="ns-maps wrap">\n    {inner}\n    </div>\n    </noscript>'
