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
    var TAB_NAMES = ["report", "cores", "orders", "trades", "strategies", "settings"];
    // Nav buttons by data-tab; "deals" shows the orders or trades pane.
    var TAB_KEYS = {
        report: "mini_tab_report",
        cores: "mini_tab_cores",
        strategies: "mini_tab_strategies",
        deals: "mini_tab_trades",
        settings: "mini_tab_settings"
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
    // Start with closed trades; later visits keep the segment the user last selected.
    var dealsSegment = "trades";
    var dealsSwitch = null;
    var buttons = {};
    var payloads = {};
    var collapse = { cores: {}, orders: {}, strategies: {} };
    var collapseUser = { cores: {}, orders: {}, strategies: {} };
    var hasData = {};
    var queue = [];
    var busy = false;
    var currentJob = null;
    var pollTimer = null;
    var cmdTimer = null;
    var period = "today";
    var current = null;
    // Cores balance and profit figures start masked; one eye toggle reveals them
    // together. Kept for the popup's life like a toggled group, never persisted.
    var balanceRevealed = false;
    var BALANCE_MASK = "******";
    // Which core ids have their coin list open. Survives a repaint; not persisted.
    var coinsOpen = {};
    // Today's realised profit on the Cores hero. A cancelled read leaves no line.
    // `at` is the last completed fetch. A Cores poll or selecting the Cores tab
    // reads again once that fetch is older than REPORT_POLL_MS.
    var coresTodayGen = 0;
    var coresTodayFlight = false;
    var coresTodayShown = false;
    var coresTodayPending = false;
    var coresToday = { phase: "idle", line: null, at: 0 };
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

    // A hidden balance figure: the mask replaces the amount, never an unvalued note.
    function maskMoney(node, baseClass) {
        node.className = baseClass + " masked";
        node.textContent = BALANCE_MASK;
    }

    // Mask a real amount. An empty text stays the unvalued note, masked or not.
    function paintFigure(node, text, value, baseClass) {
        if (!balanceRevealed && text) maskMoney(node, baseClass);
        else applyMoney(node, baseClass, text, value);
    }

    // Open eye while the figures are hidden (tap to show), crossed eye while they are shown.
    function eyeIcon(crossed) {
        var icon = svgEl("svg");
        icon.setAttribute("viewBox", "0 0 24 24");
        icon.setAttribute("aria-hidden", "true");
        icon.setAttribute("class", "eye-icon");
        var lid = svgEl("path");
        lid.setAttribute("d", "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z");
        icon.appendChild(lid);
        var pupil = svgEl("circle");
        pupil.setAttribute("cx", "12");
        pupil.setAttribute("cy", "12");
        pupil.setAttribute("r", "3");
        icon.appendChild(pupil);
        if (crossed) {
            var slash = svgEl("path");
            slash.setAttribute("d", "M4 4l16 16");
            icon.appendChild(slash);
        }
        return icon;
    }

    function restoreScroll(y) {
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

    // True while the Settings draft differs from the last loaded or saved settings,
    // or a save is still in flight. A reload must not replace that draft.
    function settingsBusy() {
        return settingsSaving || settingsIsDirty();
    }

    /** Schedule the next read for `name` and `token`; Settings relies on explicit reloads. */
    function schedulePoll(name, token) {
        clearPoll();
        // A timer would replace a future Settings draft. settingsBusy is the second guard.
        if (name === "settings") return;
        if (document.visibilityState !== "visible") return;
        pollTimer = setTimeout(function () {
            pollTimer = null;
            if (!sessionOk || token !== loadToken || current !== name) return;
            if (document.visibilityState !== "visible") return;
            loadTab(name, token, true);
        }, name === "report" || name === "trades" ? REPORT_POLL_MS : POLL_MS);
    }

    /** Return the read endpoint for `name`, including notification settings. */
    function pathFor(name) {
        if (name === "report") return "/api/report";
        if (name === "cores") return "/api/cores";
        if (name === "trades") return "/api/trades";
        if (name === "strategies") return "/api/strategies";
        if (name === "settings") return "/api/notify";
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

    function groupKeyOf(item, keyFn) {
        if (typeof keyFn === "function") return String(keyFn(item) || "");
        return item && item.exchange ? String(item.exchange) : "";
    }

    // Every pane's groups start collapsed, including on a later refresh. A group the
    // user has toggled keeps that choice while the popup stays open.
    function ensureCollapse(pane, items, keyFn) {
        var groups = groupBy(items, function (item) { return groupKeyOf(item, keyFn); });
        var i;
        for (i = 0; i < groups.length; i++) {
            var slot = "k:" + groups[i].key;
            if (collapseUser[pane][slot]) continue;
            collapse[pane][slot] = true;
        }
    }

    // Exchange header while the group is collapsed: online N of M, then that exchange's total.
    function coreGroupSummary(items) {
        var online = 0;
        var trouble = false;
        var i;
        for (i = 0; i < items.length; i++) {
            var core = items[i];
            if (core && core.conn === "ready") online += 1;
            if (coreProblem(core)) trouble = true;
        }
        var wrap = el("span", "group-meta core-exchange-meta");
        var span = el("span", trouble ? "count num neg" : "count num");
        span.textContent = tr("mini_cores_online")
            .replace("{online}", String(online))
            .replace("{total}", String(items.length));
        wrap.appendChild(span);
        var name = items.length && items[0] ? String(items[0].exchange || "") : "";
        var found = name ? exchangeBalance(name) : null;
        if (found) {
            var fig = el("span", "");
            paintFigure(fig, found.total_text, found.total, "num balance-figure");
            wrap.appendChild(fig);
            if (found.stale > 0) {
                wrap.appendChild(el("span", "badge badge-warn", tr("mini_balance_stale")));
            }
            if (found.excluded > 0) {
                wrap.appendChild(el(
                    "span",
                    "badge badge-warn",
                    tr("mini_partial").replace("{n}", String(found.excluded))
                ));
            }
        }
        return wrap;
    }

    /** Return the Cores response's exchange total for `name`, or null when absent. */
    function exchangeBalance(name) {
        var data = payloads.cores || {};
        var rows = Array.isArray(data.per_exchange) ? data.per_exchange : [];
        var i;
        for (i = 0; i < rows.length; i++) {
            if (rows[i] && rows[i].exchange === name) return rows[i];
        }
        return null;
    }

    // Signed dollars for a client PnL sum, the balance figures' "$" suffix; null when not finite.
    function signedDollars(value) {
        var text = signedFixed(value);
        return text == null ? null : text + "$";
    }

    // Finite order.pnl figures of a set of orders: their sum and how many there were. An order
    // with no position yet has no pnl (order_pnl returns nothing) and adds nothing; the chart
    // position caption skips the same rows.
    function knownPnl(orders) {
        var sum = 0;
        var known = 0;
        var i;
        for (i = 0; i < orders.length; i++) {
            var pnl = orders[i] && orders[i].pnl;
            if (typeof pnl !== "number" || pnl !== pnl || pnl === Infinity || pnl === -Infinity) {
                continue;
            }
            known += 1;
            sum += pnl;
        }
        return { sum: sum, known: known };
    }

    // Core header: "<N> orders", plus the core's open PnL in signed dollars. A core with no
    // valued order shows the count alone.
    function orderGroupSummary(items) {
        var wrap = el("span", "group-meta");
        var n = items.length;
        wrap.appendChild(el("span", "num hint", trf("mini_orders_n_" + pluralForm(n), { n: n })));
        var pnl = knownPnl(items);
        var text = pnl.known ? signedDollars(pnl.sum) : null;
        if (text != null) {
            var fig = el("span", "num");
            applyMoney(fig, "num", text, pnl.sum);
            wrap.appendChild(fig);
        }
        return wrap;
    }

    function appendGroups(parent, pane, items, isProblem, renderRow, repaint, keyFn, labelFn, nameClass, summaryFn) {
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
            var collapsed = !!collapse[pane][slot];
            var card = el("div", "card");
            var head = document.createElement("button");
            head.type = "button";
            // A core-name header wraps its title, so its chevron and figures sit on the first line.
            head.className = (" " + (nameClass || "") + " ").indexOf(" core-name ") !== -1 ? "row group-head core-head" : "row group-head";
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

    /** Build a navigable `core` row with its balance and a separate coin-list toggle. */
    function coreRow(core) {
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
        var sub = el("div", "sub core-sub");
        if (secondary) {
            sub.appendChild(el("span", "", secondary));
            sub.appendChild(el("span", "sep", "·"));
        }
        // What a trader checks first: whether this core trades. Read-only; switched on the detail screen.
        var trade = el("span", "trade-state");
        trade.appendChild(el("span", "", tr("mini_trading")));
        trade.appendChild(stateTag(core.trading));
        sub.appendChild(trade);
        body.appendChild(sub);
        row.appendChild(body);
        var metrics = metricsBlock(core);
        if (metrics) row.appendChild(metrics);
        var figure = core.balance;
        var coins = Array.isArray(core.coins) ? core.coins : [];
        if (figure || coins.length) {
            var line = el("div", "core-money");
            if (figure) {
                var money = el("div", "core-balance");
                var badge = balanceBadge(figure.state);
                if (badge) money.appendChild(badge);
                var total = el("span", "");
                paintFigure(total, figure.total_text, figure.total, "num balance-figure");
                money.appendChild(total);
                line.appendChild(money);
            }
            if (coins.length) {
                var open = !!coinsOpen[core.id];
                var toggle = document.createElement("button");
                toggle.type = "button";
                toggle.className = "coin-chev";
                toggle.setAttribute("aria-expanded", open ? "true" : "false");
                toggle.setAttribute("aria-label", coinNames(coins));
                toggle.textContent = open ? "\u25BE" : "\u25B8";
                toggle.addEventListener("click", function (event) {
                    event.stopPropagation();
                    coinsOpen[core.id] = !coinsOpen[core.id];
                    hapticSelection();
                    paintCores();
                });
                line.appendChild(toggle);
            }
            row.appendChild(line);
        }
        // Per-core controls live on the detail screen the row opens.
        row.appendChild(el("span", "row-chev", "\u203A"));
        return row;
    }

    /** Return the non-empty names in `coins` as the coin-toggle's accessible label. */
    function coinNames(coins) {
        var names = [];
        var i;
        for (i = 0; i < coins.length; i++) {
            if (coins[i] && coins[i].coin) names.push(coins[i].coin);
        }
        return names.join(", ");
    }

    /** Return `core`'s row and its coin list when this popup has that list expanded. */
    function coreBlock(core) {
        var coins = Array.isArray(core.coins) ? core.coins : [];
        if (!coinsOpen[core.id] || !coins.length) return coreRow(core);
        var block = el("div", "core-block");
        block.appendChild(coreRow(core));
        block.appendChild(coinList(coins, core.balance));
        return block;
    }

    /** Build the `coins` list using the core's `figure` to mark stale holdings. */
    function coinList(coins, figure) {
        var list = el("div", "coin-list");
        var i;
        for (i = 0; i < coins.length; i++) list.appendChild(coinLine(coins[i], figure));
        return list;
    }

    /** Render `row`'s quantity and masked value, preserving unpriced and stale states. */
    function coinLine(row, figure) {
        var unpriced = !row || row.value == null;
        var classes = "coin-row";
        if (unpriced) classes += " coin-unpriced";
        if (figure && figure.state && figure.state !== "live") classes += " coin-stale";
        var line = el("div", classes);
        var id = el("span", "coin-id");
        id.appendChild(el("span", "name", row && row.coin ? row.coin : ""));
        id.appendChild(el("span", "num coin-qty", row && row.qty_text ? row.qty_text : ""));
        line.appendChild(id);
        if (unpriced) {
            var word = row && row.value_text ? row.value_text : tr("mini_coin_unpriced");
            line.appendChild(el("span", "num hint", word));
        } else {
            var fig = el("span", "");
            paintFigure(fig, row.value_text, row.value, "num balance-figure");
            line.appendChild(fig);
        }
        return line;
    }

    function balanceBadge(state) {
        if (!state || state === "live") return null;
        var text = tr("mini_balance_" + state);
        if (!text) return null;
        return el("span", "badge badge-warn", text);
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

    // Directional move of a held position; for a resting entry, how far the mark is from it.
    // Nothing at all when neither is known: a dash reads as a broken figure.
    function orderChange(order) {
        if (order.change_text) {
            var tone = signClass(order.change_pct);
            return el("span", "num order-change" + (tone ? " " + tone : ""), order.change_text);
        }
        if (order.to_entry_text) {
            var wait = el("span", "num order-change hint");
            wait.appendChild(el("span", "k", tr("mini_orders_to_entry") + " "));
            wait.appendChild(document.createTextNode(order.to_entry_text));
            return wait;
        }
        return null;
    }

    function orderResult(order) {
        if (!order.pnl_text) return null;
        var node = el("span", "num order-pnl");
        // Dollars like the core header and the summary above it.
        applyMoney(node, "num order-pnl", order.pnl_text + "$", order.pnl);
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

    // Owner order actions, a labelled pair on their own line under the figures. Colours name the
    // action, never the side: cancel is neutral, Panic Sell the danger tone on every row.
    // Drawn only when can_control is true.
    function orderActions(row, order) {
        var line = el("div", "order-actions");
        var cancel = document.createElement("button");
        cancel.type = "button";
        cancel.className = "cmd cmd-cancel";
        cancel.textContent = "✕ " + tr("mini_cancel");
        cancel.disabled = commandBusy;
        var panic = document.createElement("button");
        panic.type = "button";
        panic.className = "cmd cmd-danger";
        var panicLabel = tr(order.panic_armed ? "mini_panic_off" : "mini_panic_sell");
        panic.textContent = (order.panic_armed ? "↺ " : "⚡ ") + panicLabel;
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

    // Dense terminal row: coin, side, change and PnL; the price line; the owner's actions below.
    function orderRow(order, data) {
        var row = el("div", "row order-row");
        var main = el("div", "order-main");
        var top = el("div", "order-top");
        var coin = el("span", "name order-coin", order.coin || "");
        coin.title = order.coin || "";
        top.appendChild(coin);
        top.appendChild(el("span", "badge order-side " + sideClass(order.side), sideLabel(order.side)));
        var change = orderChange(order);
        if (change) top.appendChild(change);
        var result = orderResult(order);
        if (result) top.appendChild(result);
        main.appendChild(top);
        // Entry -> current price, then entry volume. Each figure is one unbreakable unit and the
        // line wraps between them, so no price is ever cut to an ellipsis.
        var flow = el("div", "sub order-flow");
        if (order.entry_text || order.mark_text) {
            var prices = el("span", "order-bit");
            prices.appendChild(el("span", "k", tr("mini_orders_entry")));
            prices.appendChild(el("span", "num", order.entry_text || "?"));
            if (order.mark_text) {
                prices.appendChild(el("span", "order-arrow", " → "));
                prices.appendChild(el("span", "num order-mark", order.mark_text));
            }
            flow.appendChild(prices);
        }
        if (order.volume_text) {
            var volume = el("span", "order-bit");
            volume.appendChild(el("span", "k", tr("mini_orders_volume")));
            volume.appendChild(el("span", "num", order.volume_text));
            flow.appendChild(volume);
        }
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

    // Closed | Open switch of the merged trades tab; it sits outside both panes like the period bar.
    function dealsBar() {
        var bar = el("div", "segments");
        var segs = [["trades", "mini_deals_closed"], ["orders", "mini_deals_open"]];
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
        var pnl = knownPnl(orders);
        var text = pnl.known ? signedDollars(pnl.sum) : null;
        if (text != null) {
            var fig = el("span", "");
            applyMoney(fig, "num money-col", text, pnl.sum);
            line.appendChild(fig);
        }
        return line;
    }

    function paintReport() {
        var host = paneBody("report");
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.report || {};
        var total = data.total || {};
        var orders = typeof total.orders === "number" ? total.orders : 0;
        if (!orders) {
            host.appendChild(emptyState(tr("mini_empty_report"), refreshAction()));
            restoreScroll(y);
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
        // A one-day window (today, yesterday) has one row that only repeats the total above.
        if (days.length > 1) host.appendChild(dayTable(data, days));
        appendMoneyList(host, tr("mini_report_by_exchange"), data.by_exchange, { sort: true });
        appendMoneyList(host, tr("mini_report_by_core"), data.by_core, {
            nameClass: "name core-name",
            limit: MONEY_LIST_LIMIT,
            meta: coreOrdersMeta
        });
        restoreScroll(y);
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
        // One line per switch: its name and how many cores have it on, then two actions for
        // every core. Neither button is a selected state; Stop only carries the danger tone.
        function addPair(label, key, onConfirm, offConfirm) {
            var group = el("div", "mass-group");
            var text = el("div", "mass-text");
            text.appendChild(el("span", "mass-label", label));
            var on = 0;
            for (var c = 0; c < cores.length; c++) {
                if (cores[c][key] === true) on += 1;
            }
            text.appendChild(el("span", "mass-state", trf("mini_on_of", { on: on, total: ids.length })));
            group.appendChild(text);
            var pair = el("span", "mass-pair");
            group.appendChild(pair);
            [true, false].forEach(function (turnOn) {
                var btn = document.createElement("button");
                btn.type = "button";
                btn.className = "cmd chip mass-btn" + (turnOn ? "" : " mass-stop");
                btn.textContent = tr(turnOn ? "mini_start_all" : "mini_stop_all");
                btn.setAttribute("aria-label", label + ": " + btn.textContent);
                btn.disabled = commandBusy || !ids.length;
                btn.addEventListener("click", function () {
                    if (commandBusy || !ids.length) return;
                    var ask = trf(turnOn ? onConfirm : offConfirm, { n: ids.length });
                    runCommand(ask, "/api/cores/switch", { cores: ids, switch: key, on: turnOn });
                });
                pair.appendChild(btn);
            });
            card.appendChild(group);
        }
        addPair(tr("mini_trading"), "trading", "mini_cores_trading_on_confirm", "mini_cores_trading_off_confirm");
        addPair(tr("mini_autodetect"), "auto_detect", "mini_cores_auto_on_confirm", "mini_cores_auto_off_confirm");
        return card;
    }

    /** Repaint the core list or open detail, retaining scroll and requesting today's profit. */
    function paintCores() {
        var host = sections.cores;
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.cores || {};
        var cores = Array.isArray(data.cores) ? data.cores : [];
        if (coreDetailId != null) {
            var shownCore = findCore(cores, coreDetailId);
            if (shownCore) {
                coresTodayPending = false;
                paintCoreDetail(host, shownCore, data);
                restoreScroll(y);
                return;
            }
            // The core left the list (grant or config change): back to the list at its
            // old scroll; restoreScroll below scrolls there and syncs the Back button.
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
            coresTodayPending = false;
            host.appendChild(emptyState(tr("mini_empty_cores"), refreshAction()));
            restoreScroll(y);
            return;
        }
        coresTodayPending = false;
        ensureCoresToday();
        host.appendChild(coresHero(data));
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
        if (data.can_control) host.appendChild(massActions(cores));
        // Exchange groups start folded; the header summary says how many cores are online.
        ensureCollapse("cores", cores, null);
        appendGroups(
            host, "cores", cores, coreProblem,
            coreBlock,
            paintCores, null, null, null, coreGroupSummary
        );
        restoreScroll(y);
    }

    /** Return the Cores total from `data`, trust badges, and any cached profit line. */
    function coresHero(data) {
        var hero = el("div", "card hero");
        var head = el("div", "hero-head");
        head.appendChild(el("p", "label", tr("mini_cores_total")));
        var eye = document.createElement("button");
        eye.type = "button";
        eye.className = "eye-btn";
        eye.setAttribute("aria-pressed", balanceRevealed ? "true" : "false");
        eye.setAttribute("aria-label", tr(balanceRevealed ? "mini_balances_hide" : "mini_balances_show"));
        eye.appendChild(eyeIcon(balanceRevealed));
        eye.addEventListener("click", function () {
            balanceRevealed = !balanceRevealed;
            hapticSelection();
            paintCores();
            // The repaint rebuilds the button; hand keyboard focus to the new one.
            var again = sections.cores.querySelector(".eye-btn");
            if (again) again.focus();
        });
        head.appendChild(eye);
        hero.appendChild(head);
        var big = el("p", "");
        paintFigure(big, data.total_text, data.total, "hero-value num");
        hero.appendChild(big);
        if (data.stale > 0 || data.excluded > 0) {
            var meta = el("div", "hero-sub badges");
            if (data.stale > 0) meta.appendChild(el("span", "badge badge-warn", tr("mini_balance_stale")));
            if (data.excluded > 0) {
                meta.appendChild(el(
                    "span",
                    "badge badge-warn",
                    tr("mini_partial").replace("{n}", String(data.excluded))
                ));
            }
            hero.appendChild(meta);
        }
        var line = coresToday.line;
        if (line && line.text) {
            var today = el("p", "hero-sub");
            today.appendChild(document.createTextNode(tr("mini_cores_today") + ": "));
            var profit = el("span", "");
            paintFigure(profit, line.text, line.usdt, "num");
            today.appendChild(profit);
            hero.appendChild(today);
            coresTodayShown = true;
        } else {
            coresTodayShown = false;
        }
        return hero;
    }

    /** Invalidate old profit callbacks and clear the line before an explicit Cores refresh. */
    function resetCoresToday() {
        coresTodayGen += 1;
        coresTodayFlight = false;
        coresTodayShown = false;
        coresTodayPending = true;
        coresToday = { phase: "idle", line: null, at: 0 };
    }

    /** Return `res`'s formatted report total, or null on failure or any unknown order. */
    function todayLine(res) {
        if (!res.ok || !res.data || !res.data.total) return null;
        var total = res.data.total;
        if (typeof total.unknown_orders === "number" && total.unknown_orders > 0) return null;
        if (!total.text) return null;
        return { text: total.text, usdt: total.usdt };
    }

    /** Return whether a completed profit fetch is younger than REPORT_POLL_MS. */
    function coresTodayFresh() {
        return coresToday.phase === "ready" && Date.now() - coresToday.at < REPORT_POLL_MS;
    }

    // One report read for today's profit. A cancel leaves no line. A completed read is kept
    // until it is older than REPORT_POLL_MS. The Cores poll and selecting the Cores tab both
    // call this. Do not start another read from the cancel callback: selectTab cancels before
    // it changes the current tab, and a replacement read would outlive the switch.
    function ensureCoresToday() {
        if (coresTodayFlight || coresTodayFresh()) return;
        coresTodayFlight = true;
        var gen = coresTodayGen;
        api("/api/report", { period: "today" }).then(function (res) {
            if (gen !== coresTodayGen) return;
            coresTodayFlight = false;
            if (res.cancelled) {
                coresToday = { phase: "idle", line: null, at: 0 };
                return;
            }
            var line = todayLine(res);
            var show = !!(line && line.text);
            var prev = coresToday.line;
            var textChanged = (!line) !== (!prev)
                || (!!line && !!prev && (line.text !== prev.text || line.usdt !== prev.usdt));
            coresToday = { phase: "ready", line: line, at: Date.now() };
            if (current === "cores" && coreDetailId == null && (show !== coresTodayShown || textChanged)) {
                paintCores();
            }
        });
    }

    function paintOrders() {
        var host = sections.orders;
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.orders || {};
        var orders = Array.isArray(data.orders) ? data.orders : [];
        if (!orders.length) {
            host.appendChild(emptyState(tr("mini_empty_orders"), refreshAction()));
            restoreScroll(y);
            return;
        }
        host.appendChild(ordersSummary(orders));
        ensureCollapse("orders", orders, orderCoreKey);
        // Core groups arrive in exchange sections; each section gets the terminal's caption.
        var sectionsOf = groupBy(orders, function (order) { return String(order.exchange || ""); });
        var s;
        for (s = 0; s < sectionsOf.length; s++) {
            if (sectionsOf[s].key) host.appendChild(el("h2", "section-label", sectionsOf[s].key));
            appendGroups(
                host,
                "orders",
                sectionsOf[s].items,
                orderIsProblem,
                function (order) { return orderRow(order, data); },
                paintOrders,
                orderCoreKey,
                orderCoreLabel,
                "name grow core-name",
                orderGroupSummary
            );
        }
        restoreScroll(y);
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
        // Telegram's header back arrow already closes the screen; the text link is only for a
        // client without that button.
        if (!headerBackAvailable()) {
            wrap.appendChild(button("cmd text-btn detail-back", "‹ " + tr("mini_back"), closeCoreDetail));
        }
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

    // Most characters of a core name a closed-trade row shows before it drops leading words.
    var TRADE_CORE_CHARS = 24;

    // Names share long prefixes, so a long one keeps its trailing whole words behind "…".
    // Words split on spaces and "/"; the last word alone is kept even when it exceeds the budget.
    function coreNameTail(name, budget) {
        if (name.length <= budget) return name;
        var parts = name.split(/(?=[ \/])/);
        var tail = parts.pop();
        while (parts.length && parts[parts.length - 1].length + tail.length + 1 <= budget) {
            tail = parts.pop() + tail;
        }
        return "…" + tail;
    }

    // Once the row is laid out, drop further leading words while the name still overflows its
    // span, so the kept tail is never cut at its right end.
    function fitCoreName(node) {
        var text = node.textContent;
        while (node.scrollWidth > node.clientWidth) {
            var body = text.charAt(0) === "…" ? text.slice(1) : text;
            var cut = body.slice(1).search(/[ \/]/);
            if (cut < 0) return;
            text = "…" + body.slice(cut + 1);
            node.textContent = text;
        }
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
            var name = String(trade.core_name);
            var core = el("span", "trade-core", coreNameTail(name, TRADE_CORE_CHARS));
            core.title = name;
            meta.appendChild(core);
            window.requestAnimationFrame(function () { fitCoreName(core); });
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
        var y = window.pageYOffset || 0;
        clear(host);
        var data = payloads.trades || {};
        var trades = Array.isArray(data.trades) ? data.trades : [];
        if (!trades.length) {
            host.appendChild(emptyState(tr("mini_empty_trades"), refreshAction()));
            restoreScroll(y);
            return;
        }
        // The count shown, not the backend's cap.
        host.appendChild(el("p", "list-caption", trf("mini_trades_shown_" + pluralForm(trades.length), { n: trades.length })));
        var card = el("div", "card");
        var i;
        for (i = 0; i < trades.length; i++) card.appendChild(tradeRow(trades[i]));
        host.appendChild(card);
        restoreScroll(y);
    }

    // Closed-trade details retain exchange quantity and add safe entry volume when available.
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
        if (trade.volume_text) detailLine(sheet, tr("mini_trade_volume"), trade.volume_text);
        detailLine(sheet, tr("mini_trade_qty"), trade.qty_text);
        detailLine(sheet, tr("mini_trade_duration"), fmtDuration(trade.duration_secs));
        detailLine(sheet, tr("mini_trade_strategy"), trade.strategy || (trade.manual ? tr("mini_trade_manual") : "—"));
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

    // Core header: "on N of M" strategies, muted at the right.
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
        return el("span", "count num hint", trf("mini_on_of", { on: on, total: total }));
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

    // Small read-only ON / OFF tag in the pill's look; an unknown state is a muted dash.
    function stateTag(state) {
        var known = state === true || state === false;
        return el("span", "tag " + (known ? (state ? "on" : "off") : "unknown"), known ? (state ? "ON" : "OFF") : "—");
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
            restoreScroll(y);
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
                paintStrategies,
                strategyCoreKey,
                strategyCoreLabel,
                "name grow core-name",
                strategyGroupSummary
            );
        }
        restoreScroll(y);
    }

    // Settings draft. Cores and zone are display data and stay out of the snapshot.
    var settingsForm = null;
    var settingsBaseSnap = "";
    var settingsRevision = 0;
    var settingsSaving = false;
    var settingsPhase = "idle";
    var settingsError = "";
    var settingsSavedTimer = null;
    var settingsCores = [];
    var settingsZone = "";
    // Category and search are popup-local navigation, like the other tabs' detail views.
    var settingsCategoryId = null;
    var settingsCoreSearch = "";
    var settingsCoreScroll = 0;
    var SETTINGS_CATEGORIES = [
        { id: "notifications", titleKey: "mini_settings_notifications", summary: settingsNotifySummary, render: settingsNotifications }
    ];

    /** Summarize enabled notification kinds from the retained draft. */
    function settingsNotifySummary() {
        var active = [];
        if (settingsForm.tradesOn) active.push(tr("mini_settings_summary_trades"));
        if (settingsForm.downOn) active.push(tr("mini_settings_summary_cores"));
        if (settingsForm.dailyOn) active.push(tr("mini_settings_summary_daily"));
        return active.length ? active.join(" · ") : tr("mini_settings_summary_off");
    }

    /** Open a registry category or return to the list without discarding unsaved edits. */
    function settingsNavigate(id) {
        settingsCategoryId = id;
        hapticSelection();
        settingsRender();
        restoreScroll(0);
        var focus = sections.settings.querySelector(id ? "[data-settings-back]" : "[data-settings-category]");
        if (focus) focus.focus({ preventScroll: true });
    }

    /** Append the existing notification form under its category title. */
    function settingsNotifications(host) {
        host.appendChild(settingsNote());
        host.appendChild(settingsTradesCard());
        host.appendChild(settingsDownCard());
        host.appendChild(settingsDailyCard());
        host.appendChild(settingsSaveBlock());
    }

    /** Return whether the current form differs from its last adopted settings baseline. */
    function settingsIsDirty() {
        if (!settingsForm || !settingsBaseSnap) return false;
        return settingsSnap(settingsForm) !== settingsBaseSnap;
    }

    /** Return a numerically sorted copy of `ids`, leaving the draft array untouched. */
    function settingsSortedIds(ids) {
        var copy = ids.slice();
        copy.sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; });
        return copy;
    }

    /** Parse unsigned decimal `text`, accepting a comma separator; return null on invalid input. */
    function settingsFinite(text) {
        var raw = String(text == null ? "" : text).trim().replace(/,/g, ".");
        if (!/^\d+(\.\d+)?$/.test(raw)) return null;
        var n = Number(raw);
        if (n !== n || n === Infinity || n === -Infinity) return null;
        return n;
    }

    /** Return whether `text` denotes a finite non-negative threshold. */
    function settingsAmountOk(text) {
        var n = settingsFinite(text);
        return n != null && n >= 0;
    }

    /** Normalize valid threshold `text` for draft comparison, retaining invalid text trimmed. */
    function settingsAmountCanon(text) {
        if (!settingsAmountOk(text)) return String(text == null ? "" : text).trim();
        return String(settingsFinite(text));
    }

    /** Return whether `text` is an integer delay from 1 through 1440 minutes. */
    function settingsMinutesOk(text) {
        var raw = String(text == null ? "" : text).trim();
        if (!/^\d+$/.test(raw)) return false;
        var n = Number(raw);
        return n >= 1 && n <= 1440;
    }

    /** Normalize valid delay `text` for draft comparison, retaining invalid text trimmed. */
    function settingsMinutesCanon(text) {
        if (!settingsMinutesOk(text)) return String(text == null ? "" : text).trim();
        return String(Number(String(text).trim()));
    }

    // Parse HH:MM or HH:MM:00 into hour and minute; return null for any other clock text.
    function settingsParseTime(text) {
        var raw = String(text == null ? "" : text).trim();
        var parts = raw.split(":");
        if (parts.length < 2 || parts.length > 3) return null;
        if (parts.length === 3 && parts[2] !== "00") return null;
        if (!/^\d{1,2}$/.test(parts[0]) || !/^\d{2}$/.test(parts[1])) return null;
        var hour = Number(parts[0]);
        var minute = Number(parts[1]);
        if (hour < 0 || hour > 23 || minute < 0 || minute > 59) return null;
        return { hour: hour, minute: minute };
    }

    /** Normalize valid time `text` to HH:MM, retaining invalid text trimmed. */
    function settingsTimeCanon(text) {
        var parsed = settingsParseTime(text);
        if (!parsed) return String(text == null ? "" : text).trim();
        return twoDigits(parsed.hour) + ":" + twoDigits(parsed.minute);
    }

    /** Serialize `form`'s normalized rules for comparison, ignoring disabled threshold text. */
    function settingsSnap(form) {
        var ids = form.scopeKind === "only" ? settingsSortedIds(form.scopeIds) : [];
        return JSON.stringify({
            t: form.tradesOn === true,
            k: form.scopeKind === "only" ? "only" : "all",
            ids: ids,
            v: form.volumeOn === true ? settingsAmountCanon(form.volumeText) : "",
            p: form.profitOn === true ? settingsAmountCanon(form.profitText) : "",
            l: form.lossOn === true ? settingsAmountCanon(form.lossText) : "",
            d: form.downOn === true,
            m: settingsMinutesCanon(form.minutesText),
            y: form.dailyOn === true,
            h: settingsTimeCanon(form.timeText)
        });
    }

    /** Validate `form`'s enabled thresholds, scope, delay, and clock before allowing Save. */
    function settingsFormValid(form) {
        if (!form) return false;
        if (form.volumeOn && !settingsAmountOk(form.volumeText)) return false;
        if (form.profitOn && !settingsAmountOk(form.profitText)) return false;
        if (form.lossOn && !settingsAmountOk(form.lossText)) return false;
        if (!settingsMinutesOk(form.minutesText)) return false;
        if (!settingsParseTime(form.timeText)) return false;
        if (form.scopeKind === "only" && !form.scopeIds.length) return false;
        return true;
    }

    /** Return the wire rules for a valid `form`; disabled thresholds become null. */
    function settingsPayload(form) {
        var time = settingsParseTime(form.timeText);
        var cores = form.scopeKind === "only"
            ? { kind: "only", ids: settingsSortedIds(form.scopeIds) }
            : { kind: "all" };
        return {
            trades: {
                on: form.tradesOn === true,
                cores: cores,
                min_volume_usd: form.volumeOn ? settingsFinite(form.volumeText) : null,
                profit_at_least_usd: form.profitOn ? settingsFinite(form.profitText) : null,
                loss_at_least_usd: form.lossOn ? settingsFinite(form.lossText) : null
            },
            down: {
                on: form.downOn === true,
                after_minutes: Number(String(form.minutesText).trim())
            },
            daily: {
                on: form.dailyOn === true,
                hour: time.hour,
                minute: time.minute
            }
        };
    }

    // Return a non-negative safe-integer revision, or 0 when it cannot round-trip exactly.
    function settingsRevisionOf(data) {
        var value = data && data.revision;
        if (typeof value !== "number" || value !== value || value < 0 || value > 9007199254740991 || Math.floor(value) !== value) return 0;
        return value;
    }

    // Adopt server settings, cores, zone and revision as the draft baseline; false leaves it unchanged.
    function settingsAdopt(data) {
        var settings = data && data.settings;
        var trades = settings && settings.trades;
        var down = settings && settings.down;
        var daily = settings && settings.daily;
        if (!trades || !down || !daily) return false;
        var scope = trades.cores || {};
        var ids = [];
        if (scope.kind === "only" && Array.isArray(scope.ids)) {
            var i;
            for (i = 0; i < scope.ids.length; i++) {
                var id = scope.ids[i];
                if (typeof id === "number" && id === id && id !== Infinity && id !== -Infinity) ids.push(id);
            }
        }
        var hour = daily.hour;
        var minute = daily.minute;
        var timeText = "21:00";
        if (typeof hour === "number" && typeof minute === "number" && hour === hour && minute === minute) {
            timeText = twoDigits(hour) + ":" + twoDigits(minute);
        }
        var minutes = down.after_minutes;
        var form = {
            tradesOn: trades.on === true,
            scopeKind: scope.kind === "only" ? "only" : "all",
            scopeIds: ids,
            volumeOn: typeof trades.min_volume_usd === "number",
            volumeText: typeof trades.min_volume_usd === "number" ? String(trades.min_volume_usd) : "",
            profitOn: typeof trades.profit_at_least_usd === "number",
            profitText: typeof trades.profit_at_least_usd === "number" ? String(trades.profit_at_least_usd) : "",
            lossOn: typeof trades.loss_at_least_usd === "number",
            lossText: typeof trades.loss_at_least_usd === "number" ? String(trades.loss_at_least_usd) : "",
            downOn: down.on === true,
            minutesText: typeof minutes === "number" ? String(minutes) : "5",
            dailyOn: daily.on === true,
            timeText: timeText
        };
        settingsForm = form;
        settingsCores = Array.isArray(data.cores) ? data.cores : [];
        settingsZone = typeof data.zone === "string" ? data.zone : "";
        settingsRevision = settingsRevisionOf(data);
        settingsBaseSnap = settingsSnap(form);
        return true;
    }

    /** Cancel the saved-status timer and clear completed feedback when the draft changes. */
    function settingsTouch() {
        if (settingsSavedTimer) {
            clearTimeout(settingsSavedTimer);
            settingsSavedTimer = null;
        }
        if (settingsPhase === "saved" || settingsPhase === "error") {
            settingsPhase = "idle";
            settingsError = "";
        }
    }

    /** Hide saved feedback after two seconds unless a later edit changed its phase. */
    function settingsArmSaved() {
        if (settingsSavedTimer) clearTimeout(settingsSavedTimer);
        settingsSavedTimer = setTimeout(function () {
            settingsSavedTimer = null;
            if (settingsPhase !== "saved") return;
            settingsPhase = "idle";
            settingsSyncChrome();
        }, 2000);
    }

    /** Return whether `id` occurs in the latest server-provided visible core list. */
    function settingsKnownCore(id) {
        var i;
        for (i = 0; i < settingsCores.length; i++) {
            if (settingsCores[i].id === id) return true;
        }
        return false;
    }

    /** Return whether any selected `ids` occur in the visible core list. */
    function settingsHasVisible(ids) {
        var i;
        for (i = 0; i < ids.length; i++) {
            if (settingsKnownCore(ids[i])) return true;
        }
        return false;
    }

    /** Return whether `ids` retain any selection absent from the visible core list. */
    function settingsHasUnknown(ids) {
        var i;
        for (i = 0; i < ids.length; i++) {
            if (!settingsKnownCore(ids[i])) return true;
        }
        return false;
    }

    /** Render the category registry or its selected page while retaining the Settings draft. */
    function settingsRender() {
        var host = sections.settings;
        if (!host || !settingsForm) return;
        var y = window.pageYOffset || 0;
        clear(host);
        var category = null;
        SETTINGS_CATEGORIES.forEach(function (entry) {
            if (entry.id === settingsCategoryId) category = entry;
        });
        if (category) {
            var back = button("settings-back", tr("mini_back"), function () { settingsNavigate(null); });
            back.setAttribute("data-settings-back", "");
            host.appendChild(back);
            host.appendChild(el("h2", "settings-title", tr(category.titleKey)));
            category.render(host);
        } else {
            settingsCategoryId = null;
            host.appendChild(el("h2", "settings-title", tr("mini_tab_settings")));
            var list = el("div", "card settings-categories");
            SETTINGS_CATEGORIES.forEach(function (entry) {
                var row = button("settings-category", "", function () { settingsNavigate(entry.id); });
                row.setAttribute("data-settings-category", entry.id);
                var text = el("span", "settings-category-text");
                text.appendChild(el("span", "settings-category-title", tr(entry.titleKey)));
                text.appendChild(el("span", "settings-category-summary", entry.summary()));
                row.appendChild(text);
                var arrow = el("span", "chev", "›");
                arrow.setAttribute("aria-hidden", "true");
                row.appendChild(arrow);
                list.appendChild(row);
            });
            host.appendChild(list);
        }
        var coreList = host.querySelector(".settings-core-list");
        if (coreList) coreList.scrollTop = settingsCoreScroll;
        restoreScroll(y);
    }

    /** Clear old feedback, give selection feedback, and repaint after a draft toggle. */
    function settingsToggleDone() {
        settingsTouch();
        hapticSelection();
        settingsRender();
    }

    /** Return a labelled `on`/off toggle that calls `onClick` only while editing is allowed. */
    function settingsSwitch(on, label, disabled, attr, onClick) {
        var pill = button("pill " + (on ? "on" : "off"), on ? "ON" : "OFF", function () {
            if (pill.disabled || settingsSaving) return;
            onClick();
        });
        pill.setAttribute("aria-pressed", on ? "true" : "false");
        pill.setAttribute("aria-label", label);
        if (attr) pill.setAttribute("data-settings", attr);
        pill.disabled = !!disabled || settingsSaving;
        return pill;
    }

    /** Return a labelled text input; edits call `assign` and update validation without rebuilding it. */
    function settingsTextInput(attr, value, disabled, invalid, label, mode, assign) {
        var input = document.createElement("input");
        input.type = "text";
        input.setAttribute("inputmode", mode);
        input.setAttribute("autocomplete", "off");
        input.className = "settings-input" + (invalid ? " invalid" : "");
        input.value = value;
        input.disabled = !!disabled;
        input.setAttribute("aria-label", label);
        input.setAttribute("aria-invalid", invalid ? "true" : "false");
        input.setAttribute("data-settings", attr);
        /** Copy this input's value into the draft unless a save is in flight. */
        function apply() {
            if (settingsSaving || !settingsForm) return;
            assign(input.value);
            settingsTouch();
            settingsSyncChrome();
        }
        input.addEventListener("input", apply);
        input.addEventListener("change", apply);
        return input;
    }

    /** Return a minute-resolution time input initialized from `value` and bound to the draft. */
    function settingsTimeInput(value, disabled, invalid) {
        var input = document.createElement("input");
        input.type = "time";
        input.step = "60";
        input.className = "settings-input" + (invalid ? " invalid" : "");
        input.value = value;
        input.disabled = !!disabled;
        input.setAttribute("aria-label", tr("mini_settings_time"));
        input.setAttribute("aria-invalid", invalid ? "true" : "false");
        input.setAttribute("data-settings", "time");
        /** Copy this input's clock into the draft unless a save is in flight. */
        function apply() {
            if (settingsSaving || !settingsForm) return;
            settingsForm.timeText = input.value;
            settingsTouch();
            settingsSyncChrome();
        }
        input.addEventListener("input", apply);
        input.addEventListener("change", apply);
        return input;
    }

    /** Update `node`'s visual and accessible invalid state, ignoring an absent node. */
    function settingsMarkField(node, invalid) {
        if (!node) return;
        if (invalid) node.classList.add("invalid");
        else node.classList.remove("invalid");
        node.setAttribute("aria-invalid", invalid ? "true" : "false");
    }

    /** Refresh field errors, Save availability, and feedback while retaining input focus. */
    function settingsSyncChrome() {
        var host = sections.settings;
        if (!host || !settingsForm) return;
        var form = settingsForm;
        settingsMarkField(host.querySelector('[data-settings="min-volume"]'), form.volumeOn && !settingsAmountOk(form.volumeText));
        settingsMarkField(host.querySelector('[data-settings="profit"]'), form.profitOn && !settingsAmountOk(form.profitText));
        settingsMarkField(host.querySelector('[data-settings="loss"]'), form.lossOn && !settingsAmountOk(form.lossText));
        settingsMarkField(host.querySelector('[data-settings="minutes"]'), !settingsMinutesOk(form.minutesText));
        settingsMarkField(host.querySelector('[data-settings="time"]'), !settingsParseTime(form.timeText));
        var save = host.querySelector("[data-settings-save]");
        if (save) save.disabled = settingsSaving || !settingsFormValid(form) || !settingsIsDirty();
        var status = host.querySelector("[data-settings-status]");
        if (!status) return;
        if (settingsPhase === "saved") {
            status.hidden = false;
            status.className = "settings-status pos";
            status.textContent = tr("mini_settings_saved");
        } else if (settingsPhase === "error") {
            status.hidden = false;
            status.className = "settings-status neg";
            status.textContent = settingsError;
        } else {
            status.hidden = true;
            status.textContent = "";
            status.className = "settings-status";
        }
    }

    /** Return the localized explanation of where this chat receives notifications. */
    function settingsNote() {
        var note = el("p", "settings-note", tr("mini_settings_chat_note"));
        note.setAttribute("data-settings-note", "");
        return note;
    }

    /** Return a card for `kind` whose localized header toggles draft field `onField`. */
    function settingsCard(kind, titleKey, onField) {
        var card = el("div", "card settings-card");
        card.setAttribute("data-settings-card", kind);
        var head = el("div", "row spread settings-head");
        head.appendChild(el("div", "name", tr(titleKey)));
        head.appendChild(settingsSwitch(!!settingsForm[onField], tr(titleKey), false, "card-on", function () {
            settingsForm[onField] = !settingsForm[onField];
            settingsToggleDone();
        }));
        card.appendChild(head);
        return card;
    }

    /** Return an options container with the inactive visual state when `on` is false. */
    function settingsOptions(on) {
        return el("div", "settings-options" + (on ? "" : " is-off"));
    }

    /** Return a compact core row whose name truncates while its check remains visible. */
    function settingsCoreRow(core, disabled) {
        var id = core.id;
        var selected = settingsForm.scopeKind === "only" && settingsForm.scopeIds.indexOf(id) >= 0;
        var chip = button("settings-core-row" + (selected ? " on" : ""), "", function () {
            if (chip.disabled || settingsSaving) return;
            var list = chip.parentNode;
            settingsCoreScroll = list.scrollTop;
            settingsToggleCore(id);
            var replacement = sections.settings.querySelector('[data-settings-core="' + id + '"]');
            if (replacement) replacement.focus({ preventScroll: true });
        });
        var check = el("span", "settings-core-check", selected ? "✓" : "");
        check.setAttribute("aria-hidden", "true");
        chip.appendChild(check);
        chip.appendChild(el("span", "settings-core-name", core.name || ""));
        chip.title = (core.name || "") + " · " + (core.exchange || "");
        chip.setAttribute("aria-label", chip.title);
        chip.setAttribute("data-settings-core", String(id));
        chip.setAttribute("aria-pressed", selected ? "true" : "false");
        chip.disabled = !!disabled;
        return chip;
    }

    /** Populate a bounded core list, filtering names and exchanges without changing selection. */
    function settingsCoreRows(list, disabled) {
        clear(list);
        var query = settingsCoreSearch.trim().toLocaleLowerCase();
        settingsCores.forEach(function (core) {
            if (((core.name || "") + " " + (core.exchange || "")).toLocaleLowerCase().indexOf(query) >= 0) {
                list.appendChild(settingsCoreRow(core, disabled));
            }
        });
        if (!list.firstChild) list.appendChild(el("p", "settings-caption", tr("mini_settings_no_cores")));
        list.scrollTop = settingsCoreScroll;
    }

    /** Return search, the retained selection count, and a dense scrollable multi-select list. */
    function settingsCorePicker(disabled) {
        var picker = el("div", "settings-core-picker");
        var search = el("input", "settings-input settings-core-search");
        search.type = "search";
        search.placeholder = tr("mini_settings_search_cores");
        search.setAttribute("aria-label", search.placeholder);
        search.setAttribute("data-settings-search", "");
        search.value = settingsCoreSearch;
        search.disabled = disabled;
        picker.appendChild(search);
        picker.appendChild(el("p", "settings-caption", trf("mini_settings_selected", {
            n: settingsForm.scopeIds.length, m: settingsCores.length
        })));
        var list = el("div", "settings-core-list");
        search.addEventListener("input", function () {
            settingsCoreSearch = search.value;
            settingsCoreScroll = 0;
            settingsCoreRows(list, disabled);
        });
        settingsCoreRows(list, disabled);
        picker.appendChild(list);
        return picker;
    }

    /** Toggle `id` in the explicit scope; removing its last id restores the All scope. */
    function settingsToggleCore(id) {
        if (settingsSaving || !settingsForm) return;
        var form = settingsForm;
        var ids = form.scopeIds.slice();
        if (form.scopeKind !== "only") {
            form.scopeKind = "only";
            form.scopeIds = [id];
        } else {
            var at = ids.indexOf(id);
            if (at >= 0) ids.splice(at, 1);
            else ids.push(id);
            if (!settingsHasVisible(ids) && !settingsHasUnknown(ids)) {
                form.scopeKind = "all";
                form.scopeIds = [];
            } else {
                form.scopeKind = "only";
                form.scopeIds = ids;
            }
        }
        settingsToggleDone();
    }

    /** Return a labelled threshold toggle and input bound to `onField` and `textField`. */
    function settingsThresholdRow(labelKey, switchAttr, inputAttr, onField, textField, cardOn) {
        var on = !!settingsForm[onField];
        var text = settingsForm[textField];
        var row = el("div", "settings-field");
        row.appendChild(el("span", "settings-label", tr(labelKey)));
        var locked = !cardOn || settingsSaving;
        row.appendChild(settingsSwitch(on, tr(labelKey), locked, switchAttr, function () {
            settingsForm[onField] = !settingsForm[onField];
            settingsToggleDone();
        }));
        row.appendChild(settingsTextInput(
            inputAttr,
            text,
            locked || !on,
            on && !settingsAmountOk(text),
            tr(labelKey),
            "decimal",
            function (value) { settingsForm[textField] = value; }
        ));
        return row;
    }

    /** Return the trade card with core selection, optional thresholds, and the no-filter hint. */
    function settingsTradesCard() {
        var card = settingsCard("trades", "mini_settings_trades", "tradesOn");
        var options = settingsOptions(settingsForm.tradesOn);
        var locked = !settingsForm.tradesOn || settingsSaving;
        var chips = el("div", "settings-chips");
        var allOn = settingsForm.scopeKind !== "only";
        var all = button("chip" + (allOn ? " on" : ""), tr("mini_settings_all_cores"), function () {
            if (all.disabled || settingsSaving) return;
            settingsForm.scopeKind = allOn ? "only" : "all";
            settingsForm.scopeIds = [];
            settingsToggleDone();
        });
        all.setAttribute("data-settings", "scope-all");
        all.setAttribute("aria-pressed", allOn ? "true" : "false");
        all.disabled = locked;
        chips.appendChild(all);
        options.appendChild(chips);
        if (!allOn) options.appendChild(settingsCorePicker(locked));
        options.appendChild(settingsThresholdRow("mini_settings_min_volume", "volume-switch", "min-volume", "volumeOn", "volumeText", settingsForm.tradesOn));
        options.appendChild(settingsThresholdRow("mini_settings_profit_at_least", "profit-switch", "profit", "profitOn", "profitText", settingsForm.tradesOn));
        options.appendChild(settingsThresholdRow("mini_settings_loss_at_least", "loss-switch", "loss", "lossOn", "lossText", settingsForm.tradesOn));
        if (!settingsForm.volumeOn && !settingsForm.profitOn && !settingsForm.lossOn) {
            var hint = el("p", "settings-hint", tr("mini_settings_trades_hint"));
            hint.setAttribute("data-settings-hint", "");
            options.appendChild(hint);
        }
        card.appendChild(options);
        return card;
    }

    /** Return the outage card with its retained delay and save-time validation state. */
    function settingsDownCard() {
        var card = settingsCard("down", "mini_settings_down", "downOn");
        var options = settingsOptions(settingsForm.downOn);
        var locked = !settingsForm.downOn || settingsSaving;
        var row = el("div", "settings-field");
        row.appendChild(el("span", "settings-label", tr("mini_settings_after_minutes")));
        row.appendChild(settingsTextInput(
            "minutes",
            settingsForm.minutesText,
            locked,
            !settingsMinutesOk(settingsForm.minutesText),
            tr("mini_settings_after_minutes"),
            "numeric",
            function (value) { settingsForm.minutesText = value; }
        ));
        options.appendChild(row);
        card.appendChild(options);
        return card;
    }

    /** Return the daily card with a minute-resolution clock and the host's report zone. */
    function settingsDailyCard() {
        var card = settingsCard("daily", "mini_settings_daily", "dailyOn");
        var options = settingsOptions(settingsForm.dailyOn);
        var locked = !settingsForm.dailyOn || settingsSaving;
        var row = el("div", "settings-field");
        row.appendChild(el("span", "settings-label", tr("mini_settings_time")));
        row.appendChild(settingsTimeInput(settingsForm.timeText, locked, !settingsParseTime(settingsForm.timeText)));
        options.appendChild(row);
        options.appendChild(el("p", "settings-caption", tr("mini_settings_zone") + " " + settingsZone));
        card.appendChild(options);
        return card;
    }

    /** Return the decorative SVG used while a settings save is in flight. */
    function settingsSpinner() {
        var icon = svgEl("svg");
        icon.setAttribute("viewBox", "0 0 24 24");
        icon.setAttribute("class", "settings-spin");
        icon.setAttribute("aria-hidden", "true");
        var ring = svgEl("circle");
        ring.setAttribute("cx", "12");
        ring.setAttribute("cy", "12");
        ring.setAttribute("r", "8");
        ring.setAttribute("fill", "none");
        ring.setAttribute("stroke", "currentColor");
        ring.setAttribute("stroke-width", "2");
        ring.setAttribute("stroke-dasharray", "14 36");
        icon.appendChild(ring);
        return icon;
    }

    /** Return Save and its status line, reflecting draft validity, dirtiness, and save progress. */
    function settingsSaveBlock() {
        var wrap = el("div", "settings-save-wrap");
        var btn = button("action-btn settings-save", tr("mini_settings_save"), settingsSave);
        btn.setAttribute("data-settings-save", "");
        btn.disabled = settingsSaving || !settingsFormValid(settingsForm) || !settingsIsDirty();
        if (settingsSaving) {
            btn.appendChild(settingsSpinner());
            btn.setAttribute("aria-busy", "true");
        }
        wrap.appendChild(btn);
        var status = el("p", "settings-status");
        status.setAttribute("data-settings-status", "");
        status.setAttribute("role", "status");
        if (settingsPhase === "saved") {
            status.className = "settings-status pos";
            status.textContent = tr("mini_settings_saved");
        } else if (settingsPhase === "error") {
            status.className = "settings-status neg";
            status.textContent = settingsError;
        } else {
            status.hidden = true;
        }
        wrap.appendChild(status);
        return wrap;
    }

    /** Submit a valid changed draft with its revision, retaining the request across tab switches. */
    function settingsSave() {
        if (settingsSaving || !settingsForm) return;
        if (!settingsFormValid(settingsForm) || !settingsIsDirty()) return;
        settingsSaving = true;
        settingsPhase = "saving";
        settingsError = "";
        hapticImpact("light");
        settingsRender();
        api("/api/notify/save", { settings: settingsPayload(settingsForm), revision: settingsRevision }, true).then(settingsSaved);
    }

    // A refused save is HTTP 200 with error set. Keep the draft, except a stale
    // revision: another window saved, so the form shows the stored settings and
    // the draft is cleared. Only a clean save replaces the baseline without an
    // error. A cancelled keep-request drops the spinner and stops.
    function settingsSaved(res) {
        if (res.cancelled) {
            settingsSaving = false;
            if (settingsPhase === "saving") settingsPhase = "idle";
            if (current === "settings") settingsRender();
            return;
        }
        if (res.ok && res.data && res.data.fault === "stale" && settingsAdopt(res.data)) {
            payloads.settings = res.data;
            lastRaw.settings = res.raw || null;
            hasData.settings = true;
            settingsSaving = false;
            settingsPhase = "error";
            settingsError = String(res.data.error);
            haptic("error");
            if (current === "settings") settingsRender();
            return;
        }
        if (!res.ok || !res.data || res.data.error != null) {
            settingsSaving = false;
            settingsPhase = "error";
            settingsError = res.ok && res.data && res.data.error != null
                ? String(res.data.error)
                : tr("mini_settings_err_save");
            haptic("error");
            if (current === "settings") settingsRender();
            return;
        }
        if (!settingsAdopt(res.data)) {
            settingsSaving = false;
            settingsPhase = "error";
            settingsError = tr("mini_settings_err_save");
            haptic("error");
            if (current === "settings") settingsRender();
            return;
        }
        payloads.settings = res.data;
        lastRaw.settings = res.raw || null;
        hasData.settings = true;
        settingsSaving = false;
        settingsPhase = "saved";
        settingsError = "";
        haptic("success");
        markUpdated("settings");
        if (current === "settings") {
            settingsRender();
            settingsArmSaved();
        }
    }

    /** Adopt loaded settings only for a clean idle form, then render the retained draft. */
    function paintSettings() {
        var host = sections.settings;
        if (!host) return;
        var y = window.pageYOffset || 0;
        // A dirty draft or a save in flight keeps what the user typed.
        if (!settingsSaving && !settingsIsDirty()) {
            if (!settingsAdopt(payloads.settings)) {
                settingsForm = null;
                settingsBaseSnap = "";
                clear(host);
                host.appendChild(emptyState(tr("mini_error_read")));
                restoreScroll(y);
                return;
            }
        }
        settingsRender();
    }

    var paint = {
        report: paintReport,
        cores: paintCores,
        orders: paintOrders,
        trades: paintTrades,
        strategies: paintStrategies,
        settings: paintSettings
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

    /** Refresh the active tab, protecting Settings edits and invalidating the Cores profit cache. */
    function refreshCurrent() {
        if (!current || inFlight[current]) return;
        if (current === "settings" && settingsBusy()) return;
        hapticImpact("light");
        setSpinning(true);
        if (current === "cores") resetCoresToday();
        reloadPane(current, !!hasData[current]);
    }

    /** Read `name` for the current `token`, preserving good background data and Settings edits. */
    function loadTab(name, token, silent) {
        if (name !== current || token !== loadToken) return;
        if (name === "settings" && settingsBusy()) return;
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
            if (name === "settings" && settingsBusy()) return;
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
            if (name === "settings" && settingsBusy()) return;
            if (!same) paint[name]();
            else if (name === "cores" && coreDetailId == null && coresToday.line && !coresTodayShown) paintCores();
            else if (name === "cores" && coresTodayPending) paintCores();
            if (name === "cores") ensureCoresToday();
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

    /** Show `name`, cancel disposable reads, and request its data plus Cores profit when needed. */
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
        syncBackButton();
        loadTab(name, loadToken, !!hasData[name]);
        if (name === "cores") ensureCoresToday();
    }

    /** Resume visible-tab reads and age updates while preserving a dirty or saving Settings form. */
    function onVisible() {
        if (!sessionOk || !current) return;
        if (document.visibilityState !== "visible") {
            clearPoll();
            stopUpdatedTimer();
            return;
        }
        paintUpdated();
        startUpdatedTimer();
        if (current === "settings" && settingsBusy()) return;
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

    /** Return whether a Cores, Orders, or Strategies exchange/core group is expanded. */
    function anyGroupOpen() {
        var panes = ["cores", "orders", "strategies"];
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

    /** Collapse open Cores, Orders, and Strategies groups and retain that popup-local choice. */
    function collapseOpenGroups() {
        var panes = ["cores", "orders", "strategies"];
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

    /** Return from the active sheet, detail, or Settings category before collapsing groups. */
    function onBack() {
        if (sheetOpen) {
            closeSheet();
            return;
        }
        if (coreDetailId != null) {
            closeCoreDetail();
            return;
        }
        if (current === "settings" && settingsCategoryId != null) {
            settingsNavigate(null);
            return;
        }
        collapseOpenGroups();
        if (current && paint[current]) paint[current]();
        else syncBackButton();
    }

    function headerBackAvailable() {
        var button = webapp && webapp.BackButton;
        if (!button || typeof button.show !== "function") return false;
        // Clients before Bot API 6.1 carry the object but never draw the arrow.
        return typeof webapp.isVersionAtLeast !== "function" || webapp.isVersionAtLeast("6.1");
    }

    /** Show Telegram's BackButton whenever the visible page has a local back destination. */
    function syncBackButton() {
        var button = webapp && webapp.BackButton;
        if (!button) return;
        var needed = sheetOpen || coreDetailId != null || anyGroupOpen()
            || (current === "settings" && settingsCategoryId != null);
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
        var nodes = document.querySelectorAll(".order-actions .cmd, .mass-actions .cmd, .strategy-row .cmd, .core-detail .cmd");
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
            // A rejected command (4xx other than the session ones) never reads as a read error.
            haptic("error");
            var rejected = res.status >= 400 && res.status < 500 && res.status !== 401 && res.status !== 403;
            showCmdLine(rejected ? tr("mini_cmd_failed") : (res.error || tr("mini_error_read")));
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
