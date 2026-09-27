(function () {
    "use strict";

    var SVG = "http://www.w3.org/2000/svg";
    var POLL_MS = 2000;
    // The backend keeps a finished report for 15 s, so a faster report poll returns the same data.
    var REPORT_POLL_MS = 15000;
    var RETRY_MS = [1000, 2000, 4000];
    var REPORT_RETRY_LIMIT = 6;
    var REPORT_RETRY_MS = 2000;
    var TAB_NAMES = ["report", "cores", "balances", "orders"];
    var TAB_KEYS = {
        report: "mini_tab_report",
        cores: "mini_tab_cores",
        balances: "mini_tab_balances",
        orders: "mini_tab_orders"
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
    var buttons = {};
    var payloads = {};
    var queries = { cores: "", balances: "", orders: "" };
    var collapse = { cores: {}, balances: {}, orders: {} };
    var collapseUser = { cores: {}, balances: {}, orders: {} };
    var hasData = {};
    var queue = [];
    var busy = false;
    var currentJob = null;
    var pollTimer = null;
    var cmdTimer = null;
    var period = "today";
    var current = null;
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

    function tr(key) {
        var value = labels[key];
        return typeof value === "string" ? value : "";
    }

    if (labels.locale) {
        document.documentElement.lang = labels.locale;
    }

    function applyScheme() {
        var scheme = webapp && webapp.colorScheme;
        if (scheme === "dark" || scheme === "light") {
            document.documentElement.style.colorScheme = scheme;
        }
    }

    function applyChromeColors() {
        if (!webapp) return;
        if (webapp.setHeaderColor) webapp.setHeaderColor("secondary_bg_color");
        if (webapp.setBackgroundColor) webapp.setBackgroundColor("secondary_bg_color");
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
        fetch(job.path, opts).then(function (response) {
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
            if (job.cancelled) {
                finish(job, { ok: false, status: 0, cancelled: true, retry: false, error: "" });
                return;
            }
            finish(job, { ok: false, status: 0, error: tr("mini_error_network"), retry: true });
        });
    }

    function retryLimit(job) {
        return job.path === "/api/report" ? REPORT_RETRY_LIMIT : RETRY_MS.length;
    }

    function retryWait(job) {
        if (job.path === "/api/report") return REPORT_RETRY_MS;
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
        }, name === "report" ? REPORT_POLL_MS : POLL_MS);
    }

    function pathFor(name) {
        if (name === "report") return "/api/report";
        if (name === "cores") return "/api/cores";
        if (name === "balances") return "/api/balances";
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

    function problemsFirst(items, isProblem) {
        var bad = [];
        var good = [];
        var i;
        for (i = 0; i < items.length; i++) {
            if (isProblem(items[i])) bad.push(items[i]);
            else good.push(items[i]);
        }
        return bad.concat(good);
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

    // Every group starts collapsed, including on a later refresh. A group the
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
            var ordered = problemsFirst(group.items, isProblem);
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

    function coreRow(core) {
        var row = el("div", "row core-row");
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
        cancel.textContent = tr("mini_cancel");
        cancel.disabled = commandBusy;
        var panic = document.createElement("button");
        panic.type = "button";
        panic.className = "cmd cmd-danger";
        panic.textContent = tr(order.panic_armed ? "mini_panic_off" : "mini_panic_sell");
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

    function orderRow(order, data) {
        var row = el("div", "row order-row");
        var top = el("div", "order-top");
        var coin = el("span", "name", order.coin || "");
        coin.title = order.coin || "";
        top.appendChild(coin);
        top.appendChild(el("span", "badge order-side " + sideClass(order.side), order.side || ""));
        top.appendChild(orderChange(order));
        top.appendChild(orderResult(order));
        row.appendChild(top);
        var flow = el("div", "sub order-flow");
        flow.appendChild(mutedBits(tr("mini_orders_qty"), order.qty_text));
        flow.appendChild(document.createTextNode(" "));
        flow.appendChild(mutedBits(tr("mini_orders_entry"), order.entry_text));
        flow.appendChild(document.createTextNode(" \u2192 "));
        flow.appendChild(mutedBits(tr("mini_orders_mark"), order.mark_text));
        row.appendChild(flow);
        if (data.can_control) orderActions(row, order);
        return row;
    }

    // Bars share one viewBox so the card width scales them. Null days are a hint tick.
    function renderBars(days) {
        var wrap = el("div", "chart");
        var n = days.length;
        var slot = 10;
        var width = Math.max(n, 1) * slot;
        var height = 80;
        var pad = 2;
        var svg = svgEl("svg");
        svg.setAttribute("class", "bars");
        svg.setAttribute("viewBox", "0 0 " + width + " " + height);
        svg.setAttribute("preserveAspectRatio", "none");
        svg.setAttribute("role", "img");
        var min = 0;
        var max = 0;
        var i;
        for (i = 0; i < n; i++) {
            var sample = days[i] && days[i].usdt;
            if (typeof sample !== "number" || sample !== sample) continue;
            if (sample < min) min = sample;
            if (sample > max) max = sample;
        }
        if (min === 0 && max === 0) max = 1;
        var span = max - min;
        if (!span) span = 1;
        function yOf(v) {
            return height - pad - ((v - min) / span) * (height - pad * 2);
        }
        var baseline = yOf(0);
        var base = svgEl("line");
        base.setAttribute("class", "baseline");
        base.setAttribute("x1", "0");
        base.setAttribute("x2", String(width));
        base.setAttribute("y1", String(baseline));
        base.setAttribute("y2", String(baseline));
        svg.appendChild(base);
        for (i = 0; i < n; i++) {
            var day = days[i] || {};
            var x = i * slot;
            var amount = day.usdt;
            var known = typeof amount === "number" && amount === amount;
            if (!known) {
                var tick = svgEl("line");
                tick.setAttribute("class", "tick");
                tick.setAttribute("x1", String(x + slot / 2));
                tick.setAttribute("x2", String(x + slot / 2));
                tick.setAttribute("y1", String(baseline - 4));
                tick.setAttribute("y2", String(baseline + 4));
                svg.appendChild(tick);
            } else if (amount === 0) {
                var flat = svgEl("rect");
                flat.setAttribute("class", "bar-zero");
                flat.setAttribute("x", String(x + 1));
                flat.setAttribute("y", String(baseline - 1));
                flat.setAttribute("width", String(slot - 2));
                flat.setAttribute("height", "2");
                svg.appendChild(flat);
            } else {
                var y1 = yOf(amount);
                var top = y1 < baseline ? y1 : baseline;
                var h = Math.abs(baseline - y1);
                if (h < 1) h = 1;
                var bar = svgEl("rect");
                bar.setAttribute("class", amount < 0 ? "bar-neg" : "bar-pos");
                bar.setAttribute("x", String(x + 1));
                bar.setAttribute("y", String(top));
                bar.setAttribute("width", String(slot - 2));
                bar.setAttribute("height", String(h));
                svg.appendChild(bar);
            }
        }
        var caption = el("p", "chart-caption", tr("mini_chart_hint"));
        svg.addEventListener("click", function (ev) {
            if (!n) return;
            var rect = svg.getBoundingClientRect();
            if (!(rect.width > 0)) return;
            var index = Math.floor(((ev.clientX - rect.left) / rect.width) * n);
            if (index < 0) index = 0;
            if (index >= n) index = n - 1;
            var picked = days[index] || {};
            var text = picked.text;
            if (text == null || text === "") text = tr("mini_unvalued");
            caption.textContent = (picked.start || "") + " " + text;
        });
        wrap.appendChild(svg);
        wrap.appendChild(caption);
        return wrap;
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
        for (i = 0; i < cut; i++) {
            var row = list[i];
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
        if (days.length >= 2) {
            var chartCard = el("div", "card");
            chartCard.appendChild(el("h2", "card-title", tr("mini_report_daily")));
            chartCard.appendChild(renderBars(days));
            host.appendChild(chartCard);
        }
        appendMoneyList(host, tr("mini_report_by_core"), data.by_core, {
            nameClass: "name core-name",
            sort: true,
            limit: MONEY_LIST_LIMIT,
            meta: coreOrdersMeta
        });
        appendMoneyList(host, tr("mini_report_by_exchange"), data.by_exchange, { sort: true });
        restoreSnap(snap, y);
    }

    function paintCores() {
        var host = sections.cores;
        var snap = focusSnap();
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.cores || {};
        var cores = Array.isArray(data.cores) ? data.cores : [];
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
        var coreQuery = cores.length > 10 ? queries.cores : "";
        if (cores.length > 10) {
            host.appendChild(searchField("search-cores", coreQuery, function (value) {
                queries.cores = value;
                paintCores();
            }));
        }
        var filtered = filterItems(cores, coreQuery, function (core) {
            return [core.name, core.exchange];
        });
        if (coreQuery && !filtered.length) {
            host.appendChild(emptyState(tr("mini_empty_search"), clearSearchAction("cores", paintCores)));
            restoreSnap(snap, y);
            return;
        }
        ensureCollapse("cores", cores);
        appendToggleAll(host, "cores", cores, null, coreQuery, paintCores);
        appendGroups(host, "cores", filtered, coreProblem, coreRow, coreQuery, paintCores, null, null, null, coreGroupSummary);
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
        var hero = el("div", "card hero");
        hero.appendChild(el("p", "label", tr("mini_total")));
        var big = el("p", "");
        applyMoney(big, "hero-value num", data.total_text, data.total);
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
        var balanceQuery = perCore.length > 10 ? queries.balances : "";
        if (perCore.length > 10) {
            host.appendChild(searchField("search-balances", balanceQuery, function (value) {
                queries.balances = value;
                paintBalances();
            }));
        }
        var filtered = filterItems(perCore, balanceQuery, function (row) {
            return [row.name, row.exchange];
        });
        if (balanceQuery && perCore.length && !filtered.length) {
            host.appendChild(emptyState(tr("mini_empty_search"), clearSearchAction("balances", paintBalances)));
            restoreSnap(snap, y);
            return;
        }
        if (filtered.length) {
            ensureCollapse("balances", perCore);
            appendToggleAll(host, "balances", perCore, null, balanceQuery, paintBalances);
            appendGroups(
                host,
                "balances",
                filtered,
                balanceProblem,
                balanceRow,
                balanceQuery,
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
            return [order.coin, order.core_name, order.market, order.side];
        });
        if (orderQuery && !filtered.length) {
            host.appendChild(emptyState(tr("mini_empty_search"), clearSearchAction("orders", paintOrders)));
            restoreSnap(snap, y);
            return;
        }
        ensureCollapse("orders", orders, orderCoreKey);
        appendToggleAll(host, "orders", orders, orderCoreKey, orderQuery, paintOrders);
        appendGroups(
            host,
            "orders",
            filtered,
            orderIsProblem,
            function (order) { return orderRow(order, data); },
            orderQuery,
            paintOrders,
            orderCoreKey,
            orderCoreLabel,
            "name grow core-name",
            orderGroupSummary
        );
        restoreSnap(snap, y);
    }

    var paint = {
        report: paintReport,
        cores: paintCores,
        balances: paintBalances,
        orders: paintOrders
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
        return name === "report" ? REPORT_POLL_MS * 2 : Math.max(POLL_MS * 5, 15000);
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
        loadToken += 1;
        var i;
        for (i = 0; i < TAB_NAMES.length; i++) {
            var tab = TAB_NAMES[i];
            var on = tab === name;
            sections[tab].hidden = !on;
            buttons[tab].className = on ? "active" : "";
            if (on) buttons[tab].setAttribute("aria-current", "page");
            else buttons[tab].removeAttribute("aria-current");
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
            buttons[name] = button;
            var label = tr(TAB_KEYS[name]);
            var span = button.querySelector(".nav-label");
            if (span) span.textContent = label;
            button.setAttribute("aria-label", label);
            button.addEventListener("click", function (ev) {
                selectTab(ev.currentTarget.getAttribute("data-tab"));
            });
        }
        var found = document.querySelectorAll("#app-main section[data-tab]");
        for (i = 0; i < found.length; i++) {
            sections[found[i].getAttribute("data-tab")] = found[i];
        }
        reportPeriod = document.getElementById("report-period");
        reportBody = document.getElementById("report-body");
        if (reportPeriod) reportPeriod.appendChild(periodBar());
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
        return !!(queries.cores || queries.balances || queries.orders);
    }

    function anyGroupOpen() {
        var panes = ["cores", "balances", "orders"];
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
        var panes = ["cores", "balances", "orders"];
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
        if (hasQuery()) {
            queries.cores = "";
            queries.balances = "";
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
        var needed = hasQuery() || anyGroupOpen();
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

    function setOrderActionsDisabled(disabled) {
        var nodes = document.querySelectorAll(".order-actions .cmd");
        var i;
        for (i = 0; i < nodes.length; i++) nodes[i].disabled = !!disabled;
    }

    // Cancelled, a dropped connection, or 504: the page cannot tell whether the command landed.
    function commandUnresolved(res) {
        return !!(res && (res.cancelled || res.status === 0 || res.status === 504));
    }

    function finishCommand(res) {
        commandBusy = false;
        setOrderActionsDisabled(false);
        if (commandUnresolved(res)) {
            showCmdLine(tr("mini_cmd_unknown"));
            hasData.orders = false;
            if (current === "orders") reloadPane("orders", false);
            return;
        }
        if (!res.ok) {
            showCmdLine(res.error || tr("mini_error_read"));
            return;
        }
        var data = res.data || {};
        if (data.ok === true) {
            haptic("success");
            showCmdLine(tr("mini_cmd_sent"));
            if (current === "orders") reloadPane("orders", true);
            return;
        }
        haptic("error");
        var failed = data.error === "not_found" ? tr("mini_cmd_not_found") : tr("mini_cmd_failed");
        showCmdLine(failed);
    }

    function runCommand(confirmText, path, body) {
        if (commandBusy) return;
        commandBusy = true;
        setOrderActionsDisabled(true);
        askConfirm(confirmText, function (ok) {
            if (!ok) {
                commandBusy = false;
                setOrderActionsDisabled(false);
                return;
            }
            hapticImpact();
            api(path, body, true).then(finishCommand);
        });
    }

    bindChrome();
    document.addEventListener("visibilitychange", onVisible);
    startSession();
})();
