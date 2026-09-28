(function () {
    "use strict";

    var SVG = "http://www.w3.org/2000/svg";
    var POLL_MS = 2000;
    // The backend keeps a finished report for 15 s, so a faster report poll returns the same data.
    var REPORT_POLL_MS = 15000;
    var RETRY_MS = [1000, 2000, 4000];
    var REPORT_RETRY_LIMIT = 6;
    var REPORT_RETRY_MS = 2000;
    // The server answers within 5 s; a request still open well past that is dead, and the queue is serial.
    var FETCH_TIMEOUT_MS = 12000;
    var TAB_NAMES = ["report", "cores", "balances", "orders", "trades", "strategies"];
    // Nav buttons by data-tab; "deals" shows the orders or trades pane.
    var TAB_KEYS = {
        report: "mini_tab_report",
        cores: "mini_tab_cores",
        balances: "mini_tab_balances",
        strategies: "mini_tab_strategies",
        deals: "mini_tab_trades"
    };
    var PERIODS = [
        ["today", "mini_period_today"],
        ["yesterday", "mini_period_yesterday"],
        ["month", "mini_period_month"],
        ["last_month", "mini_period_lastmonth"]
    ];

    var labels = window.telegramLabels || {};
    var webapp = window.Telegram && window.Telegram.WebApp;
    var title = document.getElementById("app-title");
    var updated = document.getElementById("app-updated");
    var startup = document.getElementById("startup");
    var main = document.getElementById("app-main");
    var nav = document.getElementById("app-nav");
    var sections = {};
    // One nav tab holds open orders and closed trades; this is the segment it shows.
    var dealsSegment = "orders";
    var dealsSwitch = null;
    var buttons = {};
    var payloads = {};
    var queries = { orders: "" };
    var collapse = { cores: {}, balances: {}, orders: {}, strategies: {} };
    var collapseUser = { cores: {}, balances: {}, orders: {}, strategies: {} };
    var hasData = {};
    var queue = [];
    var busy = false;
    var currentJob = null;
    var pollTimer = null;
    var cmdTimer = null;
    var period = "today";
    var current = null;
    // The balances total starts masked each time the pane opens; never persisted.
    var balanceRevealed = false;
    var loadToken = 0;
    var sessionOk = false;
    var commandBusy = false;
    var reportPeriod = null;
    var inFlight = {};
    var reportBody = null;
    var refreshButton = document.getElementById("app-refresh");
    var paneStatus = document.getElementById("pane-status");
    var header = document.querySelector(".app-header");
    var lastOk = {};
    var lastRaw = {};
    var updatedTimer = null;
    var reportExpanded = false;
    var MONEY_LIST_LIMIT = 8;
    // Core detail screen: the open core id, and the list scroll to return to.
    var coreDetailId = null;
    var coreListY = 0;
    var sheet = document.getElementById("sheet");
    var sheetBackdrop = null;
    var sheetOpen = false;

    function tr(key) {
        var value = labels[key];
        return typeof value === "string" ? value : "";
    }

    function trf(key, vars) {
        var s = tr(key);
        for (var k in vars) {
            if (Object.prototype.hasOwnProperty.call(vars, k)) s = s.split("{" + k + "}").join(String(vars[k]));
        }
        return s;
    }

    // Plural form for a count: "one", "few" or "many" (other falls to many).
    function pluralForm(n) {
        var form = "many";
        try {
            form = new Intl.PluralRules(labels.locale || "en").select(n);
        } catch (err) {
            form = n === 1 ? "one" : "many";
        }
        return form === "one" || form === "few" ? form : "many";
    }

    if (labels.locale) {
        document.documentElement.lang = labels.locale;
    }

    function applyScheme() {
        var scheme = webapp && webapp.colorScheme;
        if (scheme === "dark" || scheme === "light") {
            document.documentElement.style.colorScheme = scheme;
            document.documentElement.dataset.scheme = scheme;
        }
    }

    // Telegram 6.9+ takes a hex, so the chrome matches the terminal page colour.
    function applyChromeColors() {
        if (!webapp) return;
        var color = "secondary_bg_color";
        if (typeof webapp.isVersionAtLeast === "function" && webapp.isVersionAtLeast("6.9")) {
            var page = getComputedStyle(document.documentElement).getPropertyValue("--page").trim();
            if (/^#[0-9a-fA-F]{6}$/.test(page)) color = page;
        }
        if (webapp.setHeaderColor) webapp.setHeaderColor(color);
        if (webapp.setBackgroundColor) webapp.setBackgroundColor(color);
        if (color.charAt(0) === "#" && typeof webapp.setBottomBarColor === "function") {
            webapp.setBottomBarColor(color);
        }
    }

    applyScheme();
    if (webapp) {
        if (webapp.onEvent) {
            webapp.onEvent("themeChanged", function () {
                applyScheme();
                applyChromeColors();
                syncHeaderHeight();
            });
        }
        webapp.ready();
        webapp.expand();
        applyChromeColors();
    }

    function initData() {
        return (webapp && webapp.initData) || "";
    }

    function el(tag, className, text) {
        var node = document.createElement(tag);
        if (className) node.className = className;
        if (text != null && text !== "") node.textContent = String(text);
        return node;
    }

    function clear(node) {
        while (node.firstChild) node.removeChild(node.firstChild);
    }

    function svgEl(name) {
        return document.createElementNS(SVG, name);
    }

    function signClass(value) {
        if (typeof value !== "number" || value !== value) return "hint";
        if (value > 0) return "pos";
        if (value < 0) return "neg";
        return "";
    }

    // Group PnL is a client sum of order.pnl. Match signed_fixed's visible
    // contract: half away from zero, two decimals, a sign only when non-zero.
    function signedFixed(value) {
        if (typeof value !== "number" || value !== value || value === Infinity || value === -Infinity) {
            return null;
        }
        var scaled = value * 100;
        if (scaled !== scaled || scaled === Infinity || scaled === -Infinity) return null;
        var away = (scaled < 0 ? -1 : 1) * Math.round(Math.abs(scaled));
        var rounded = away / 100;
        if (rounded === 0) return "0.00";
        return (rounded > 0 ? "+" : "-") + Math.abs(rounded).toFixed(2);
    }

    function applyMoney(node, baseClass, text, value) {
        if (text == null || text === "") {
            node.className = baseClass + " hint";
            node.textContent = tr("mini_unvalued");
            return;
        }
        var tone = signClass(value);
        node.className = baseClass + (tone ? " " + tone : "");
        node.textContent = text;
    }

    function focusSnap() {
        var active = document.activeElement;
        if (!active || !active.id || active.selectionStart == null) return null;
        return { id: active.id, start: active.selectionStart, end: active.selectionEnd };
    }

    function restoreSnap(snap, y) {
        if (snap) {
            var node = document.getElementById(snap.id);
            if (node) {
                node.focus();
                if (node.setSelectionRange && snap.start != null) {
                    node.setSelectionRange(snap.start, snap.end);
                }
            }
        }
        window.scrollTo(0, y || 0);
        syncBackButton();
    }

    // One POST at a time. 503 and 504 retry three times (1s, 2s, 4s) before mini_error_busy.
    // /api/report retries six times, two seconds apart, so a slow month read can land in the cache.
    function api(path, body, keep) {
        return new Promise(function (resolve) {
            queue.push({
                path: path,
                body: body || {},
                resolve: resolve,
                cancelled: false,
                keep: !!keep,
                attempt: 0,
                waiting: false,
                timer: null,
                abort: null
            });
            pump();
        });
    }

    function pump() {
        if (busy) return;
        while (queue.length && queue[0].cancelled) {
            var dropped = queue.shift();
            dropped.resolve({ ok: false, status: 0, cancelled: true, retry: false, error: "" });
        }
        if (!queue.length) return;
        busy = true;
        currentJob = queue.shift();
        runJob(currentJob);
    }

    function settle(job) {
        if (currentJob !== job) return;
        currentJob = null;
        busy = false;
        pump();
    }

    function cancelPending() {
        var kept = [];
        var i;
        for (i = 0; i < queue.length; i++) {
            if (queue[i].keep) kept.push(queue[i]);
            else queue[i].cancelled = true;
        }
        queue = kept;
        clearPoll();
        var job = currentJob;
        if (!job || job.keep) return;
        job.cancelled = true;
        if (job.timer) {
            clearTimeout(job.timer);
            job.timer = null;
        }
        if (job.abort) job.abort();
        if (job.waiting) {
            job.waiting = false;
            job.resolve({ ok: false, status: 0, cancelled: true, retry: false, error: "" });
            settle(job);
        }
    }

    function finish(job, result) {
        job.resolve(result);
        settle(job);
    }

    function runJob(job) {
        if (job.cancelled) {
            finish(job, { ok: false, status: 0, cancelled: true, retry: false, error: "" });
            return;
        }
        var controller = typeof AbortController === "function" ? new AbortController() : null;
        job.abort = controller ? function () { controller.abort(); } : null;
        var opts = {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                "X-Telegram-Init-Data": initData()
            },
            body: JSON.stringify(job.body)
        };
        if (controller) opts.signal = controller.signal;
        var deadline = controller ? setTimeout(function () { controller.abort(); }, FETCH_TIMEOUT_MS) : null;
        fetch(job.path, opts).then(function (response) {
            if (deadline) clearTimeout(deadline);
            return response.text().then(function (text) {
                var parsed = null;
                if (text) {
                    try {
                        parsed = JSON.parse(text);
                    } catch (err) {
                        parsed = null;
                    }
                }
                return { status: response.status, ok: response.ok, parsed: parsed, text: text };
            });
        }).then(function (pack) {
            if (job.cancelled) {
                finish(job, { ok: false, status: 0, cancelled: true, retry: false, error: "" });
                return;
            }
            if (pack.status === 503 || pack.status === 504) {
                if (job.attempt < retryLimit(job)) {
                    var wait = retryWait(job);
                    job.attempt += 1;
                    job.waiting = true;
                    job.timer = setTimeout(function () {
                        job.timer = null;
                        job.waiting = false;
                        if (job.cancelled) {
                            finish(job, { ok: false, status: 0, cancelled: true, retry: false, error: "" });
                            return;
                        }
                        runJob(job);
                    }, wait);
                    return;
                }
                finish(job, { ok: false, status: pack.status, error: tr("mini_error_busy"), retry: true });
                return;
            }
            if (pack.status === 401) {
                finish(job, { ok: false, status: 401, error: tr("mini_error_stale_session"), retry: false });
                return;
            }
            if (pack.status === 403) {
                finish(job, { ok: false, status: 403, error: tr("mini_shell_denied"), retry: false });
                return;
            }
            if (pack.status === 500) {
                finish(job, { ok: false, status: 500, error: tr("mini_error_read"), retry: true });
                return;
            }
            if (!pack.ok || !pack.parsed || typeof pack.parsed !== "object") {
                finish(job, { ok: false, status: pack.status, error: tr("mini_error_read"), retry: true });
                return;
            }
            finish(job, { ok: true, status: pack.status, data: pack.parsed, raw: pack.text });
        }).catch(function () {
            if (deadline) clearTimeout(deadline);
            if (job.cancelled) {
                finish(job, { ok: false, status: 0, cancelled: true, retry: false, error: "" });
                return;
            }
            finish(job, { ok: false, status: 0, error: tr("mini_error_network"), retry: true });
        });
    }

    // Database reads (report, trades) get the long retry so a slow read can land in the cache.
    function slowRead(path) {
        return path === "/api/report" || path === "/api/trades";
    }

    function retryLimit(job) {
        return slowRead(job.path) ? REPORT_RETRY_LIMIT : RETRY_MS.length;
    }

    function retryWait(job) {
        if (slowRead(job.path)) return REPORT_RETRY_MS;
        return RETRY_MS[job.attempt];
    }

    function clearPoll() {
        if (pollTimer) {
            clearTimeout(pollTimer);
            pollTimer = null;
        }
    }

    function schedulePoll(name, token) {
        clearPoll();
        if (document.visibilityState !== "visible") return;
        pollTimer = setTimeout(function () {
            pollTimer = null;
            if (!sessionOk || token !== loadToken || current !== name) return;
            if (document.visibilityState !== "visible") return;
            loadTab(name, token, true);
        }, name === "report" || name === "trades" ? REPORT_POLL_MS : POLL_MS);
    }

    function pathFor(name) {
        if (name === "report") return "/api/report";
        if (name === "cores") return "/api/cores";
        if (name === "balances") return "/api/balances";
        if (name === "trades") return "/api/trades";
        if (name === "strategies") return "/api/strategies";
        return "/api/orders";
    }

    function bodyFor(name) {
        if (name === "report") return { period: period };
        return {};
    }

    // One generic skeleton for every tab: a hero bar and five rows.
    function skeleton(host) {
        host.appendChild(el("p", "sr-only", tr("mini_loading")));
        var card = el("div", "card");
        card.setAttribute("aria-hidden", "true");
        card.appendChild(el("div", "skel skel-hero"));
        var widths = ["72%", "48%", "64%", "40%", "56%"];
        var i;
        for (i = 0; i < widths.length; i++) {
            var row = el("div", "skel-row");
            var bar = el("div", "skel");
            bar.style.maxWidth = widths[i];
            row.appendChild(bar);
            row.appendChild(el("div", "skel"));
            card.appendChild(row);
        }
        host.appendChild(card);
    }

    function showLoading(host) {
        clear(host);
        skeleton(host);
    }

    function button(className, label, onClick) {
        var node = document.createElement("button");
        node.type = "button";
        node.className = className;
        node.textContent = label;
        node.addEventListener("click", onClick);
        return node;
    }

    function retryButton(className) {
        return button(className, tr("mini_retry"), refreshCurrent);
    }

    function clearStale() {
        if (paneStatus) clear(paneStatus);
    }

    function showStale(message, allowRetry) {
        if (!paneStatus) return;
        clear(paneStatus);
        var bar = el("div", "stale-bar");
        var text = tr("mini_stale_data");
        var line = el("span", "stale-text", text);
        if (message) line.appendChild(el("span", "stale-reason", message));
        bar.appendChild(line);
        if (allowRetry) bar.appendChild(retryButton(""));
        paneStatus.appendChild(bar);
    }

    function showError(host, message, allowRetry) {
        clear(host);
        var box = el("div", "error");
        box.appendChild(el("p", "", message || tr("mini_error_read")));
        if (allowRetry) box.appendChild(retryButton("retry"));
        host.appendChild(box);
    }

    // An optional action ({label, run}) gives the empty pane a way forward.
    function emptyState(message, action) {
        var box = el("div", "empty");
        var icon = svgEl("svg");
        icon.setAttribute("viewBox", "0 0 48 48");
        icon.setAttribute("class", "empty-icon");
        icon.setAttribute("aria-hidden", "true");
        var circle = svgEl("circle");
        circle.setAttribute("cx", "24");
        circle.setAttribute("cy", "24");
        circle.setAttribute("r", "14");
        circle.setAttribute("fill", "none");
        circle.setAttribute("stroke", "currentColor");
        circle.setAttribute("stroke-width", "2");
        var path = svgEl("path");
        path.setAttribute("d", "M18 24h12");
        path.setAttribute("fill", "none");
        path.setAttribute("stroke", "currentColor");
        path.setAttribute("stroke-width", "2");
        path.setAttribute("stroke-linecap", "round");
        icon.appendChild(circle);
        icon.appendChild(path);
        box.appendChild(icon);
        box.appendChild(el("p", "", message));
        if (action && action.label) box.appendChild(button("retry", action.label, action.run));
        return box;
    }

    function refreshAction() {
        return { label: tr("mini_refresh"), run: refreshCurrent };
    }

    function clearSearchAction(pane, repaint) {
        return {
            label: tr("mini_clear_search"),
            run: function () {
                queries[pane] = "";
                repaint();
            }
        };
    }

    function searchField(id, value, onInput) {
        var input = document.createElement("input");
        input.type = "search";
        input.id = id;
        input.className = "search";
        input.value = value || "";
        input.placeholder = tr("mini_search");
        input.setAttribute("aria-label", tr("mini_search"));
        input.setAttribute("autocomplete", "off");
        input.setAttribute("autocapitalize", "off");
        input.setAttribute("spellcheck", "false");
        input.addEventListener("input", function () {
            onInput(input.value);
        });
        return input;
    }

    function matchesQuery(query, fields) {
        if (!query) return true;
        var needle = String(query).toLowerCase();
        var i;
        for (i = 0; i < fields.length; i++) {
            if (fields[i] == null) continue;
            if (String(fields[i]).toLowerCase().indexOf(needle) !== -1) return true;
        }
        return false;
    }

    function filterItems(items, query, fieldsOf) {
        if (!query) return items;
        var out = [];
        var i;
        for (i = 0; i < items.length; i++) {
            if (matchesQuery(query, fieldsOf(items[i]))) out.push(items[i]);
        }
        return out;
    }

    function groupBy(items, keyFn) {
        var order = [];
        var map = {};
        var i;
        for (i = 0; i < items.length; i++) {
            var key = keyFn(items[i]);
            var slot = "k:" + key;
            if (!Object.prototype.hasOwnProperty.call(map, slot)) {
                map[slot] = [];
                order.push(key);
            }
            map[slot].push(items[i]);
        }
        var groups = [];
        for (i = 0; i < order.length; i++) {
            groups.push({ key: order[i], items: map["k:" + order[i]] });
        }
        return groups;
    }

    function coreProblem(core) {
        return !core || core.conn !== "ready" || !!core.fault;
    }

    function balanceProblem(row) {
        return !row || row.state !== "live";
    }

    function groupKeyOf(item, keyFn) {
        if (typeof keyFn === "function") return String(keyFn(item) || "");
        return item && item.exchange ? String(item.exchange) : "";
    }

    // Orders and Strategies groups start collapsed, including on a later refresh. A group the
    // user has toggled keeps that choice while the popup stays open. Search
    // still draws matching groups open and clearing it restores this choice.
    function ensureCollapse(pane, items, keyFn) {
        var groups = groupBy(items, function (item) { return groupKeyOf(item, keyFn); });
        var i;
        for (i = 0; i < groups.length; i++) {
            var slot = "k:" + groups[i].key;
            if (collapseUser[pane][slot]) continue;
            collapse[pane][slot] = true;
        }
    }

    // Exchange header while the group is collapsed: online N of M, danger when
    // any core is offline or faulted.
    function coreGroupSummary(items) {
        var online = 0;
        var trouble = false;
        var i;
        for (i = 0; i < items.length; i++) {
            var core = items[i];
            if (core && core.conn === "ready") online += 1;
            if (coreProblem(core)) trouble = true;
        }
        var span = el("span", trouble ? "count num neg" : "count num");
        span.textContent = tr("mini_cores_online")
            .replace("{online}", String(online))
            .replace("{total}", String(items.length));
        return span;
    }

    function exchangeTotal(name) {
        var data = payloads.balances || {};
        var rows = Array.isArray(data.per_exchange) ? data.per_exchange : [];
        var i;
        for (i = 0; i < rows.length; i++) {
            if (rows[i] && rows[i].exchange === name) return rows[i];
        }
        return null;
    }

    // Exchange header uses the preformatted two-decimal total already on the page.
    function balanceGroupSummary(items) {
        var name = items.length ? String(items[0].exchange || "") : "";
        var found = exchangeTotal(name);
        var wrap = el("span", "group-meta");
        var span = el("span", "num balance-figure");
        if (!found) applyMoney(span, "num balance-figure", null, null);
        else applyMoney(span, "num balance-figure", found.total_text, found.total);
        wrap.appendChild(span);
        return wrap;
    }

    // Core header: order count, plus the sum of finite order.pnl figures.
    // An order that has no position yet has no pnl, because order_pnl returns
    // nothing, and that row adds nothing. The chart position caption skips
    // the same rows. The header shows a dash only when no order in the group
    // has a finite figure.
    function orderGroupSummary(items) {
        var wrap = el("span", "group-meta");
        wrap.appendChild(el("span", "num", String(items.length)));
        var sum = 0;
        var known = 0;
        var i;
        for (i = 0; i < items.length; i++) {
            var pnl = items[i] && items[i].pnl;
            if (typeof pnl !== "number" || pnl !== pnl || pnl === Infinity || pnl === -Infinity) {
                continue;
            }
            known += 1;
            sum += pnl;
        }
        var text = known ? signedFixed(sum) : null;
        var fig = el("span", "num");
        if (!known || text == null) {
            fig.className = "num hint";
            fig.textContent = "\u2014";
        } else {
            applyMoney(fig, "num", text, sum);
        }
        wrap.appendChild(fig);
        return wrap;
    }

    function appendGroups(parent, pane, items, isProblem, renderRow, query, repaint, keyFn, labelFn, nameClass, summaryFn) {
        var groups = groupBy(items, function (item) { return groupKeyOf(item, keyFn); });
        var g;
        for (g = 0; g < groups.length; g++) {
            var group = groups[g];
            // The server already sends rows by exchange section, then by name.
            var ordered = group.items;
            var problems = 0;
            var j;
            for (j = 0; j < ordered.length; j++) {
                if (isProblem(ordered[j])) problems += 1;
            }
            var slot = "k:" + group.key;
            var collapsed = !query && !!collapse[pane][slot];
            var card = el("div", "card");
            var head = document.createElement("button");
            head.type = "button";
            head.className = "row group-head";
            head.setAttribute("aria-expanded", collapsed ? "false" : "true");
            var title = group.key;
            if (typeof labelFn === "function" && group.items.length) title = labelFn(group.items[0]);
            head.appendChild(el("span", "chev", collapsed ? "\u25B8" : "\u25BE"));
            head.appendChild(el("span", nameClass || "name grow", title || ""));
            var summary = typeof summaryFn === "function" ? summaryFn(ordered) : null;
            if (summary) head.appendChild(summary);
            else {
                head.appendChild(el("span", "count num", String(ordered.length)));
                if (problems > 0) head.appendChild(el("span", "num neg", String(problems)));
            }
            (function (key) {
                head.addEventListener("click", function () {
                    hapticSelection();
                    collapse[pane][key] = !collapse[pane][key];
                    collapseUser[pane][key] = true;
                    repaint();
                });
            })(slot);
            card.appendChild(head);
            if (!collapsed) {
                for (j = 0; j < ordered.length; j++) card.appendChild(renderRow(ordered[j]));
            }
            parent.appendChild(card);
        }
    }

    function dotClass(core) {
        if (core.fault || core.conn === "failed" || core.conn === "disconnected") return "dot dot-bad";
        if (core.conn === "ready") return "dot dot-ok";
        return "dot dot-wait";
    }

    function faultLabel(kind) {
        var specific = tr("mini_fault_" + kind);
        return specific || tr("mini_fault");
    }

    function metricLine(box, label, value, unit) {
        box.appendChild(el("span", "k", label));
        box.appendChild(el("span", "num", value));
        box.appendChild(el("span", "unit", unit));
    }

    function metricsBlock(core) {
        var box = el("div", "metrics");
        var any = false;
        if (typeof core.ping_ms === "number") {
            any = true;
            metricLine(box, tr("mini_ping"), String(core.ping_ms), tr("mini_unit_ms"));
        }
        if (typeof core.exch_ping_ms === "number") {
            any = true;
            metricLine(box, tr("mini_exch_ping"), String(core.exch_ping_ms), tr("mini_unit_ms"));
        }
        var cpu = typeof core.cpu_proc === "number" ? core.cpu_proc : core.cpu_sys;
        if (typeof cpu === "number") {
            any = true;
            metricLine(box, tr("mini_cpu"), String(Math.round(cpu * 10) / 10), tr("mini_unit_pct"));
        }
        return any ? box : null;
    }

    function switchChip(core, label, field, key) {
        var state = core[field];
        var known = state === true || state === false;
        var chip = document.createElement("button");
        chip.type = "button";
        chip.className = "cmd chip " + (known ? (state ? "on" : "off") : "unknown");
        // On/off reads from the chip colour, as the terminal's toggles do.
        chip.textContent = known ? label : label + " · " + tr("mini_state_unknown");
        chip.setAttribute("aria-pressed", known ? String(state) : "mixed");
        chip.disabled = commandBusy || !known;
        chip.addEventListener("click", function () {
            if (commandBusy || !known) return;
            runCommand(null, "/api/core/switch", { core: core.id, switch: key, on: !state });
        });
        return chip;
    }

    // Owner-only per-core switches; a single core fires without a confirm.
    function coreActions(core) {
        var line = el("div", "core-actions");
        line.appendChild(switchChip(core, tr("mini_trading"), "trading", "trading"));
        line.appendChild(switchChip(core, tr("mini_autodetect"), "auto_detect", "auto_detect"));
        var cancel = document.createElement("button");
        cancel.type = "button";
        cancel.className = "cmd chip cmd-danger";
        cancel.textContent = tr("mini_cancel_all");
        cancel.disabled = commandBusy;
        cancel.addEventListener("click", function () {
            if (commandBusy) return;
            runCommand(null, "/api/core/cancel_all", { core: core.id });
        });
        line.appendChild(cancel);
        return line;
    }

    function coreRow(core, data) {
        var row = el("div", "row core-row");
        row.setAttribute("role", "button");
        row.tabIndex = 0;
        row.addEventListener("click", function () {
            openCoreDetail(core.id);
        });
        row.addEventListener("keydown", function (event) {
            if (event.target !== row) return;
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            openCoreDetail(core.id);
        });
        row.appendChild(el("span", dotClass(core)));
        var body = el("div", "grow");
        var coreName = el("div", "name core-name", core.name || "");
        coreName.title = core.name || "";
        body.appendChild(coreName);
        var secondary = core.fault ? faultLabel(core.fault) : tr("mini_core_" + (core.conn || ""));
        if (secondary) body.appendChild(el("div", "sub", secondary));
        row.appendChild(body);
        var metrics = metricsBlock(core);
        if (metrics) row.appendChild(metrics);
        if (data && data.can_control) {
            var actions = coreActions(core);
            // A switch tap acts on the core; it never opens the detail screen.
            actions.addEventListener("click", function (event) {
                event.stopPropagation();
            });
            row.appendChild(actions);
        }
        return row;
    }

    function balanceBadge(state) {
        if (!state || state === "live") return null;
        var text = tr("mini_balance_" + state);
        if (!text) return null;
        return el("span", "badge badge-warn", text);
    }

    function balanceRow(row) {
        var wrap = el("div", "row");
        var top = el("div", "spread");
        top.appendChild(el("span", "name core-name", row.name || ""));
        var badge = balanceBadge(row.state);
        if (badge) top.appendChild(badge);
        wrap.appendChild(top);
        var bottom = el("div", "spread fine");
        var free = el("span", "");
        free.appendChild(el("span", "k", tr("mini_free") + " "));
        var freeVal = el("span", "num");
        applyMoney(freeVal, "num", row.free_text, row.free);
        free.appendChild(freeVal);
        var total = el("span", "money-col");
        total.appendChild(el("span", "k", tr("mini_total") + " "));
        var totalVal = el("span", "num");
        applyMoney(totalVal, "num balance-figure", row.total_text, row.total);
        total.appendChild(totalVal);
        bottom.appendChild(free);
        bottom.appendChild(total);
        wrap.appendChild(bottom);
        return wrap;
    }

    // The wire keeps buy/sell; the page names the position side. LONG / SHORT stay untranslated.
    function sideLabel(side) {
        if (side === "buy") return "LONG";
        if (side === "sell") return "SHORT";
        return side || "";
    }

    function sideClass(side) {
        if (side === "buy") return "pos";
        if (side === "sell") return "neg";
        return "hint";
    }

    function mutedBits(label, text) {
        var span = el("span", "");
        span.appendChild(el("span", "k", label));
        var shown = text == null || text === "" ? "\u2014" : text;
        span.appendChild(el("span", "num", shown));
        return span;
    }

    function orderChange(order) {
        var node = el("span", "num order-change");
        if (order.change_text) {
            var tone = signClass(order.change_pct);
            node.className = "num order-change" + (tone ? " " + tone : "");
            node.textContent = order.change_text;
            return node;
        }
        node.className = "num order-change hint";
        node.textContent = "\u2014";
        return node;
    }

    function orderResult(order) {
        var node = el("span", "num order-pnl");
        if (order.pnl_text) {
            applyMoney(node, "num order-pnl", order.pnl_text, order.pnl);
            return node;
        }
        node.className = "num order-pnl hint";
        node.textContent = "\u2014";
        return node;
    }

    function orderCoreKey(order) {
        return "id:" + String(order.core);
    }

    function orderCoreLabel(order) {
        return order.core_name || "";
    }

    function orderIsProblem() {
        return false;
    }

    // Owner order actions. Drawn only when can_control is true.
    function orderActions(row, order) {
        var line = el("div", "order-actions");
        var cancel = document.createElement("button");
        cancel.type = "button";
        cancel.className = "cmd";
        cancel.textContent = "✕";
        cancel.title = tr("mini_cancel");
        cancel.setAttribute("aria-label", tr("mini_cancel"));
        cancel.disabled = commandBusy;
        var panic = document.createElement("button");
        panic.type = "button";
        panic.className = "cmd cmd-danger";
        var panicLabel = tr(order.panic_armed ? "mini_panic_off" : "mini_panic_sell");
        panic.textContent = order.panic_armed ? "↺" : "⚡";
        panic.title = panicLabel;
        panic.setAttribute("aria-label", panicLabel);
        panic.disabled = commandBusy;
        cancel.addEventListener("click", function () {
            if (commandBusy) return;
            runCommand(
                withCoin(tr("mini_cancel_confirm"), order.coin),
                "/api/order/cancel",
                { core: order.core, uid: order.uid }
            );
        });
        panic.addEventListener("click", function () {
            if (commandBusy) return;
            var key = order.panic_armed ? "mini_panic_off_confirm" : "mini_panic_confirm";
            runCommand(
                withCoin(tr(key), order.coin),
                "/api/panic",
                { core: order.core, market: order.market, on: !order.panic_armed }
            );
        });
        line.appendChild(cancel);
        line.appendChild(panic);
        row.appendChild(line);
    }

    // Dense terminal row: coin, side, change and PnL; muted flow line; compact actions at the right.
    function orderRow(order, data) {
        var row = el("div", "row order-row");
        var main = el("div", "order-main");
        var top = el("div", "order-top");
        var coin = el("span", "name order-coin", order.coin || "");
        coin.title = order.coin || "";
        top.appendChild(coin);
        top.appendChild(el("span", "badge order-side " + sideClass(order.side), sideLabel(order.side)));
        top.appendChild(orderChange(order));
        top.appendChild(orderResult(order));
        main.appendChild(top);
        var flow = el("div", "sub order-flow");
        flow.appendChild(mutedBits(tr("mini_orders_qty"), order.qty_text));
        flow.appendChild(document.createTextNode(" · "));
        flow.appendChild(mutedBits(tr("mini_orders_entry"), order.entry_text));
        flow.appendChild(document.createTextNode(" → "));
        flow.appendChild(mutedBits(tr("mini_orders_mark"), order.mark_text));
        main.appendChild(flow);
        row.appendChild(main);
        if (data.can_control) orderActions(row, order);
        return row;
    }

    // One row per day, oldest first, the same figures the chat report's day table shows.
    function dayTable(data, days) {
        var card = el("div", "card day-card");
        card.appendChild(el("h2", "card-title", tr("report_days")));
        if (data.from_text && data.to_text) {
            card.appendChild(el("p", "day-window num", data.from_text + " — " + data.to_text));
        }
        var table = el("table", "day-table");
        var head = el("tr", "");
        head.appendChild(el("th", "", tr("report_date")));
        head.appendChild(el("th", "right", "USDT"));
        head.appendChild(el("th", "right", tr("report_trades")));
        var thead = el("thead", "");
        thead.appendChild(head);
        table.appendChild(thead);
        var body = el("tbody", "");
        var i;
        for (i = 0; i < days.length; i++) {
            var day = days[i] || {};
            var row = el("tr", "");
            row.appendChild(el("td", "num", day.start || ""));
            var money = el("td", "");
            applyMoney(money, "right num", day.text, day.usdt);
            row.appendChild(money);
            row.appendChild(el("td", "right num", String(typeof day.trades === "number" ? day.trades : 0)));
            body.appendChild(row);
        }
        table.appendChild(body);
        card.appendChild(table);
        return card;
    }

    function periodBar() {
        var bar = el("div", "segments");
        var i;
        for (i = 0; i < PERIODS.length; i++) {
            (function (value, key) {
                var button = document.createElement("button");
                button.type = "button";
                button.className = value === period ? "active" : "";
                button.textContent = tr(key);
                button.setAttribute("data-period", value);
                button.setAttribute("aria-pressed", value === period ? "true" : "false");
                button.addEventListener("click", function () {
                    if (value === period) return;
                    period = value;
                    reportExpanded = false;
                    hapticSelection();
                    paintPeriodPressed();
                    reloadPane("report", false);
                });
                bar.appendChild(button);
            })(PERIODS[i][0], PERIODS[i][1]);
        }
        return bar;
    }

    function paintPeriodPressed() {
        if (!reportPeriod) return;
        var nodes = reportPeriod.querySelectorAll("button");
        var i;
        for (i = 0; i < nodes.length; i++) {
            var on = nodes[i].getAttribute("data-period") === period;
            nodes[i].className = on ? "active" : "";
            nodes[i].setAttribute("aria-pressed", on ? "true" : "false");
        }
    }

    // Open | Closed switch of the merged trades tab; it sits outside both panes like the period bar.
    function dealsBar() {
        var bar = el("div", "segments");
        var segs = [["orders", "mini_deals_open"], ["trades", "mini_deals_closed"]];
        var i;
        for (i = 0; i < segs.length; i++) {
            (function (value, key) {
                var button = document.createElement("button");
                button.type = "button";
                button.textContent = tr(key);
                button.setAttribute("data-seg", value);
                button.addEventListener("click", function () {
                    selectTab(value);
                });
                bar.appendChild(button);
            })(segs[i][0], segs[i][1]);
        }
        return bar;
    }

    function paintDealsPressed() {
        var nodes = dealsSwitch.querySelectorAll("button");
        var i;
        for (i = 0; i < nodes.length; i++) {
            var on = nodes[i].getAttribute("data-seg") === dealsSegment;
            nodes[i].className = on ? "active" : "";
            nodes[i].setAttribute("aria-pressed", on ? "true" : "false");
        }
    }

    // The report period bar sits outside this node, so loading and errors do not remove it.
    function paneBody(name) {
        if (name === "report" && reportBody) return reportBody;
        return sections[name];
    }

    function moneySize(row) {
        var usdt = row && row.money && row.money.usdt;
        return typeof usdt === "number" && usdt === usdt ? Math.abs(usdt) : -1;
    }

    // Largest amount first, unvalued rows last. The source order breaks ties.
    function sortedByMoney(rows) {
        var copy = rows.map(function (row, index) { return { row: row, index: index }; });
        copy.sort(function (a, b) {
            return moneySize(b.row) - moneySize(a.row) || a.index - b.index;
        });
        return copy.map(function (item) { return item.row; });
    }

    // opts: nameClass, sort (by |usdt|), limit (rows before "show all"), meta (row -> text).
    function appendMoneyList(host, label, rows, opts) {
        if (!Array.isArray(rows) || !rows.length) return;
        opts = opts || {};
        var list = opts.sort ? sortedByMoney(rows) : rows;
        var cut = opts.limit && !reportExpanded && list.length > opts.limit ? opts.limit : list.length;
        var card = el("div", "card");
        card.appendChild(el("h2", "card-title", label));
        var i;
        var lastSection = null;
        for (i = 0; i < cut; i++) {
            var row = list[i];
            // Per-core rows carry their exchange section and arrive grouped by it.
            if (typeof row.section === "string" && row.section !== lastSection) {
                lastSection = row.section;
                card.appendChild(el("div", "row section-row", row.section));
            }
            var line = el("div", "row spread");
            var left = el("div", "grow");
            var name = el("div", opts.nameClass || "name", row.name || row.key || "");
            name.title = row.name || row.key || "";
            left.appendChild(name);
            var metaText = typeof opts.meta === "function" ? opts.meta(row) : "";
            if (metaText) left.appendChild(el("div", "row-meta", metaText));
            line.appendChild(left);
            var money = row.money || {};
            var val = el("span", "");
            applyMoney(val, "num money-col", money.text, money.usdt);
            line.appendChild(val);
            card.appendChild(line);
        }
        if (cut < list.length) {
            card.appendChild(button("show-more", tr("mini_show_all").replace("{n}", String(list.length)), function () {
                reportExpanded = true;
                hapticSelection();
                paintReport();
            }));
        }
        host.appendChild(card);
    }

    // "2026-09-27" -> "27.09", plus ".26" when withYear; anything else is shown as sent.
    function shortDate(text, withYear) {
        var m = /^(\d{4})-(\d{2})-(\d{2})/.exec(text || "");
        if (!m) return text || "";
        return m[3] + "." + m[2] + (withYear ? "." + m[1].slice(2) : "");
    }

    function yearOf(text) {
        var m = /^(\d{4})-/.exec(text || "");
        return m ? m[1] : "";
    }

    // The year is shown when the range spans two years or is not the current one.
    function reportRange(data) {
        var fromYear = yearOf(data.from);
        var toYear = yearOf(data.to);
        var thisYear = String(new Date().getFullYear());
        var withYear = !!((fromYear && toYear && fromYear !== toYear)
            || (fromYear && fromYear !== thisYear) || (toYear && toYear !== thisYear));
        var from = shortDate(data.from, withYear);
        var to = shortDate(data.to, withYear);
        return !to || from === to ? from : from + "\u2013" + to;
    }

    function coreOrdersMeta(row) {
        var n = row.money && row.money.orders;
        return typeof n === "number" ? tr("mini_report_core_orders").replace("{n}", String(n)) : "";
    }

    // Order count plus the client sum of every known PnL, same rounding as the group heads.
    function ordersSummary(orders) {
        var line = el("p", "summary spread");
        line.appendChild(el("span", "", tr("mini_orders_summary").replace("{n}", String(orders.length))));
        var sum = 0;
        var seen = 0;
        var i;
        for (i = 0; i < orders.length; i++) {
            var pnl = orders[i] && orders[i].pnl;
            if (typeof pnl !== "number" || pnl !== pnl || pnl === Infinity || pnl === -Infinity) continue;
            seen += 1;
            sum += pnl;
        }
        var text = seen ? signedFixed(sum) : null;
        if (text != null) {
            var fig = el("span", "");
            applyMoney(fig, "num money-col", text, sum);
            line.appendChild(fig);
        }
        return line;
    }

    // One text button that opens or closes every group of the pane at once.
    function appendToggleAll(host, pane, items, keyFn, query, repaint) {
        if (query) return;
        var groups = groupBy(items, function (item) { return groupKeyOf(item, keyFn); });
        if (groups.length < 2) return;
        var anyOpen = false;
        var g;
        for (g = 0; g < groups.length; g++) {
            if (!collapse[pane]["k:" + groups[g].key]) anyOpen = true;
        }
        var toggle = button("text-btn", tr(anyOpen ? "mini_collapse_all" : "mini_expand_all"), function () {
            var i;
            for (i = 0; i < groups.length; i++) {
                var slot = "k:" + groups[i].key;
                collapse[pane][slot] = anyOpen;
                collapseUser[pane][slot] = true;
            }
            hapticSelection();
            repaint();
        });
        // The toggle rides the line above it instead of taking a row of its own: beside the
        // search field when there is one, else at the end of the pane's summary line.
        var prev = host.lastElementChild;
        if (prev && prev.classList.contains("search")) {
            var row = el("div", "tool-row");
            host.replaceChild(row, prev);
            row.appendChild(prev);
            row.appendChild(toggle);
        } else if (prev && (prev.classList.contains("stat-strip") || prev.classList.contains("summary"))) {
            toggle.className = "text-btn inline";
            prev.appendChild(toggle);
        } else {
            toggle.className = "text-btn standalone";
            host.appendChild(toggle);
        }
    }

    function paintReport() {
        var host = paneBody("report");
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.report || {};
        var total = data.total || {};
        var orders = typeof total.orders === "number" ? total.orders : 0;
        if (!orders) {
            host.appendChild(emptyState(tr("mini_empty_report"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        var hero = el("div", "card hero");
        hero.appendChild(el("p", "label", tr("mini_report_total")));
        var big = el("p", "");
        applyMoney(big, "hero-value num", total.text, total.usdt);
        hero.appendChild(big);
        var bits = [];
        var range = reportRange(data);
        if (range) bits.push(range);
        bits.push(tr("mini_report_orders") + ": " + orders);
        if (total.unknown_orders > 0) {
            bits.push(tr("mini_report_unvalued_n").replace("{n}", String(total.unknown_orders)));
        }
        hero.appendChild(el("p", "hero-sub num", bits.join(" \u00B7 ")));
        host.appendChild(hero);
        var days = Array.isArray(data.days) ? data.days : [];
        if (days.length) host.appendChild(dayTable(data, days));
        appendMoneyList(host, tr("mini_report_by_exchange"), data.by_exchange, { sort: true });
        appendMoneyList(host, tr("mini_report_by_core"), data.by_core, {
            nameClass: "name core-name",
            limit: MONEY_LIST_LIMIT,
            meta: coreOrdersMeta
        });
        restoreSnap(snap, y);
    }

    // Owner-only bar over every visible core; each action asks one confirm.
    function massActions(cores) {
        var ids = [];
        var i;
        for (i = 0; i < cores.length; i++) ids.push(cores[i].id);
        var card = el("div", "card mass-actions");
        card.setAttribute("role", "group");
        card.setAttribute("aria-label", tr("mini_all_cores"));
        card.appendChild(el("span", "mass-title", tr("mini_all_cores")));
        function addPair(label, key, onConfirm, offConfirm) {
            var group = el("div", "mass-group");
            group.appendChild(el("span", "mass-label", label));
            var pair = el("span", "mass-pair");
            group.appendChild(pair);
            [true, false].forEach(function (on) {
                var btn = document.createElement("button");
                btn.type = "button";
                btn.className = "cmd chip mass-btn" + (on ? "" : " cmd-danger");
                btn.textContent = tr(on ? "mini_start" : "mini_stop");
                btn.setAttribute("aria-label", label + " " + btn.textContent);
                btn.disabled = commandBusy || !ids.length;
                btn.addEventListener("click", function () {
                    if (commandBusy || !ids.length) return;
                    var ask = trf(on ? onConfirm : offConfirm, { n: ids.length });
                    runCommand(ask, "/api/cores/switch", { cores: ids, switch: key, on: on });
                });
                pair.appendChild(btn);
            });
            card.appendChild(group);
        }
        addPair(tr("mini_trading"), "trading", "mini_cores_trading_on_confirm", "mini_cores_trading_off_confirm");
        addPair(tr("mini_autodetect"), "auto_detect", "mini_cores_auto_on_confirm", "mini_cores_auto_off_confirm");
        return card;
    }

    function paintCores() {
        var host = sections.cores;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.cores || {};
        var cores = Array.isArray(data.cores) ? data.cores : [];
        if (coreDetailId != null) {
            var shownCore = findCore(cores, coreDetailId);
            if (shownCore) {
                paintCoreDetail(host, shownCore, data);
                restoreSnap(snap, y);
                return;
            }
            // The core left the list (grant or config change): back to the list at its
            // old scroll; restoreSnap below scrolls there and syncs the Back button.
            coreDetailId = null;
            y = coreListY;
        }
        var online = 0;
        var problems = 0;
        var i;
        for (i = 0; i < cores.length; i++) {
            if (cores[i].conn === "ready") online += 1;
            if (coreProblem(cores[i])) problems += 1;
        }
        if (!cores.length) {
            host.appendChild(emptyState(tr("mini_empty_cores"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        var strip = el("div", "stat-strip");
        strip.setAttribute("role", "group");
        strip.setAttribute(
            "aria-label",
            tr("mini_cores_online").replace("{online}", String(online)).replace("{total}", String(cores.length))
        );
        strip.appendChild(el("span", "stat-value num", online + "/" + cores.length));
        strip.appendChild(el("span", "stat-label", tr("mini_online")));
        if (problems > 0) {
            strip.appendChild(el("span", "neg", tr("mini_cores_problems").replace("{n}", String(problems))));
        }
        host.appendChild(strip);
        // Exchange groups start open; a header tap still folds one.
        if (data.can_control) host.appendChild(massActions(cores));
        appendGroups(
            host, "cores", cores, coreProblem,
            function (core) { return coreRow(core, data); },
            "", paintCores, null, null, null, coreGroupSummary
        );
        restoreSnap(snap, y);
    }

    function paintBalances() {
        var host = sections.balances;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.balances || {};
        var perCore = Array.isArray(data.per_core) ? data.per_core : [];
        var perExchange = Array.isArray(data.per_exchange) ? data.per_exchange : [];
        if (!perCore.length && !perExchange.length) {
            host.appendChild(emptyState(tr("mini_empty_balances"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        var hero = el("div", "card hero hero-toggle");
        hero.setAttribute("role", "button");
        hero.tabIndex = 0;
        hero.appendChild(el("p", "label", tr("mini_total")));
        var big = el("p", "");
        function paintTotal() {
            if (balanceRevealed) applyMoney(big, "hero-value num", data.total_text, data.total);
            else {
                big.className = "hero-value num masked";
                big.textContent = "******";
            }
            hero.setAttribute("aria-pressed", balanceRevealed ? "true" : "false");
        }
        function toggleTotal() {
            balanceRevealed = !balanceRevealed;
            hapticSelection();
            paintTotal();
        }
        hero.addEventListener("click", function (event) {
            if (event.target && event.target.closest && event.target.closest("button, a")) return;
            toggleTotal();
        });
        hero.addEventListener("keydown", function (event) {
            if (event.target !== hero) return;
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            toggleTotal();
        });
        paintTotal();
        hero.appendChild(big);
        if (data.stale > 0 || data.excluded > 0) {
            var meta = el("div", "hero-sub badges");
            if (data.stale > 0) meta.appendChild(el("span", "badge badge-bad", tr("mini_balance_stale")));
            if (data.excluded > 0) {
                meta.appendChild(el("span", "", tr("mini_partial").replace("{n}", String(data.excluded))));
            }
            hero.appendChild(meta);
        }
        host.appendChild(hero);
        // Exchange groups start open; a header tap still folds one.
        if (perCore.length) {
            appendGroups(
                host,
                "balances",
                perCore,
                balanceProblem,
                balanceRow,
                "",
                paintBalances,
                null,
                null,
                null,
                balanceGroupSummary
            );
        }
        restoreSnap(snap, y);
    }

    function paintOrders() {
        var host = sections.orders;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.orders || {};
        var orders = Array.isArray(data.orders) ? data.orders : [];
        if (!orders.length) {
            host.appendChild(emptyState(tr("mini_empty_orders"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        host.appendChild(ordersSummary(orders));
        var orderQuery = orders.length > 10 ? queries.orders : "";
        if (orders.length > 10) {
            host.appendChild(searchField("search-orders", orderQuery, function (value) {
                queries.orders = value;
                paintOrders();
            }));
        }
        var filtered = filterItems(orders, orderQuery, function (order) {
            return [order.coin, order.core_name, order.market, sideLabel(order.side)];
        });
        if (orderQuery && !filtered.length) {
            host.appendChild(emptyState(tr("mini_empty_search"), clearSearchAction("orders", paintOrders)));
            restoreSnap(snap, y);
            return;
        }
        ensureCollapse("orders", orders, orderCoreKey);
        appendToggleAll(host, "orders", orders, orderCoreKey, orderQuery, paintOrders);
        // Core groups arrive in exchange sections; each section gets the terminal's caption.
        var sectionsOf = groupBy(filtered, function (order) { return String(order.exchange || ""); });
        var s;
        for (s = 0; s < sectionsOf.length; s++) {
            if (sectionsOf[s].key) host.appendChild(el("h2", "section-label", sectionsOf[s].key));
            appendGroups(
                host,
                "orders",
                sectionsOf[s].items,
                orderIsProblem,
                function (order) { return orderRow(order, data); },
                orderQuery,
                paintOrders,
                orderCoreKey,
                orderCoreLabel,
                "name grow core-name",
                orderGroupSummary
            );
        }
        restoreSnap(snap, y);
    }

    function findCore(cores, id) {
        var i;
        for (i = 0; i < cores.length; i++) {
            if (cores[i] && cores[i].id === id) return cores[i];
        }
        return null;
    }

    function openCoreDetail(id) {
        if (coreDetailId === id) return;
        hapticSelection();
        coreListY = window.pageYOffset || 0;
        coreDetailId = id;
        paintCores();
        window.scrollTo(0, 0);
        syncBackButton();
    }

    function closeCoreDetail() {
        if (coreDetailId == null) return;
        coreDetailId = null;
        if (current === "cores") {
            paintCores();
            window.scrollTo(0, coreListY);
        }
        syncBackButton();
    }

    function detailLine(card, label, value) {
        var line = el("div", "row spread");
        line.appendChild(el("span", "k", label));
        var shown = value == null || value === "" ? "—" : value;
        line.appendChild(el("span", "num", shown));
        card.appendChild(line);
    }

    function unitText(value, unitKey) {
        if (typeof value !== "number") return null;
        return String(value) + " " + tr(unitKey);
    }

    // One core in full: state, build, memory, CPU and pings; the owner also gets its controls.
    // One switch row of the detail screen: label left, ON / OFF pill right.
    function detailSwitch(core, label, field, key) {
        var line = el("div", "row spread detail-switch");
        line.appendChild(el("span", "grow", label));
        var state = core[field];
        var known = state === true || state === false;
        var pill = onOffPill(known ? state : null, true, false);
        pill.setAttribute("aria-label", label);
        pill.addEventListener("click", function () {
            if (commandBusy || !known) return;
            runCommand(null, "/api/core/switch", { core: core.id, switch: key, on: !state });
        });
        line.appendChild(pill);
        return line;
    }

    // Owner controls of the detail screen: two switch rows, then full-width actions.
    function detailControls(core) {
        var controls = el("div", "card detail-controls");
        controls.appendChild(detailSwitch(core, tr("mini_trading"), "trading", "trading"));
        controls.appendChild(detailSwitch(core, tr("mini_autodetect"), "auto_detect", "auto_detect"));
        var line = el("div", "detail-actions");
        var cancel = button("cmd action-btn cmd-danger", tr("mini_cancel_all"), function () {
            if (commandBusy) return;
            runCommand(null, "/api/core/cancel_all", { core: core.id });
        });
        cancel.disabled = commandBusy;
        line.appendChild(cancel);
        var reconnect = button("cmd action-btn", tr("mini_reconnect"), function () {
            if (commandBusy) return;
            runCommand(null, "/api/core/reconnect", { core: core.id });
        });
        reconnect.disabled = commandBusy;
        line.appendChild(reconnect);
        controls.appendChild(line);
        return controls;
    }

    function paintCoreDetail(host, core, data) {
        var wrap = el("div", "core-detail");
        wrap.appendChild(button("cmd text-btn detail-back", "‹ " + tr("mini_back"), closeCoreDetail));
        var head = el("div", "card");
        var top = el("div", "row detail-head");
        top.appendChild(el("span", dotClass(core)));
        var body = el("div", "grow");
        body.appendChild(el("div", "name core-name", core.name || ""));
        var state = core.fault ? faultLabel(core.fault) : tr("mini_core_" + (core.conn || ""));
        var sub = [];
        if (core.exchange) sub.push(core.exchange);
        if (state) sub.push(state);
        if (sub.length) body.appendChild(el("div", core.fault ? "sub neg" : "sub", sub.join(" · ")));
        top.appendChild(body);
        head.appendChild(top);
        wrap.appendChild(head);
        var facts = el("div", "card");
        detailLine(facts, tr("mini_version"), core.version);
        detailLine(facts, tr("mini_memory"), unitText(core.mem_mb, "mini_unit_mb"));
        detailLine(facts, tr("mini_free_memory"), unitText(core.free_mem_mb, "mini_unit_mb"));
        var cpu = typeof core.cpu_proc === "number" ? core.cpu_proc : core.cpu_sys;
        detailLine(facts, tr("mini_cpu"), typeof cpu === "number" ? unitText(Math.round(cpu * 10) / 10, "mini_unit_pct") : null);
        detailLine(facts, tr("mini_ping"), unitText(core.ping_ms, "mini_unit_ms"));
        detailLine(facts, tr("mini_exch_ping"), unitText(core.exch_ping_ms, "mini_unit_ms"));
        wrap.appendChild(facts);
        if (data && data.can_control) {
            wrap.appendChild(detailControls(core));
        }
        host.appendChild(wrap);
    }

    // Days and hours, hours and minutes, or minutes and seconds; every unit comes from the label map.
    function fmtDuration(secs) {
        if (typeof secs !== "number" || secs !== secs || secs < 0) return null;
        var s = Math.floor(secs);
        var d = Math.floor(s / 86400);
        var h = Math.floor((s % 86400) / 3600);
        var m = Math.floor((s % 3600) / 60);
        if (d > 0) return trf("mini_duration_dh", { d: d, h: h });
        if (h > 0) return trf("mini_duration_hm", { h: h, m: m });
        return trf("mini_duration_ms", { m: m, s: s % 60 });
    }

    function tradePct(trade, baseClass) {
        var node = el("span", baseClass);
        if (trade.profit_pct_text) {
            var tone = signClass(trade.profit_pct);
            node.className = baseClass + (tone ? " " + tone : "");
            node.textContent = trade.profit_pct_text;
        } else {
            node.className = baseClass + " hint";
            node.textContent = "—";
        }
        return node;
    }

    function tradeRow(trade) {
        var row = el("div", "row trade-row");
        row.setAttribute("role", "button");
        row.tabIndex = 0;
        var left = el("div", "grow");
        var top = el("div", "order-top");
        var coin = el("span", "name order-coin", trade.coin || "");
        coin.title = trade.coin || "";
        top.appendChild(coin);
        top.appendChild(el("span", "badge order-side " + sideClass(trade.side), sideLabel(trade.side)));
        left.appendChild(top);
        var meta = el("div", "sub trade-row-meta");
        var when = trade.closed_short_text || trade.closed_text;
        if (trade.core_name) {
            var core = el("span", "trade-core", trade.core_name);
            core.title = trade.core_name;
            meta.appendChild(core);
        }
        if (trade.core_name && when) meta.appendChild(el("span", "trade-sep", " · "));
        if (when) meta.appendChild(el("span", "trade-when", when));
        left.appendChild(meta);
        row.appendChild(left);
        var right = el("div", "trade-result");
        var profit = el("span", "");
        applyMoney(profit, "num money-col", trade.profit_text, trade.profit);
        right.appendChild(profit);
        right.appendChild(tradePct(trade, "num fine"));
        row.appendChild(right);
        function open() {
            openTradeSheet(trade);
        }
        row.addEventListener("click", open);
        row.addEventListener("keydown", function (event) {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            open();
        });
        return row;
    }

    function paintTrades() {
        var host = sections.trades;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.trades || {};
        var trades = Array.isArray(data.trades) ? data.trades : [];
        if (!trades.length) {
            host.appendChild(emptyState(tr("mini_empty_trades"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        // The count shown, not the backend's cap.
        host.appendChild(el("p", "list-caption", trf("mini_trades_shown_" + pluralForm(trades.length), { n: trades.length })));
        var card = el("div", "card");
        var i;
        for (i = 0; i < trades.length; i++) card.appendChild(tradeRow(trades[i]));
        host.appendChild(card);
        restoreSnap(snap, y);
    }

    function openTradeSheet(trade) {
        if (!sheet) return;
        hapticSelection();
        clear(sheet);
        var head = el("div", "sheet-head");
        var titleBox = el("div", "grow");
        var top = el("div", "order-top");
        var title = el("span", "name order-coin", trade.coin || "");
        title.id = "sheet-title";
        top.appendChild(title);
        top.appendChild(el("span", "badge order-side " + sideClass(trade.side), sideLabel(trade.side)));
        titleBox.appendChild(top);
        var where = [];
        if (trade.core_name) where.push(trade.core_name);
        if (trade.exchange) where.push(trade.exchange);
        titleBox.appendChild(el("div", "sub trade-meta", where.join(" · ")));
        head.appendChild(titleBox);
        var closeBtn = button("cmd text-btn sheet-close", tr("mini_close"), closeSheet);
        head.appendChild(closeBtn);
        sheet.appendChild(head);
        var result = el("div", "row spread sheet-result");
        var profit = el("span", "");
        applyMoney(profit, "hero-value num", trade.profit_text, trade.profit);
        result.appendChild(profit);
        result.appendChild(tradePct(trade, "num"));
        sheet.appendChild(result);
        detailLine(sheet, tr("mini_trade_entry"), trade.entry_text);
        detailLine(sheet, tr("mini_trade_exit"), trade.exit_text);
        detailLine(sheet, tr("mini_trade_qty"), trade.qty_text);
        detailLine(sheet, tr("mini_trade_duration"), fmtDuration(trade.duration_secs));
        detailLine(sheet, tr("mini_trade_strategy"), trade.strategy || tr("mini_trade_manual"));
        detailLine(sheet, tr("mini_trade_closed"), trade.closed_text);
        if (!sheetBackdrop) {
            sheetBackdrop = el("div", "sheet-backdrop");
            sheetBackdrop.addEventListener("click", closeSheet);
            document.body.appendChild(sheetBackdrop);
        }
        sheetBackdrop.hidden = false;
        sheet.setAttribute("aria-labelledby", "sheet-title");
        sheet.hidden = false;
        sheetOpen = true;
        document.body.style.overflow = "hidden";
        closeBtn.focus();
        syncBackButton();
    }

    function closeSheet() {
        if (!sheetOpen) return;
        sheetOpen = false;
        if (sheet) {
            sheet.hidden = true;
            clear(sheet);
        }
        if (sheetBackdrop) sheetBackdrop.hidden = true;
        document.body.style.overflow = "";
        syncBackButton();
    }

    document.addEventListener("keydown", function (event) {
        if (sheetOpen && event.key === "Escape") closeSheet();
    });

    function strategyCoreKey(core) {
        return "id:" + String(core.core);
    }

    function strategyCoreLabel(core) {
        return core.core_name || "";
    }

    function strategiesOf(core) {
        var out = [];
        var folders = Array.isArray(core.folders) ? core.folders : [];
        var i;
        var j;
        for (i = 0; i < folders.length; i++) {
            var list = Array.isArray(folders[i].strategies) ? folders[i].strategies : [];
            for (j = 0; j < list.length; j++) out.push(list[j]);
        }
        return out;
    }

    function strategyCoreProblem(core) {
        var list = strategiesOf(core);
        var i;
        for (i = 0; i < list.length; i++) {
            if (list[i].pending === "timed_out") return true;
        }
        return false;
    }

    // Core header: strategies switched on out of all of them.
    function strategyGroupSummary(items) {
        var on = 0;
        var total = 0;
        var i;
        var j;
        for (i = 0; i < items.length; i++) {
            var list = strategiesOf(items[i]);
            for (j = 0; j < list.length; j++) {
                total += 1;
                if (list[j].checked) on += 1;
            }
        }
        return el("span", "count num", on + "/" + total);
    }

    // The app's one on/off look: an ON / OFF pill (labels stay untranslated). `state` null is
    // unknown; a pending pill waits for its core and is never tappable.
    function onOffPill(state, interactive, pending) {
        var known = state === true || state === false;
        var cls = "pill " + (known ? (state ? "on" : "off") : "unknown") + (pending ? " pending" : "");
        var pill = document.createElement(interactive ? "button" : "span");
        pill.className = interactive ? "cmd " + cls : cls;
        pill.textContent = known ? (state ? "ON" : "OFF") : "?";
        if (!interactive) return pill;
        pill.type = "button";
        pill.setAttribute("aria-pressed", known ? String(state) : "mixed");
        pill.disabled = commandBusy || !known || !!pending;
        return pill;
    }

    // The pill is the core's confirmed state; the sub line says when a change is unconfirmed.
    function strategyRow(core, strategy, canControl) {
        var row = el("div", "row spread strategy-row");
        var body = el("div", "grow");
        var name = el("div", "name core-name", strategy.name || "");
        name.title = strategy.name || "";
        body.appendChild(name);
        if (strategy.pending === "pending") {
            body.appendChild(el("div", "sub", tr("mini_strategy_pending")));
        } else if (strategy.pending === "timed_out") {
            body.appendChild(el("div", "sub neg", tr("mini_strategy_timed_out")));
        }
        row.appendChild(body);
        var on = strategy.checked === true;
        var waiting = strategy.pending === "pending";
        if (!canControl) {
            var shown = onOffPill(on, false, waiting);
            shown.setAttribute("aria-label", strategy.name || "");
            row.appendChild(shown);
            return row;
        }
        var chip = onOffPill(on, true, waiting);
        chip.setAttribute("aria-label", strategy.name || "");
        chip.addEventListener("click", function () {
            if (commandBusy || waiting) return;
            runCommand(null, "/api/strategy/toggle", { core: core.core, id: strategy.id, on: !on });
        });
        row.appendChild(chip);
        return row;
    }

    function strategyCoreBody(core, canControl) {
        var box = el("div", "strategy-core");
        var folders = Array.isArray(core.folders) ? core.folders : [];
        var i;
        var j;
        for (i = 0; i < folders.length; i++) {
            var folder = folders[i];
            var list = Array.isArray(folder.strategies) ? folder.strategies : [];
            if (!list.length) continue;
            var folderRow = el("div", "row section-row folder-row", folder.path ? folder.path : tr("mini_strategy_root"));
            if (folder.path) folderRow.title = folder.path;
            box.appendChild(folderRow);
            for (j = 0; j < list.length; j++) box.appendChild(strategyRow(core, list[j], canControl));
        }
        return box;
    }

    function paintStrategies() {
        var host = sections.strategies;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.strategies || {};
        var cores = Array.isArray(data.cores) ? data.cores : [];
        var withRows = [];
        var i;
        for (i = 0; i < cores.length; i++) {
            if (strategiesOf(cores[i]).length) withRows.push(cores[i]);
        }
        if (!withRows.length) {
            host.appendChild(emptyState(tr("mini_empty_strategies"), refreshAction()));
            restoreSnap(snap, y);
            return;
        }
        // A core carries dozens of strategies, so its group starts folded.
        ensureCollapse("strategies", withRows, strategyCoreKey);
        var sectionsOf = groupBy(withRows, function (core) { return String(core.exchange || ""); });
        var s;
        for (s = 0; s < sectionsOf.length; s++) {
            if (sectionsOf[s].key) host.appendChild(el("h2", "section-label", sectionsOf[s].key));
            appendGroups(
                host,
                "strategies",
                sectionsOf[s].items,
                strategyCoreProblem,
                function (core) { return strategyCoreBody(core, !!data.can_control); },
                "",
                paintStrategies,
                strategyCoreKey,
                strategyCoreLabel,
                "name grow core-name",
                strategyGroupSummary
            );
        }
        restoreSnap(snap, y);
    }

    var paint = {
        report: paintReport,
        cores: paintCores,
        balances: paintBalances,
        orders: paintOrders,
        trades: paintTrades,
        strategies: paintStrategies
    };

    var lastClock = "";

    function twoDigits(value) {
        return (value < 10 ? "0" : "") + value;
    }

    // Fixed clock is 24-hour HH:MM:SS. The word stays when the header is wider
    // than 300px and is dropped at 300px and under, so the title row never wraps.
    function clockNow() {
        var now = new Date();
        return twoDigits(now.getHours()) + ":" + twoDigits(now.getMinutes()) + ":" + twoDigits(now.getSeconds());
    }

    function headerIsNarrow() {
        return !!(header && header.clientWidth > 0 && header.clientWidth <= 300);
    }

    function staleAfter(name) {
        return name === "report" || name === "trades" ? REPORT_POLL_MS * 2 : Math.max(POLL_MS * 5, 15000);
    }

    // Relative age of the current tab's last good read; the fixed clock rides the tooltip.
    function paintUpdated() {
        if (!current || !lastOk[current]) {
            updated.hidden = true;
            return;
        }
        var age = Date.now() - lastOk[current];
        var secs = Math.max(0, Math.floor(age / 1000));
        var rel;
        if (secs < 5) rel = tr("mini_updated_now");
        else if (secs < 60) rel = tr("mini_updated_secs").replace("{n}", String(secs));
        else rel = tr("mini_updated_mins").replace("{n}", String(Math.floor(secs / 60)));
        var word = tr("mini_updated");
        var bare = secs < 5 || headerIsNarrow() || !word;
        updated.textContent = bare ? rel : word + " " + rel;
        updated.title = lastClock;
        updated.className = age > staleAfter(current) ? "updated updated-stale" : "updated";
        updated.hidden = false;
    }

    function startUpdatedTimer() {
        if (updatedTimer || document.visibilityState !== "visible") return;
        updatedTimer = setInterval(paintUpdated, 1000);
    }

    function stopUpdatedTimer() {
        if (!updatedTimer) return;
        clearInterval(updatedTimer);
        updatedTimer = null;
    }

    function markUpdated(name) {
        lastClock = clockNow();
        lastOk[name] = Date.now();
        paintUpdated();
    }

    function setSpinning(on) {
        if (!refreshButton) return;
        if (on) refreshButton.classList.add("spinning");
        else refreshButton.classList.remove("spinning");
        refreshButton.disabled = !!on;
    }

    function refreshCurrent() {
        if (!current || inFlight[current]) return;
        hapticImpact("light");
        setSpinning(true);
        reloadPane(current, !!hasData[current]);
    }

    function loadTab(name, token, silent) {
        if (name !== current || token !== loadToken) return;
        clearPoll();
        if (!silent) showLoading(paneBody(name));
        var ticket = {};
        inFlight[name] = ticket;
        if (refreshButton) refreshButton.disabled = true;
        api(pathFor(name), bodyFor(name)).then(function (res) {
            if (inFlight[name] === ticket) {
                inFlight[name] = null;
                setSpinning(false);
                // A read still running for the shown tab keeps the button busy.
                if (refreshButton && current && inFlight[current]) refreshButton.disabled = true;
            }
            if (res.cancelled || token !== loadToken || name !== current) return;
            if (!res.ok || !res.data) {
                // A failed background read keeps the last good data on screen.
                if (silent && hasData[name] && res.status !== 401 && res.status !== 403) {
                    showStale(res.error, !!res.retry);
                    schedulePoll(name, token);
                    return;
                }
                hasData[name] = false;
                lastRaw[name] = null;
                clearStale();
                showError(paneBody(name), res.error, !!res.retry);
                return;
            }
            clearStale();
            markUpdated(name);
            var same = !!(hasData[name] && res.raw && res.raw === lastRaw[name]);
            hasData[name] = true;
            payloads[name] = res.data;
            lastRaw[name] = res.raw || null;
            if (!same) paint[name]();
            schedulePoll(name, token);
        });
    }

    function reloadPane(name, silent) {
        if (name !== current) return;
        cancelPending();
        loadToken += 1;
        if (!silent) hasData[name] = false;
        loadTab(name, loadToken, silent);
    }

    function selectTab(name) {
        if (!sections[name] || name === current) return;
        if (current) hapticSelection();
        showCmdLine("");
        clearStale();
        setSpinning(false);
        cancelPending();
        current = name;
        closeSheet();
        coreDetailId = null;
        if (name === "balances") balanceRevealed = false;
        loadToken += 1;
        var i;
        for (i = 0; i < TAB_NAMES.length; i++) {
            var tab = TAB_NAMES[i];
            sections[tab].hidden = tab !== name;
            buttons[tab].className = "";
            buttons[tab].removeAttribute("aria-current");
        }
        buttons[name].className = "active";
        buttons[name].setAttribute("aria-current", "page");
        var deals = name === "orders" || name === "trades";
        if (deals) dealsSegment = name;
        if (dealsSwitch) {
            dealsSwitch.hidden = !deals;
            paintDealsPressed();
        }
        paintUpdated();
        loadTab(name, loadToken, !!hasData[name]);
    }

    function onVisible() {
        if (!sessionOk || !current) return;
        if (document.visibilityState !== "visible") {
            clearPoll();
            stopUpdatedTimer();
            return;
        }
        paintUpdated();
        startUpdatedTimer();
        // A read still running reschedules the poll itself; a second one would only queue behind it.
        if (inFlight[current]) return;
        loadTab(current, loadToken, !!hasData[current]);
    }

    function showStartup(message, allowRetry) {
        clear(startup);
        startup.hidden = false;
        startup.appendChild(el("p", "", message));
        if (!allowRetry) return;
        startup.appendChild(button("retry", tr("mini_retry"), function () {
            startSession();
        }));
    }

    function startSession() {
        showStartup(tr("mini_shell_checking"), false);
        var skel = el("div", "startup-skel");
        skeleton(skel);
        startup.appendChild(skel);
        main.hidden = true;
        nav.hidden = true;
        api("/api/session", {}).then(function (res) {
            if (res.cancelled) return;
            if (res.ok && res.data && res.data.ok === true) {
                clear(startup);
                startup.hidden = true;
                main.hidden = false;
                nav.hidden = false;
                sessionOk = true;
                if (refreshButton) refreshButton.hidden = false;
                syncHeaderHeight();
                startUpdatedTimer();
                selectTab("report");
                return;
            }
            if (res.status === 403) {
                showStartup(tr("mini_shell_denied"), false);
                return;
            }
            var busy = res.status === 503 || res.status === 504;
            showStartup(busy ? (res.error || tr("mini_error_busy")) : tr("mini_shell_unreachable"), true);
        });
    }

    function bindChrome() {
        title.textContent = tr("mini_title");
        var nodes = document.querySelectorAll("#app-nav button[data-tab]");
        var i;
        for (i = 0; i < nodes.length; i++) {
            var button = nodes[i];
            var name = button.getAttribute("data-tab");
            if (name === "deals") {
                buttons.orders = button;
                buttons.trades = button;
            } else {
                buttons[name] = button;
            }
            var label = tr(TAB_KEYS[name]);
            var span = button.querySelector(".nav-label");
            if (span) span.textContent = label;
            button.setAttribute("aria-label", label);
            button.addEventListener("click", function (ev) {
                var tab = ev.currentTarget.getAttribute("data-tab");
                selectTab(tab === "deals" ? dealsSegment : tab);
            });
        }
        var found = document.querySelectorAll("#app-main section[data-tab]");
        for (i = 0; i < found.length; i++) {
            sections[found[i].getAttribute("data-tab")] = found[i];
        }
        reportPeriod = document.getElementById("report-period");
        reportBody = document.getElementById("report-body");
        if (reportPeriod) reportPeriod.appendChild(periodBar());
        dealsSwitch = document.getElementById("deals-switch");
        if (dealsSwitch) dealsSwitch.appendChild(dealsBar());
        window.addEventListener("resize", paintUpdated);
        bindRefresh();
        bindHeaderHeight();
        bindBackButton();
    }

    function bindRefresh() {
        if (!refreshButton) return;
        refreshButton.setAttribute("aria-label", tr("mini_refresh"));
        refreshButton.title = tr("mini_refresh");
        refreshButton.addEventListener("click", refreshCurrent);
    }

    // Sticky group heads sit under the header, so its height is published as --header-h.
    function syncHeaderHeight() {
        if (!header || !header.offsetHeight) return;
        document.documentElement.style.setProperty("--header-h", header.offsetHeight + "px");
    }

    // Without ResizeObserver, window resize and command-line changes resync the height by hand.
    var observesHeader = !!(header && typeof ResizeObserver === "function");

    function bindHeaderHeight() {
        if (observesHeader) new ResizeObserver(syncHeaderHeight).observe(header);
        else window.addEventListener("resize", syncHeaderHeight);
        if (webapp && typeof webapp.onEvent === "function") {
            webapp.onEvent("viewportChanged", syncHeaderHeight);
            webapp.onEvent("safeAreaChanged", syncHeaderHeight);
            webapp.onEvent("contentSafeAreaChanged", syncHeaderHeight);
            webapp.onEvent("fullscreenChanged", syncHeaderHeight);
        }
        syncHeaderHeight();
    }

    function withCoin(text, coin) {
        var name = coin == null ? "" : String(coin);
        return String(text || "").split("{coin}").join(name);
    }

    function askConfirm(text, done) {
        if (webapp && typeof webapp.showConfirm === "function") {
            webapp.showConfirm(text, function (ok) {
                done(ok === true);
            });
            return;
        }
        done(window.confirm(text) === true);
    }

    function hapticFeedback() {
        return webapp && webapp.HapticFeedback;
    }

    function hapticSelection() {
        var feedback = hapticFeedback();
        if (feedback && typeof feedback.selectionChanged === "function") {
            feedback.selectionChanged();
        }
    }

    // A sent money command is a medium impact; other taps pass their own style.
    function hapticImpact(style) {
        var feedback = hapticFeedback();
        if (!feedback || typeof feedback.impactOccurred !== "function") return;
        if (style) feedback.impactOccurred(style);
        else feedback.impactOccurred("medium");
    }

    function haptic(kind) {
        var feedback = hapticFeedback();
        if (!feedback || typeof feedback.notificationOccurred !== "function") return;
        if (kind === "success" || kind === "error") feedback.notificationOccurred(kind);
    }

    function hasQuery() {
        return !!queries.orders;
    }

    function anyGroupOpen() {
        var panes = ["cores", "balances", "orders", "strategies"];
        var p;
        var slot;
        for (p = 0; p < panes.length; p++) {
            var map = collapse[panes[p]];
            for (slot in map) {
                if (Object.prototype.hasOwnProperty.call(map, slot) && !map[slot]) return true;
            }
        }
        return false;
    }

    function collapseOpenGroups() {
        var panes = ["cores", "balances", "orders", "strategies"];
        var p;
        var slot;
        for (p = 0; p < panes.length; p++) {
            var pane = panes[p];
            var map = collapse[pane];
            for (slot in map) {
                if (!Object.prototype.hasOwnProperty.call(map, slot) || map[slot]) continue;
                map[slot] = true;
                collapseUser[pane][slot] = true;
            }
        }
    }

    function onBack() {
        if (sheetOpen) {
            closeSheet();
            return;
        }
        if (coreDetailId != null) {
            closeCoreDetail();
            return;
        }
        if (hasQuery()) {
            queries.orders = "";
        } else {
            collapseOpenGroups();
        }
        if (current && paint[current]) paint[current]();
        else syncBackButton();
    }

    function syncBackButton() {
        var button = webapp && webapp.BackButton;
        if (!button) return;
        var needed = sheetOpen || coreDetailId != null || hasQuery() || anyGroupOpen();
        if (typeof button.isVisible === "boolean" && button.isVisible === needed) return;
        if (needed && typeof button.show === "function") button.show();
        else if (!needed && typeof button.hide === "function") button.hide();
    }

    function bindBackButton() {
        var button = webapp && webapp.BackButton;
        if (!button) return;
        if (typeof button.onClick === "function") {
            button.onClick(onBack);
            return;
        }
        if (typeof webapp.onEvent === "function") webapp.onEvent("backButtonClicked", onBack);
    }

    function showCmdLine(text) {
        var node = document.getElementById("cmd-status");
        if (!node) {
            node = el("p", "cmd-status", "");
            node.id = "cmd-status";
            if (header) header.appendChild(node);
            else document.body.appendChild(node);
        }
        if (cmdTimer) {
            clearTimeout(cmdTimer);
            cmdTimer = null;
        }
        node.textContent = text || "";
        node.hidden = !text;
        if (!observesHeader) syncHeaderHeight();
        if (!text) return;
        cmdTimer = setTimeout(function () {
            cmdTimer = null;
            node.textContent = "";
            node.hidden = true;
            if (!observesHeader) syncHeaderHeight();
        }, 4000);
    }

    function setCommandsDisabled(disabled) {
        var nodes = document.querySelectorAll(".order-actions .cmd, .core-actions .cmd, .mass-actions .cmd, .strategy-row .cmd, .core-detail .cmd");
        var i;
        for (i = 0; i < nodes.length; i++) {
            // A chip with no known state, or a strategy awaiting its core, stays off whatever the busy flag says.
            var cls = nodes[i].className;
            nodes[i].disabled = !!disabled || cls.indexOf("unknown") >= 0 || cls.indexOf("pending") >= 0;
        }
    }

    // Cancelled, a dropped connection, or 504: the page cannot tell whether the command landed.
    function commandUnresolved(res) {
        return !!(res && (res.cancelled || res.status === 0 || res.status === 504));
    }

    // Reload the pane in view; a core switch or strategy toggle lands a moment later, so those read twice.
    function reloadAfterCommand(silent) {
        var pane = current;
        if (!pane) return;
        if (!silent) hasData[pane] = false;
        reloadPane(pane, silent);
        if (pane === "cores" || pane === "strategies") {
            setTimeout(function () {
                if (current === pane) reloadPane(pane, true);
            }, 1500);
        }
    }

    function finishCommand(path, res) {
        commandBusy = false;
        setCommandsDisabled(false);
        if (commandUnresolved(res)) {
            showCmdLine(tr("mini_cmd_unknown"));
            reloadAfterCommand(false);
            return;
        }
        if (!res.ok) {
            showCmdLine(res.error || tr("mini_error_read"));
            reloadAfterCommand(true);
            return;
        }
        var data = res.data || {};
        var partial = typeof data.sent === "number" && typeof data.requested === "number"
            && data.sent !== data.requested;
        // A reconnect only starts one; the next cores poll shows whether it came back.
        if (data.ok === true && path === "/api/core/reconnect") {
            showCmdLine("");
            reloadAfterCommand(true);
            return;
        }
        if (data.ok === true && !partial) {
            haptic("success");
            showCmdLine(tr("mini_cmd_sent"));
            reloadAfterCommand(true);
            return;
        }
        haptic("error");
        if (partial) {
            showCmdLine(trf("mini_cmd_partial", { sent: data.sent, n: data.requested }));
            reloadAfterCommand(true);
            return;
        }
        var failed = tr("mini_cmd_failed");
        if (data.error === "not_found") {
            failed = path.indexOf("/api/core") === 0 ? tr("mini_cmd_core_not_found") : tr("mini_cmd_not_found");
        }
        showCmdLine(failed);
        reloadAfterCommand(true);
    }

    // confirmText null fires at once; otherwise exactly one confirm.
    function runCommand(confirmText, path, body) {
        if (commandBusy) return;
        commandBusy = true;
        setCommandsDisabled(true);
        function send() {
            // A command must not wait behind a background read; the pane reloads after it anyway.
            cancelPending();
            hapticImpact();
            api(path, body, true).then(function (res) { finishCommand(path, res); });
        }
        if (confirmText == null) {
            send();
            return;
        }
        askConfirm(confirmText, function (ok) {
            if (!ok) {
                commandBusy = false;
                setCommandsDisabled(false);
                return;
            }
            send();
        });
    }

    bindChrome();
    document.addEventListener("visibilitychange", onVisible);
    startSession();
})();
