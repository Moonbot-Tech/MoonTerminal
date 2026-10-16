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
                // The canvas reads its colours from the scheme's tokens at draw time.
                if (sheetChart) sheetChart.draw();
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

    // Group PnL is a client sum of order.pnl_usd. Match signed_fixed's visible
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
            if ((pack.status === 503 || pack.status === 504) && job.path !== "/api/trade/ticks" && job.path !== "/api/order/ticks") {
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

    function finiteNumber(value) {
        return typeof value === "number" && value === value && value !== Infinity && value !== -Infinity;
    }

    // Dollar PnL of a set of orders: the sum of the server's dollar figures (order.pnl_usd) of
    // REAL orders, how many there were, and how many real orders with a PnL had no dollar value.
    // order.pnl is in each market's own quote and is never added. Emulator orders stay out, as the
    // desktop keeps them apart. An order with no position yet has no pnl and adds nothing; the
    // chart position caption skips the same rows. A sum missing a real order is not shown.
    function knownPnl(orders) {
        var sum = 0;
        var known = 0;
        var unvalued = 0;
        var i;
        for (i = 0; i < orders.length; i++) {
            var order = orders[i];
            if (!order || order.emulator || !finiteNumber(order.pnl)) continue;
            if (!finiteNumber(order.pnl_usd)) {
                unvalued += 1;
                continue;
            }
            known += 1;
            sum += order.pnl_usd;
        }
        return { sum: sum, known: known, unvalued: unvalued };
    }

    // Core header: "<N> orders", plus the core's open PnL in signed dollars. A core with no real
    // order holding a position shows the count alone; one whose real orders cannot all be valued
    // in dollars shows "No value" beside it.
    function orderGroupSummary(items) {
        var wrap = el("span", "group-meta");
        var n = items.length;
        wrap.appendChild(el("span", "num hint", trf("mini_orders_n_" + pluralForm(n), { n: n })));
        var pnl = knownPnl(items);
        // A sum missing a real order is not shown; "No value" says why instead of an empty slot.
        if (pnl.known || pnl.unvalued) {
            var fig = el("span", "num");
            applyMoney(fig, "num", pnl.unvalued ? null : signedDollars(pnl.sum), pnl.sum);
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
        // The server's text carries the unit: dollars for a USD-stable quote, else the quote. The
        // tone follows the sign of that rounded text, not the raw value.
        applyMoney(node, "num order-pnl", order.pnl_text, order.pnl_sign);
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
        // (E) marks an emulated order, as the desktop orders table does.
        var coin = el("span", "name order-coin", (order.coin || "") + (order.emulator ? " (E)" : ""));
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
        flow.appendChild(el("span", "chart-hint", "◉ " + tr("mini_chart_live_hint")));
        main.appendChild(flow);
        row.appendChild(main);
        if (data.can_control) orderActions(row, order);
        // A row tap opens the live chart; a tap on cancel or Panic Sell stays theirs.
        row.setAttribute("role", "button");
        row.tabIndex = 0;
        function onAction(event) {
            return !!(event.target && event.target.closest && event.target.closest(".order-actions"));
        }
        row.addEventListener("click", function (event) {
            if (onAction(event)) return;
            openOrderSheet(order, data);
        });
        row.addEventListener("keydown", function (event) {
            if (onAction(event) || (event.key !== "Enter" && event.key !== " ")) return;
            event.preventDefault();
            openOrderSheet(order, data);
        });
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

    // Order count plus the dollar sum of real orders (knownPnl), same rounding as the group heads.
    function ordersSummary(orders) {
        var line = el("p", "summary spread");
        line.appendChild(el("span", "", tr("mini_orders_summary").replace("{n}", String(orders.length))));
        var pnl = knownPnl(orders);
        if (pnl.known || pnl.unvalued) {
            var fig = el("span", "");
            applyMoney(fig, "num money-col", pnl.unvalued ? null : signedDollars(pnl.sum), pnl.sum);
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
            // The desktop asks a second click for the same action; the page asks one confirm.
            runCommand(tr("mini_cancel_all_confirm"), "/api/core/cancel_all", { core: core.id });
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
        right.appendChild(el("span", "chart-hint", "◔ " + tr("mini_chart_hint")));
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

    // ---------- Tick charts: a closed trade's tape and an open position's live stream ----------

    var CHART_POLL_MS = 500;
    var CHART_SLOW_POLL_MS = 1000;
    var CHART_CLOSING_TRIES = 30;
    var CHART_LOAD_TRIES = 20;
    var CHART_LOAD_WAIT_MS = 1500;
    // The live ring: at most this many prints, none older than 15 minutes.
    var LIVE_RING = 4000;
    var LIVE_KEEP_MS = 900000;
    var LIVE_MAX_SPAN_MS = 960000;
    var CHART_MIN_SPAN_MS = 3000;
    var CHART_AXIS_W = 58;
    var CHART_VOL_H = 44;
    var TRADE_RANGES = [["deal", "mini_chart_range_deal"], ["30", "mini_chart_range_30"], ["300", "mini_chart_range_300"]];
    var LIVE_SPANS = [["120", "mini_chart_span_2"], ["300", "mini_chart_span_5"], ["900", "mini_chart_span_15"]];
    // Every sheet open and close bumps it; a reply or timer of an older sheet sees the change and stops.
    var sheetToken = 0;
    var sheetTimer = null;
    var sheetChart = null;
    // The open live sheet's poll state; null while none is open.
    var liveSheet = null;

    function cssVar(name) {
        return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    }

    function clamp01(value) {
        return Math.min(1, Math.max(0, value));
    }

    // Local wall-clock HH:MM:SS of an epoch millisecond.
    function clockAt(ms) {
        var d = new Date(ms);
        return twoDigits(d.getHours()) + ":" + twoDigits(d.getMinutes()) + ":" + twoDigits(d.getSeconds());
    }

    function millisTail(ms) {
        var rest = ((Math.floor(ms) % 1000) + 1000) % 1000;
        return (rest < 10 ? "00" : rest < 100 ? "0" : "") + rest;
    }

    // HH:MM today, HH:MM DD.MM on another day.
    function whenText(ms) {
        var d = new Date(ms);
        var hm = twoDigits(d.getHours()) + ":" + twoDigits(d.getMinutes());
        if (d.toDateString() === new Date().toDateString()) return hm;
        return hm + " " + twoDigits(d.getDate()) + "." + twoDigits(d.getMonth() + 1);
    }

    function qtyText(qty) {
        if (!finiteNumber(qty)) return "?";
        return qty >= 100 ? String(Math.round(qty)) : String(+qty.toPrecision(4));
    }

    function priceDecimals(value) {
        return finiteNumber(value) && value >= 0 && value <= 18 ? Math.floor(value) : 6;
    }

    // Prints as the wire sends them: [t_ms, price, qty, side] with side 1 a buy.
    function tickList(value) {
        var out = [];
        if (!Array.isArray(value)) return out;
        var i;
        for (i = 0; i < value.length; i++) {
            var t = value[i];
            if (Array.isArray(t) && t.length >= 4 && finiteNumber(t[0]) && finiteNumber(t[1])) out.push(t);
        }
        return out;
    }

    // Canvas tick chart: every print a dot sized by quantity on a thin price line, a side-split
    // volume histogram below, dashed order levels, entry/exit markers and the hold-span tint.
    // Drag pans, wheel and pinch zoom, a tap places the crosshair. `opts`: ticks() -> prints,
    // dp, maxSpan, hold [from, to] or null, volume {width, rows: [[start, buy, sell]]} or null
    // (null buckets the prints themselves), noVolume, live (the current-price tag), onView.
    function TickChart(host, opts) {
        var self = this;
        this.host = host;
        this.opts = opts;
        this.levels = [];
        this.marks = [];
        this.mark = null;
        this.view = { from: 0, to: 1, lo: 0, hi: 1 };
        this.cross = null;
        this.follow = false;
        this.shown = 0;
        this.canvas = el("canvas", "chart-canvas");
        this.tip = el("div", "chart-tip");
        this.tip.hidden = true;
        host.appendChild(this.canvas);
        host.appendChild(this.tip);
        this.observer = null;
        if (typeof ResizeObserver === "function") {
            this.observer = new ResizeObserver(function () { self.size(); });
            this.observer.observe(host);
        }
        this.bind();
        this.size();
    }

    TickChart.prototype.destroy = function () {
        if (this.observer) this.observer.disconnect();
        this.observer = null;
    };

    TickChart.prototype.size = function () {
        var box = this.host.getBoundingClientRect();
        var ratio = window.devicePixelRatio || 1;
        if (!box.width || !box.height) return;
        this.canvas.width = Math.round(box.width * ratio);
        this.canvas.height = Math.round(box.height * ratio);
        this.draw();
    };

    TickChart.prototype.plotWidth = function () {
        return Math.max(1, this.canvas.getBoundingClientRect().width - (this.axisWidth || CHART_AXIS_W));
    };

    TickChart.prototype.bind = function () {
        var self = this;
        var cv = this.canvas;
        var points = {};
        var drag = null;
        var pinch = null;
        function active() {
            var list = [];
            var id;
            for (id in points) {
                if (Object.prototype.hasOwnProperty.call(points, id)) list.push(points[id]);
            }
            return list;
        }
        cv.addEventListener("pointerdown", function (e) {
            if (cv.setPointerCapture) {
                try { cv.setPointerCapture(e.pointerId); } catch (err) { /* a synthetic pointer has no capture */ }
            }
            points[e.pointerId] = e.clientX;
            var list = active();
            if (list.length === 2) {
                pinch = { dist: Math.abs(list[0] - list[1]) || 1, mid: (list[0] + list[1]) / 2, from: self.view.from, to: self.view.to };
                drag = null;
            } else if (list.length === 1) {
                drag = { x: e.clientX, from: self.view.from, to: self.view.to, moved: false };
            }
        });
        cv.addEventListener("pointermove", function (e) {
            if (!Object.prototype.hasOwnProperty.call(points, e.pointerId)) return;
            points[e.pointerId] = e.clientX;
            var list = active();
            if (pinch && list.length === 2) {
                var dist = Math.abs(list[0] - list[1]) || 1;
                var fx = clamp01((pinch.mid - cv.getBoundingClientRect().left) / self.plotWidth());
                var span = pinch.to - pinch.from;
                self.zoomTo(span * pinch.dist / dist, pinch.from + fx * span, fx);
                return;
            }
            if (!drag) return;
            var dx = e.clientX - drag.x;
            if (Math.abs(dx) > 3) drag.moved = true;
            if (!drag.moved) return;
            var shift = -dx / self.plotWidth() * (drag.to - drag.from);
            self.view.from = drag.from + shift;
            self.view.to = drag.to + shift;
            self.cross = null;
            self.userMoved(false);
        });
        function release(e, tapped) {
            var tap = tapped && drag && !drag.moved && !pinch;
            delete points[e.pointerId];
            if (tap) {
                var box = cv.getBoundingClientRect();
                self.cross = { x: e.clientX - box.left, y: e.clientY - box.top };
                self.draw();
            }
            var list = active();
            if (list.length < 2) pinch = null;
            drag = list.length === 1 ? { x: list[0], from: self.view.from, to: self.view.to, moved: true } : null;
        }
        cv.addEventListener("pointerup", function (e) { release(e, true); });
        cv.addEventListener("pointercancel", function (e) { release(e, false); });
        cv.addEventListener("wheel", function (e) {
            e.preventDefault();
            var fx = clamp01((e.clientX - cv.getBoundingClientRect().left) / self.plotWidth());
            var span = self.view.to - self.view.from;
            self.zoomTo(span * (e.deltaY > 0 ? 1.18 : 1 / 1.18), self.view.from + fx * span, fx);
        }, { passive: false });
    };

    // Zoom out keeps following the newest print; zoom in and pan stop following.
    TickChart.prototype.zoomTo = function (span, at, fx) {
        var wider = span > this.view.to - this.view.from;
        var next = Math.max(CHART_MIN_SPAN_MS, Math.min(this.opts.maxSpan, span));
        this.view.from = at - fx * next;
        this.view.to = this.view.from + next;
        this.userMoved(wider);
    };

    TickChart.prototype.userMoved = function (keepFollow) {
        if (!keepFollow) this.follow = false;
        if (this.follow && this.opts.now) this.anchor(this.view.to - this.view.from);
        this.fitY();
        this.draw();
        if (this.opts.onView) this.opts.onView();
    };

    // The live window: `span` wide, the newest moment 8 % in from the right edge.
    TickChart.prototype.anchor = function (span) {
        this.view.to = this.opts.now() + span * 0.08;
        this.view.from = this.view.to - span;
    };

    TickChart.prototype.setView = function (from, to) {
        this.view.from = from;
        this.view.to = to;
        this.cross = null;
        this.fitY();
        this.draw();
    };

    TickChart.prototype.fitY = function () {
        var ts = this.opts.ticks();
        var v = this.view;
        var lo = Infinity;
        var hi = -Infinity;
        var i;
        for (i = 0; i < ts.length; i++) {
            var t = ts[i];
            if (t[0] < v.from || t[0] > v.to) continue;
            if (t[1] < lo) lo = t[1];
            if (t[1] > hi) hi = t[1];
        }
        for (i = 0; i < this.levels.length; i++) {
            if (!this.levels[i].fit) continue;
            lo = Math.min(lo, this.levels[i].price);
            hi = Math.max(hi, this.levels[i].price);
        }
        if (!isFinite(lo) || !isFinite(hi)) {
            lo = 0;
            hi = 1;
        }
        var pad = (hi - lo) * 0.08 || Math.abs(hi) * 0.001 || 1;
        v.lo = lo - pad;
        v.hi = hi + pad;
    };

    TickChart.prototype.draw = function () {
        var cv = this.canvas;
        var g = cv.getContext ? cv.getContext("2d") : null;
        if (!g || !cv.width || !cv.height) return;
        var ratio = window.devicePixelRatio || 1;
        var W = cv.width / ratio;
        var H = cv.height / ratio;
        g.setTransform(ratio, 0, 0, ratio, 0, 0);
        g.clearRect(0, 0, W, H);
        var v = this.view;
        var dp = this.opts.dp;
        var plotW;
        var plotH = H - CHART_VOL_H - 18;
        var span = v.to - v.from || 1;
        var tone = {};
        function color(name) {
            if (!Object.prototype.hasOwnProperty.call(tone, name)) tone[name] = cssVar(name);
            return tone[name];
        }
        function X(t) { return (t - v.from) / span * plotW; }
        function Y(p) { return 6 + (v.hi - p) / (v.hi - v.lo) * (plotH - 6); }
        var mono = color("--font-mono");
        var ui = color("--font-ui");
        var ts = this.opts.ticks();
        var vis = [];
        var i;
        for (i = 0; i < ts.length; i++) {
            if (ts[i][0] >= v.from - 2000 && ts[i][0] <= v.to + 2000) vis.push(ts[i]);
        }
        this.shown = 0;
        for (i = 0; i < vis.length; i++) if (vis[i][0] >= v.from && vis[i][0] <= v.to) this.shown += 1;
        // What is drawn, readable without the canvas: held prints, the window, and follow.
        this.host.setAttribute("data-ticks", String(ts.length));
        this.host.setAttribute("data-span", String(Math.round(span)));
        this.host.setAttribute("data-follow", this.follow ? "1" : "0");
        var levels = this.levels;
        var nowPrice = null;
        if (this.opts.live) {
            if (finiteNumber(this.mark)) nowPrice = this.mark;
            else if (ts.length) nowPrice = ts[ts.length - 1][1];
        }
        // Measure at the drawing precision so small-price tags stay inside narrow canvases.
        var axisW = CHART_AXIS_W;
        g.font = "600 10px " + mono;
        var labelPrices = [v.lo, v.hi];
        for (i = 0; i < levels.length; i++) labelPrices.push(levels[i].price);
        if (nowPrice != null) labelPrices.push(nowPrice);
        for (i = 0; i < labelPrices.length; i++) axisW = Math.max(axisW, Math.ceil(g.measureText(labelPrices[i].toFixed(dp)).width) + 12);
        this.axisWidth = axisW;
        plotW = Math.max(1, W - axisW);
        // Grid and price axis; an axis label gives way to a level tag or the price tag on its row.
        g.font = "10px " + mono;
        g.textBaseline = "middle";
        var taken = [];
        for (i = 0; i < levels.length; i++) taken.push(Y(levels[i].price));
        if (nowPrice != null) taken.push(Y(nowPrice));
        var steps = 5;
        for (i = 0; i <= steps; i++) {
            var gp = v.lo + (v.hi - v.lo) * i / steps;
            var gy = Y(gp);
            g.strokeStyle = color("--grid");
            g.lineWidth = 1;
            g.beginPath();
            g.moveTo(0, gy);
            g.lineTo(plotW, gy);
            g.stroke();
            var busyRow = false;
            var k;
            for (k = 0; k < taken.length; k++) if (Math.abs(taken[k] - gy) < 14) busyRow = true;
            if (busyRow) continue;
            g.fillStyle = color("--hint");
            g.textAlign = "left";
            g.fillText(gp.toFixed(dp), plotW + 6, gy);
        }
        // Time axis.
        var tstep = span > 1800000 ? 300000 : span > 600000 ? 120000 : span > 180000 ? 60000 : span > 60000 ? 15000 : span > 20000 ? 5000 : 2000;
        g.textAlign = "center";
        // Drawn label and tag boxes in CSS pixels, kept for the preview harness.
        var drawn = { times: [], tags: [] };
        cv.drawnBoxes = drawn;
        var lastRight = -Infinity;
        var tt;
        for (tt = Math.ceil(v.from / tstep) * tstep; tt < v.to; tt += tstep) {
            var tx = X(tt);
            g.strokeStyle = color("--grid");
            g.beginPath();
            g.moveTo(tx, 0);
            g.lineTo(tx, plotH);
            g.stroke();
            g.fillStyle = color("--hint");
            var timeLabel = span > 600000 ? clockAt(tt).slice(0, 5) : clockAt(tt);
            var halfTime = g.measureText(timeLabel).width / 2;
            var lx = Math.max(halfTime, Math.min(plotW - halfTime, tx));
            // A label crowding the previous one is skipped; its grid line stays.
            if (lx - halfTime < lastRight + 8) continue;
            lastRight = lx + halfTime;
            g.fillText(timeLabel, lx, H - 8);
            drawn.times.push({ x: lx - halfTime, y: H - 14, w: halfTime * 2, h: 12 });
        }
        // Volume: server buckets when given (raw, before thinning), else the visible prints.
        if (!this.opts.noVolume) {
            var vol = this.opts.volume;
            var bucket = Math.max(250, span / 90);
            if (vol && vol.width > 0) bucket = Math.max(1, Math.ceil(span / 90 / vol.width)) * vol.width;
            var sums = {};
            var maxV = 0;
            var add = function (start, buy, sell) {
                var key = Math.floor(start / bucket);
                var cell = sums[key] || (sums[key] = [0, 0]);
                cell[0] += buy || 0;
                cell[1] += sell || 0;
            };
            if (vol) {
                for (i = 0; i < vol.rows.length; i++) {
                    var row = vol.rows[i];
                    if (Array.isArray(row) && row[0] >= v.from - bucket && row[0] <= v.to) add(row[0], row[1], row[2]);
                }
            } else {
                for (i = 0; i < vis.length; i++) {
                    if (vis[i][3] === 1) add(vis[i][0], vis[i][2], 0);
                    else add(vis[i][0], 0, vis[i][2]);
                }
            }
            var key;
            for (key in sums) {
                if (Object.prototype.hasOwnProperty.call(sums, key)) maxV = Math.max(maxV, sums[key][0] + sums[key][1]);
            }
            if (maxV > 0) {
                var bw = Math.max(1, bucket / span * plotW - 1);
                var base = plotH + 2 + CHART_VOL_H - 4;
                g.globalAlpha = 0.55;
                for (key in sums) {
                    if (!Object.prototype.hasOwnProperty.call(sums, key)) continue;
                    var bx = X(key * bucket);
                    var hb = sums[key][0] / maxV * (CHART_VOL_H - 6);
                    var hs = sums[key][1] / maxV * (CHART_VOL_H - 6);
                    g.fillStyle = color("--tick-buy");
                    g.fillRect(bx, base - hb, bw, hb);
                    g.fillStyle = color("--tick-sell");
                    g.fillRect(bx, base - hb - hs, bw, hs);
                }
                g.globalAlpha = 1;
            }
        }
        // Order levels, pass 1: the dashed lines, under the prints.
        for (i = 0; i < levels.length; i++) {
            var ly = Y(levels[i].price);
            if (ly < 0 || ly > plotH) continue;
            g.strokeStyle = color(levels[i].color);
            g.setLineDash([5, 4]);
            g.lineWidth = 1.2;
            g.beginPath();
            g.moveTo(0, ly);
            g.lineTo(plotW, ly);
            g.stroke();
            g.setLineDash([]);
        }
        // Price line, then the dots, radius by the square root of the quantity.
        g.strokeStyle = color("--sub");
        g.globalAlpha = 0.45;
        g.lineWidth = 1;
        g.beginPath();
        for (i = 0; i < vis.length; i++) {
            if (i) g.lineTo(X(vis[i][0]), Y(vis[i][1]));
            else g.moveTo(X(vis[i][0]), Y(vis[i][1]));
        }
        g.stroke();
        var maxQ = 0;
        for (i = 0; i < vis.length; i++) if (finiteNumber(vis[i][2]) && vis[i][2] > maxQ) maxQ = vis[i][2];
        g.globalAlpha = 0.8;
        for (i = 0; i < vis.length; i++) {
            var q = finiteNumber(vis[i][2]) && maxQ > 0 ? vis[i][2] / maxQ : 0;
            g.fillStyle = color(vis[i][3] === 1 ? "--tick-buy" : "--tick-sell");
            g.beginPath();
            g.arc(X(vis[i][0]), Y(vis[i][1]), 1.2 + Math.sqrt(Math.max(0, q)) * 4, 0, Math.PI * 2);
            g.fill();
        }
        g.globalAlpha = 1;
        // Entry and exit markers.
        for (i = 0; i < this.marks.length; i++) {
            var m = this.marks[i];
            var mx = X(m.t);
            var my = Y(m.price);
            if (mx < -10 || mx > plotW + 10) continue;
            g.fillStyle = color(m.color);
            g.strokeStyle = color("--card");
            g.lineWidth = 2;
            g.beginPath();
            if (m.up) {
                g.moveTo(mx, my + 4);
                g.lineTo(mx - 7, my + 16);
                g.lineTo(mx + 7, my + 16);
            } else {
                g.moveTo(mx, my - 4);
                g.lineTo(mx - 7, my - 16);
                g.lineTo(mx + 7, my - 16);
            }
            g.closePath();
            g.stroke();
            g.fill();
        }
        // Hold span.
        if (this.opts.hold) {
            var h0 = X(this.opts.hold[0]);
            var h1 = X(this.opts.hold[1]);
            g.fillStyle = color("--tint");
            g.fillRect(h0, 0, Math.max(1, h1 - h0), plotH);
        }
        // Levels, pass 2: the price as a tag on the axis, the name at the left edge, and an arrow
        // chip at the right edge for a level outside the price range.
        var off = 0;
        var tagRows = [];
        // Move only the axis tag, away from a colliding tag toward its own price side; the line
        // and execution marker retain their true price. At the plot edge the push flips.
        function tagY(y0) {
            var top = 8;
            var bottom = plotH - 8;
            var y = Math.max(top, Math.min(bottom, y0));
            var guard = 2 * tagRows.length + 2;
            var j;
            for (j = 0; j < tagRows.length && guard > 0; j++) {
                var row = tagRows[j];
                if (Math.abs(y - row.y) >= 16) continue;
                var dir = y0 < row.at ? -1 : 1;
                y = row.y + dir * 16;
                if (y < top) y = row.y + 16;
                else if (y > bottom) y = row.y - 16;
                y = Math.max(top, Math.min(bottom, y));
                guard -= 1;
                j = -1;
            }
            tagRows.push({ y: y, at: y0 });
            drawn.tags.push({ x: plotW, y: y - 8, w: axisW, h: 16 });
            return y;
        }
        for (i = 0; i < levels.length; i++) {
            var lv = levels[i];
            var y = Y(lv.price);
            if (y < 0 || y > plotH) {
                var up = y < 0;
                var lab = (up ? "↑ " : "↓ ") + lv.label + " " + lv.price.toFixed(dp);
                g.font = "600 10px " + ui;
                var tw = g.measureText(lab).width + 10;
                var cy = up ? 10 + off * 18 : plotH - 10 - off * 18;
                var cx = plotW - tw - 4;
                off += 1;
                g.fillStyle = color("--card-high");
                g.fillRect(cx, cy - 8, tw, 16);
                g.strokeStyle = color(lv.color);
                g.lineWidth = 1;
                g.strokeRect(cx + 0.5, cy - 7.5, tw - 1, 15);
                g.fillStyle = color(lv.color);
                g.textAlign = "left";
                g.fillText(lab, cx + 5, cy);
                continue;
            }
            g.fillStyle = color(lv.color);
            var tagRow = tagY(y);
            g.fillRect(plotW, tagRow - 8, axisW, 16);
            g.fillStyle = color("--card");
            g.textAlign = "left";
            g.font = "600 10px " + mono;
            g.fillText(lv.price.toFixed(dp), plotW + 4, tagRow);
            g.font = "600 10px " + ui;
            var nw = g.measureText(lv.label).width + 8;
            g.fillStyle = color("--card");
            g.globalAlpha = 0.9;
            g.fillRect(2, y - 15, nw, 13);
            g.globalAlpha = 1;
            g.fillStyle = color(lv.color);
            g.fillText(lv.label, 6, y - 8);
        }
        // The current price, neutral so it never reads as an order level.
        if (nowPrice != null) {
            var py = Y(nowPrice);
            if (py >= 0 && py <= plotH) {
                py = tagY(py);
                g.fillStyle = color("--ink");
                g.fillRect(plotW, py - 8, axisW, 16);
                g.fillStyle = color("--card");
                g.textAlign = "left";
                g.font = "600 10px " + mono;
                g.fillText(nowPrice.toFixed(dp), plotW + 4, py);
            }
        }
        // Crosshair on the print nearest the tap.
        var tip = this.tip;
        var best = null;
        if (this.cross && this.cross.x < plotW) {
            var at = v.from + this.cross.x / plotW * span;
            var gap = Infinity;
            for (i = 0; i < vis.length; i++) {
                var dd = Math.abs(vis[i][0] - at);
                if (dd < gap) {
                    gap = dd;
                    best = vis[i];
                }
            }
        }
        if (!best) {
            tip.hidden = true;
            return;
        }
        var cxp = X(best[0]);
        var cyp = Y(best[1]);
        g.strokeStyle = color("--hint");
        g.setLineDash([2, 3]);
        g.beginPath();
        g.moveTo(cxp, 0);
        g.lineTo(cxp, plotH);
        g.moveTo(0, cyp);
        g.lineTo(plotW, cyp);
        g.stroke();
        g.setLineDash([]);
        clear(tip);
        tip.appendChild(el("b", "num", best[1].toFixed(dp)));
        tip.appendChild(document.createTextNode(" · " + tr(best[3] === 1 ? "mini_chart_buy" : "mini_chart_sell")));
        tip.appendChild(el("br"));
        tip.appendChild(el("span", "num", clockAt(best[0]) + "." + millisTail(best[0])));
        tip.appendChild(document.createTextNode(" · " + tr("mini_chart_qty") + " "));
        tip.appendChild(el("span", "num", qtyText(best[2])));
        tip.hidden = false;
        var left = cxp + 10;
        if (left > plotW - 150) left = Math.max(4, cxp - 160);
        tip.style.left = left + "px";
        tip.style.top = Math.max(4, cyp - 46) + "px";
    };

    // A row of view chips; `extra` (the follow button) sits at the far end.
    function chartChips(host, items, active, onPick, extra) {
        var row = el("div", "chart-tf");
        var i;
        for (i = 0; i < items.length; i++) {
            (function (key, labelKey) {
                var chip = button("chart-chip", tr(labelKey), function () { onPick(key); });
                chip.setAttribute("data-range", key);
                row.appendChild(chip);
            })(items[i][0], items[i][1]);
        }
        if (extra) {
            row.appendChild(el("span", "chart-spacer"));
            row.appendChild(extra);
        }
        pressChip(row, active);
        host.appendChild(row);
        return row;
    }

    function pressChip(row, key) {
        var nodes = row.querySelectorAll("[data-range]");
        var i;
        for (i = 0; i < nodes.length; i++) {
            var on = nodes[i].getAttribute("data-range") === key;
            nodes[i].classList.toggle("on", on);
            nodes[i].setAttribute("aria-pressed", on ? "true" : "false");
        }
    }

    function chartSkeleton(host) {
        clear(host);
        var box = el("div", "chart-state chart-loading");
        box.setAttribute("aria-busy", "true");
        box.appendChild(el("div", "chart-skeleton"));
        box.appendChild(el("p", "sr-only", tr("mini_chart_loading")));
        host.appendChild(box);
    }

    // A dashed box in place of the chart: a title, a note, and «Retry» when `onRetry` is given.
    function chartState(host, title, note, onRetry) {
        clear(host);
        var box = el("div", "chart-state");
        box.appendChild(el("div", "chart-state-title", title));
        if (note) box.appendChild(el("div", "", note));
        if (onRetry) box.appendChild(button("chart-retry", tr("mini_retry"), onRetry));
        host.appendChild(box);
    }

    function chartLegend(host, items) {
        clear(host);
        var i;
        for (i = 0; i < items.length; i++) {
            var item = el("span", "");
            var mark = el("i", items[i].dot ? "dot" : "");
            if (items[i].dot) mark.style.background = "var(" + items[i].color + ")";
            else mark.style.borderColor = "var(" + items[i].color + ")";
            item.appendChild(mark);
            item.appendChild(document.createTextNode(tr(items[i].key)));
            host.appendChild(item);
        }
        host.hidden = !items.length;
    }

    // Levels on the wire by kind; a closed trade's stop is drawn but never stretches the scale.
    function chartLevels(levels, fitStop) {
        var names = {
            buy: ["mini_chart_level_buy", "--entry"],
            entry: ["mini_chart_level_entry", "--entry"],
            take: ["mini_chart_level_take", "--take"],
            stop: ["mini_chart_level_stop", "--stop"]
        };
        var out = [];
        var i;
        if (!Array.isArray(levels)) return out;
        for (i = 0; i < levels.length; i++) {
            var level = levels[i];
            var name = level && names[level.kind];
            if (!name || !finiteNumber(level.price)) continue;
            out.push({ kind: level.kind, label: tr(name[0]), price: level.price, color: name[1], fit: level.kind !== "stop" || fitStop });
        }
        return out;
    }

    function hasLevel(levels, kind) {
        var i;
        for (i = 0; i < levels.length; i++) if (levels[i].kind === kind) return true;
        return false;
    }

    function chartArea(host) {
        var area = el("div", "chart-area");
        host.appendChild(area);
        return area;
    }

    // Stop the previous sheet's chart and timers, empty the sheet and start a new generation.
    function resetSheet() {
        sheetToken += 1;
        if (sheetTimer) {
            clearTimeout(sheetTimer);
            sheetTimer = null;
        }
        if (liveSheet) {
            if (liveSheet.timer) clearTimeout(liveSheet.timer);
            liveSheet.timer = null;
            liveSheet.stopped = true;
            liveSheet = null;
        }
        if (sheetChart) {
            sheetChart.destroy();
            sheetChart = null;
        }
        if (sheet) clear(sheet);
        return sheetToken;
    }

    function sheetHead(coin, side, where) {
        var head = el("div", "sheet-head");
        var titleBox = el("div", "grow");
        var top = el("div", "order-top");
        var title = el("span", "name order-coin", coin || "");
        title.id = "sheet-title";
        top.appendChild(title);
        top.appendChild(el("span", "badge order-side " + sideClass(side), sideLabel(side)));
        titleBox.appendChild(top);
        titleBox.appendChild(el("div", "sub trade-meta", where.join(" · ")));
        head.appendChild(titleBox);
        var closeBtn = button("cmd text-btn sheet-close", tr("mini_close"), closeSheet);
        head.appendChild(closeBtn);
        sheet.appendChild(head);
        return closeBtn;
    }

    function whereOf(item) {
        var where = [];
        if (item.core_name) where.push(item.core_name);
        if (item.exchange) where.push(item.exchange);
        return where;
    }

    function showSheet(focus) {
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
        focus.focus();
        syncBackButton();
    }

    // Closed-trade rows: entry and exit with their times once the tape answered, the hold, entry
    // volume when the DTO has it, quantity, strategy, and the exit reason when the report has one.
    function tradeDetails(host, trade, data) {
        clear(host);
        var timed = !!(data && finiteNumber(data.open_ms) && finiteNumber(data.close_ms) && data.close_ms > 0);
        var entry = trade.entry_text || "—";
        var exit = trade.exit_text || "—";
        if (timed) {
            entry += " · " + clockAt(data.open_ms);
            exit += " · " + clockAt(data.close_ms);
        }
        detailLine(host, tr("mini_trade_entry"), entry);
        detailLine(host, tr("mini_trade_exit"), exit);
        detailLine(host, tr("mini_trade_duration"), fmtDuration(trade.duration_secs));
        if (trade.volume_text) detailLine(host, tr("mini_trade_volume"), trade.volume_text);
        detailLine(host, tr("mini_trade_qty"), trade.qty_text);
        detailLine(host, tr("mini_trade_strategy"), trade.strategy || (trade.manual ? tr("mini_trade_manual") : "—"));
        if (data && data.exit_reason) detailLine(host, tr("mini_chart_exit_reason"), data.exit_reason);
        if (!timed) detailLine(host, tr("mini_trade_closed"), trade.closed_text);
    }

    // The initial window of a closed trade for a range chip.
    function tradeView(range, data) {
        var open = data.open_ms;
        var close = Math.max(data.close_ms, open);
        if (range === "30") return [open - 30000, open + 30000];
        if (range === "300") {
            var mid = (open + close) / 2;
            return [mid - 300000, mid + 300000];
        }
        var pad = Math.max(10000, (close - open) * 0.2);
        return [open - pad, close + pad];
    }

    function sourceText(data) {
        var bits = [];
        var name = tr("mini_chart_source_" + String(data.source || ""));
        if (name) bits.push(name);
        if (data.exchange) bits.push(data.exchange);
        return bits;
    }

    // Closed trade: the result, the tick chart of the deal and the trade's details. The range
    // chips only move the view: the tape is fetched once for open-5 min .. close+5 min.
    function openTradeSheet(trade) {
        if (!sheet) return;
        hapticSelection();
        var token = resetSheet();
        sheet.classList.add("chart-sheet");
        var closeBtn = sheetHead(trade.coin, trade.side, whereOf(trade));
        var result = el("div", "row spread sheet-result");
        var profit = el("span", "");
        applyMoney(profit, "hero-value num", trade.profit_text, trade.profit);
        result.appendChild(profit);
        result.appendChild(tradePct(trade, "num"));
        sheet.appendChild(result);
        var state = { range: "deal", data: null, tries: 0 };
        var chips = chartChips(sheet, TRADE_RANGES, "deal", function (key) {
            state.range = key;
            pressChip(chips, key);
            if (sheetChart && state.data) {
                var span = tradeView(key, state.data);
                sheetChart.setView(span[0], span[1]);
                provenance();
            }
        });
        var area = chartArea(sheet);
        var legend = el("div", "chart-legend");
        legend.hidden = true;
        sheet.appendChild(legend);
        var source = el("div", "chart-provenance");
        sheet.appendChild(source);
        var details = el("div", "chart-details");
        sheet.appendChild(details);
        tradeDetails(details, trade, null);

        function provenance() {
            var data = state.data;
            if (!data || !sheetChart) return;
            var bits = sourceText(data);
            bits.push(trf("mini_chart_in_frame_" + pluralForm(sheetChart.shown), { n: sheetChart.shown }));
            bits.push(tr("mini_chart_dot_size"));
            if (finiteNumber(data.thinned_ms) && data.thinned_ms > 0) bits.push(tr("mini_chart_thinned"));
            source.textContent = bits.join(" · ");
        }
        function failed() {
            legend.hidden = true;
            source.textContent = "";
            chartState(area, tr("mini_chart_error"), tr("mini_chart_error_note"), function () {
                state.tries = 0;
                load();
            });
        }
        function draw(data) {
            state.data = data;
            clear(area);
            var wrap = el("div", "chart-wrap");
            area.appendChild(wrap);
            var ticks = tickList(data.ticks);
            var long = trade.side !== "sell";
            var volume = Array.isArray(data.volume) && finiteNumber(data.volume_bucket_ms) && data.volume_bucket_ms > 0
                ? { width: data.volume_bucket_ms, rows: data.volume }
                : null;
            sheetChart = new TickChart(wrap, {
                ticks: function () { return ticks; },
                dp: priceDecimals(data.price_decimals),
                maxSpan: Math.max(600000, (data.to_ms - data.from_ms) || 0),
                hold: [data.open_ms, Math.max(data.open_ms, data.close_ms)],
                volume: volume,
                // Thinned prints would undercount the histogram, so without buckets it stays empty.
                noVolume: !volume && finiteNumber(data.thinned_ms) && data.thinned_ms > 0,
                live: false,
                onView: provenance
            });
            sheetChart.levels = chartLevels(data.levels, false);
            if (finiteNumber(data.entry_price)) {
                sheetChart.marks.push({ t: data.open_ms, price: data.entry_price, up: long, color: long ? "--tick-buy" : "--tick-sell" });
            }
            if (finiteNumber(data.exit_price)) {
                sheetChart.marks.push({ t: data.close_ms, price: data.exit_price, up: !long, color: long ? "--tick-sell" : "--tick-buy" });
            }
            var span = tradeView(state.range, data);
            sheetChart.setView(span[0], span[1]);
            var items = [
                { dot: true, color: "--tick-buy", key: "mini_chart_tick_buy" },
                { dot: true, color: "--tick-sell", key: "mini_chart_tick_sell" }
            ];
            if (hasLevel(sheetChart.levels, "buy")) items.push({ color: "--entry", key: "mini_chart_buy_order" });
            if (hasLevel(sheetChart.levels, "take")) items.push({ color: "--take", key: "mini_chart_take" });
            if (hasLevel(sheetChart.levels, "stop")) items.push({ color: "--stop", key: "mini_chart_stop" });
            chartLegend(legend, items);
            provenance();
        }
        function fetchTicks() {
            api("/api/trade/ticks", { core: trade.core, rec_id: trade.rec_id, range: state.range }).then(function (res) {
                if (token !== sheetToken) return;
                // A command or tab reload dropped the queued read; the sheet still wants it.
                if (res.cancelled) {
                    state.tries += 1;
                    if (state.tries >= CHART_LOAD_TRIES) {
                        failed();
                        return;
                    }
                    sheetTimer = setTimeout(function () {
                        sheetTimer = null;
                        if (token === sheetToken) fetchTicks();
                    }, CHART_LOAD_WAIT_MS);
                    return;
                }
                if (!res.ok || !res.data) {
                    failed();
                    return;
                }
                var data = res.data;
                if (data.state === "loading") {
                    state.tries += 1;
                    if (state.tries >= CHART_LOAD_TRIES) {
                        failed();
                        return;
                    }
                    sheetTimer = setTimeout(function () {
                        sheetTimer = null;
                        if (token === sheetToken) fetchTicks();
                    }, finiteNumber(data.retry_in_ms) && data.retry_in_ms > 0 ? data.retry_in_ms : CHART_LOAD_WAIT_MS);
                    return;
                }
                tradeDetails(details, trade, data);
                if (data.state === "ok") {
                    draw(data);
                    return;
                }
                source.textContent = "";
                if (data.state === "no_tape") {
                    chartState(area, tr("mini_chart_no_tape"), tr("mini_chart_no_tape_note"), null);
                } else if (data.state === "archive_pending") {
                    chartState(area, tr("mini_chart_archive"), finiteNumber(data.retry_at_ms)
                        ? trf("mini_chart_archive_note", { time: whenText(data.retry_at_ms) }) : null, null);
                } else {
                    failed();
                }
            });
        }
        function load() {
            chartSkeleton(area);
            legend.hidden = true;
            source.textContent = tr("mini_chart_loading");
            fetchTicks();
        }
        showSheet(closeBtn);
        load();
    }

    function liveDetail(host, label, value, tone) {
        var line = el("div", "row spread");
        line.appendChild(el("span", "k", label));
        line.appendChild(el("span", "num" + (tone ? " " + tone : ""), value == null || value === "" ? "—" : value));
        host.appendChild(line);
    }

    // Open position: live PnL, a tick chart fed every 500 ms and the order's own actions. The
    // first read takes the last 15 minutes; later reads only prints after the newest one held.
    function openOrderSheet(order, data) {
        if (!sheet) return;
        hapticSelection();
        var token = resetSheet();
        sheet.classList.add("chart-sheet");
        var live = {
            timer: null,
            inflight: false,
            stopped: false,
            frozen: false,
            ring: [],
            last: null,
            offset: 0,
            okAt: 0,
            closingTries: 0,
            span: 300000,
            data: null,
            poll: poll
        };
        liveSheet = live;
        var closeBtn = sheetHead((order.coin || "") + (order.emulator ? " (E)" : ""), order.side, whereOf(order));
        var banner = el("div", "chart-banner");
        banner.hidden = true;
        sheet.appendChild(banner);
        var hero = el("div", "row spread sheet-result chart-hero");
        var pnl = el("span", "");
        applyMoney(pnl, "hero-value num", order.pnl_text, order.pnl_sign);
        hero.appendChild(pnl);
        var tag = el("span", "live-tag");
        hero.appendChild(tag);
        sheet.appendChild(hero);
        var follow = button("chart-chip chart-follow on", tr("mini_chart_follow"), function () {
            if (!sheetChart) return;
            sheetChart.follow = true;
            sheetChart.anchor(sheetChart.view.to - sheetChart.view.from);
            sheetChart.fitY();
            sheetChart.draw();
            syncFollow();
        });
        follow.setAttribute("aria-pressed", "true");
        var chips = chartChips(sheet, LIVE_SPANS, "300", function (key) {
            live.span = +key * 1000;
            pressChip(chips, key);
            if (!sheetChart) return;
            if (live.frozen) {
                var end = sheetChart.view.to;
                sheetChart.setView(end - live.span, end);
                return;
            }
            sheetChart.follow = true;
            sheetChart.anchor(live.span);
            sheetChart.fitY();
            sheetChart.draw();
            syncFollow();
        }, follow);
        var area = chartArea(sheet);
        var legend = el("div", "chart-legend");
        legend.hidden = true;
        sheet.appendChild(legend);
        var source = el("div", "chart-provenance");
        sheet.appendChild(source);
        var details = el("div", "chart-details");
        sheet.appendChild(details);
        var actions = el("div", "chart-actions");
        // The same controls the order row carries: same handlers, same request bodies.
        if (data && data.can_control) orderActions(actions, order);
        sheet.appendChild(actions);

        function now() {
            return Date.now() + live.offset;
        }
        function syncFollow() {
            var on = !!(sheetChart && sheetChart.follow);
            follow.classList.toggle("on", on);
            follow.setAttribute("aria-pressed", on ? "true" : "false");
        }
        function setTag(text, stale) {
            clear(tag);
            tag.className = "live-tag" + (stale ? " stale" : "");
            tag.appendChild(el("span", "live-dot"));
            tag.appendChild(document.createTextNode(text));
        }
        function staleText(seconds) {
            if (!finiteNumber(seconds)) return tr("mini_chart_offline");
            var n = Math.max(1, Math.round(seconds));
            return trf("mini_chart_stale_" + pluralForm(n), { n: n });
        }
        function next(ms) {
            if (token !== sheetToken || live.stopped) return;
            if (live.timer) clearTimeout(live.timer);
            live.timer = setTimeout(poll, ms);
        }
        function poll() {
            live.timer = null;
            if (token !== sheetToken || live.stopped || live.inflight) return;
            // A hidden pane stops polling; the visibility handler resumes it.
            if (document.visibilityState === "hidden") return;
            var body = { core: order.core, uid: order.uid };
            if (live.last != null) body.since_ms = live.last;
            live.inflight = true;
            api("/api/order/ticks", body).then(function (res) {
                if (token !== sheetToken) return;
                live.inflight = false;
                if (res.cancelled) {
                    next(CHART_POLL_MS);
                    return;
                }
                if (!res.ok || !res.data) {
                    failed(res);
                    return;
                }
                received(res.data);
            });
        }
        function failed(res) {
            if (res.status === 401 || res.status === 403 || !live.okAt) {
                live.stopped = res.status === 401 || res.status === 403;
                if (sheetChart) {
                    sheetChart.destroy();
                    sheetChart = null;
                }
                legend.hidden = true;
                source.textContent = "";
                setTag(tr("mini_chart_offline"), true);
                chartState(area, tr("mini_chart_error"), res.error || tr("mini_chart_error_note"), live.stopped ? null : function () {
                    chartSkeleton(area);
                    poll();
                });
                return;
            }
            // Data on screen stays; the tag counts the time since the last good reply.
            if (!live.frozen) setTag(staleText((Date.now() - live.okAt) / 1000), true);
            next(CHART_SLOW_POLL_MS);
        }
        // An inclusive cursor repeats the boundary millisecond; dedupe only identical prints.
        function merge(ticks, truncated) {
            var list = tickList(ticks);
            if (truncated) {
                live.ring = [];
                live.last = null;
            }
            var ring = live.ring;
            var i;
            for (i = 0; i < list.length; i++) {
                var tick = list[i];
                if (live.last != null && tick[0] < live.last) continue;
                var duplicate = false;
                var j;
                for (j = ring.length - 1; j >= 0 && ring[j][0] === tick[0]; j--) {
                    if (ring[j][1] === tick[1] && ring[j][2] === tick[2] && ring[j][3] === tick[3]) {
                        duplicate = true;
                        break;
                    }
                }
                if (duplicate) continue;
                ring.push(list[i]);
                live.last = list[i][0];
            }
            var cut = now() - LIVE_KEEP_MS;
            var drop = 0;
            while (drop < ring.length && ring[drop][0] < cut) drop += 1;
            if (ring.length - drop > LIVE_RING) drop = ring.length - LIVE_RING;
            if (drop) ring.splice(0, drop);
        }
        function ensureChart(d) {
            if (sheetChart) return;
            clear(area);
            var wrap = el("div", "chart-wrap");
            area.appendChild(wrap);
            sheetChart = new TickChart(wrap, {
                ticks: function () { return live.ring; },
                dp: priceDecimals(d.price_decimals),
                maxSpan: LIVE_MAX_SPAN_MS,
                hold: null,
                volume: null,
                live: true,
                now: now,
                onView: syncFollow
            });
            sheetChart.follow = true;
            sheetChart.anchor(live.span);
        }
        function entryMark(d) {
            var long = order.side !== "sell";
            if (!finiteNumber(d.entry_ms) || d.entry_ms <= 0 || !finiteNumber(d.entry_price)) return [];
            return [{ t: d.entry_ms, price: d.entry_price, up: long, color: long ? "--tick-buy" : "--tick-sell" }];
        }
        function openDetails(d) {
            clear(details);
            var dp = priceDecimals(d.price_decimals);
            var entry = finiteNumber(d.entry_price) ? d.entry_price.toFixed(dp) : order.entry_text;
            if (finiteNumber(d.entry_ms) && d.entry_ms > 0) {
                var held = fmtDuration(Math.max(0, (now() - d.entry_ms) / 1000));
                if (held) entry += " · " + trf("mini_chart_ago", { t: held });
            }
            liveDetail(details, tr("mini_trade_entry"), entry);
            var mark = finiteNumber(d.mark_price) ? d.mark_price.toFixed(dp) : order.mark_text;
            if (d.change_text) mark += " · " + d.change_text;
            liveDetail(details, tr("mini_chart_now"), mark, signClass(d.pnl_sign));
            if (d.take_dist_text) liveDetail(details, tr("mini_chart_to_take"), d.take_dist_text);
            if (d.stop_dist_text) liveDetail(details, tr("mini_chart_to_stop"), d.stop_dist_text);
            if (order.volume_text) liveDetail(details, tr("mini_trade_volume"), order.volume_text);
        }
        function openLegend(levels) {
            var items = [
                { dot: true, color: "--tick-buy", key: "mini_chart_buy" },
                { dot: true, color: "--tick-sell", key: "mini_chart_sell" }
            ];
            if (hasLevel(levels, "entry")) items.push({ color: "--entry", key: "mini_chart_entry" });
            if (hasLevel(levels, "take")) items.push({ color: "--take", key: "mini_chart_sell_order" });
            if (hasLevel(levels, "stop")) items.push({ color: "--stop", key: "mini_chart_stop" });
            if (!live.frozen) items.push({ dot: true, color: "--ink", key: "mini_chart_price_now" });
            chartLegend(legend, items);
        }
        function received(d) {
            live.okAt = Date.now();
            if (finiteNumber(d.now_ms)) live.offset = d.now_ms - Date.now();
            live.data = d;
            if (d.state === "closed") {
                closed(d);
                return;
            }
            // A closing position's chart is frozen: later prints are not its own.
            if (!live.frozen) merge(d.ticks, d.truncated);
            if (d.pnl_text) applyMoney(pnl, "hero-value num", d.pnl_text, d.pnl_sign);
            openDetails(d);
            if (!live.ring.length && (d.state === "no_tape" || d.state === "closing")) {
                setTag(tr("mini_chart_offline"), true);
                legend.hidden = true;
                source.textContent = "";
                if (sheetChart) {
                    sheetChart.destroy();
                    sheetChart = null;
                }
                chartState(area, tr("mini_chart_live_no_tape"), tr("mini_chart_live_no_tape_note"), null);
                if (d.state === "closing") closing();
                else next(CHART_SLOW_POLL_MS);
                return;
            }
            ensureChart(d);
            if (!live.frozen) {
                sheetChart.levels = chartLevels(d.levels, true);
                sheetChart.marks = entryMark(d);
                sheetChart.mark = finiteNumber(d.mark_price) ? d.mark_price : null;
                if (sheetChart.follow) sheetChart.anchor(sheetChart.view.to - sheetChart.view.from);
                sheetChart.fitY();
                sheetChart.draw();
                openLegend(sheetChart.levels);
            }
            if (d.state === "closing") {
                closing();
                return;
            }
            var stale = d.state !== "live";
            if (stale) {
                var lag = finiteNumber(d.last_tick_ms) && finiteNumber(d.now_ms) ? (d.now_ms - d.last_tick_ms) / 1000 : null;
                setTag(staleText(lag), true);
                source.textContent = finiteNumber(d.last_tick_ms)
                    ? trf("mini_chart_stale_note", { time: clockAt(d.last_tick_ms) })
                    : tr("mini_chart_offline");
            } else {
                setTag(tr("mini_chart_live"), false);
                source.textContent = tr("mini_chart_live_stream");
            }
            next(CHART_POLL_MS);
        }
        function freeze() {
            live.frozen = true;
            follow.hidden = true;
            tag.hidden = true;
            if (sheetChart) {
                sheetChart.follow = false;
                sheetChart.opts.live = false;
                sheetChart.draw();
            }
        }
        function closing() {
            freeze();
            live.closingTries += 1;
            banner.hidden = false;
            if (live.closingTries > CHART_CLOSING_TRIES) {
                banner.textContent = tr("mini_chart_closing_late");
                live.stopped = true;
                return;
            }
            banner.textContent = tr("mini_chart_closing");
            next(CHART_SLOW_POLL_MS);
        }
        function closed(d) {
            var trade = d.trade;
            freeze();
            live.stopped = true;
            banner.hidden = false;
            if (!trade || typeof trade !== "object") {
                banner.textContent = tr("mini_chart_closing_late");
                return;
            }
            var closeMs = finiteNumber(trade.close_ms) ? trade.close_ms : finiteNumber(trade.closed_at) ? trade.closed_at * 1000 : null;
            banner.textContent = closeMs != null ? trf("mini_chart_closed", { time: clockAt(closeMs) }) : tr("mini_chart_closing_late");
            applyMoney(pnl, "hero-value num", trade.profit_text, trade.profit);
            hero.appendChild(tradePct(trade, "num"));
            clear(actions);
            // The live reply knows when the position opened; the trade knows when it closed.
            var times = finiteNumber(d.entry_ms) && d.entry_ms > 0 && closeMs != null ? { open_ms: d.entry_ms, close_ms: closeMs } : null;
            tradeDetails(details, trade, times);
            if (closeMs != null && live.ring.length) {
                // Prints after the close are not this position's; the closing second stays.
                while (live.ring.length && live.ring[live.ring.length - 1][0] > closeMs + 1000) live.ring.pop();
            }
            if (!sheetChart && live.ring.length) ensureChart(d);
            if (!sheetChart) chartState(area, tr("mini_chart_no_tape"), null, null);
            if (sheetChart) {
                var long = order.side !== "sell";
                var exitPrice = finiteNumber(trade.exit_price) ? trade.exit_price : parseFloat(String(trade.exit_text || "").replace(/[\s ]/g, ""));
                var keep = [];
                var i;
                for (i = 0; i < sheetChart.levels.length; i++) if (sheetChart.levels[i].kind !== "stop") keep.push(sheetChart.levels[i]);
                sheetChart.levels = keep;
                if (closeMs != null && finiteNumber(exitPrice)) {
                    sheetChart.marks.push({ t: closeMs, price: exitPrice, up: !long, color: long ? "--tick-sell" : "--tick-buy" });
                }
                var start = sheetChart.marks.length && sheetChart.marks[0].t;
                if (closeMs != null && start) sheetChart.opts.hold = [start, closeMs];
                sheetChart.follow = false;
                sheetChart.opts.live = false;
                sheetChart.opts.now = null;
                if (closeMs != null) sheetChart.setView(closeMs - live.span * 0.85, closeMs + live.span * 0.15);
                else sheetChart.draw();
                var items = [
                    { dot: true, color: "--tick-buy", key: "mini_chart_buy" },
                    { dot: true, color: "--tick-sell", key: "mini_chart_sell" }
                ];
                if (hasLevel(keep, "entry")) items.push({ color: "--entry", key: "mini_chart_entry" });
                if (hasLevel(keep, "take")) items.push({ color: "--take", key: "mini_chart_take" });
                chartLegend(legend, items);
            }
            source.textContent = tr("mini_chart_frozen");
        }
        setTag(tr("mini_chart_live"), false);
        chartSkeleton(area);
        source.textContent = tr("mini_chart_loading");
        showSheet(closeBtn);
        poll();
    }

    // The live sheet polls only while the page is visible.
    function chartVisibility() {
        var live = liveSheet;
        if (!live || live.stopped) return;
        if (document.visibilityState !== "visible") {
            if (live.timer) clearTimeout(live.timer);
            live.timer = null;
            return;
        }
        if (!live.timer && !live.inflight) live.poll();
    }

    function closeSheet() {
        if (!sheetOpen) return;
        sheetOpen = false;
        resetSheet();
        if (sheet) {
            sheet.hidden = true;
            sheet.classList.remove("chart-sheet");
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
            m: settingsMinutesCanon(form.minutesText)
        });
    }

    /** Validate `form`'s enabled thresholds, scope, and delay before allowing Save. */
    function settingsFormValid(form) {
        if (!form) return false;
        if (form.volumeOn && !settingsAmountOk(form.volumeText)) return false;
        if (form.profitOn && !settingsAmountOk(form.profitText)) return false;
        if (form.lossOn && !settingsAmountOk(form.lossText)) return false;
        if (!settingsMinutesOk(form.minutesText)) return false;
        if (form.scopeKind === "only" && !form.scopeIds.length) return false;
        return true;
    }

    /** Return the wire rules for a valid `form`; disabled thresholds become null. */
    function settingsPayload(form) {
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
            }
        };
    }

    // Return a non-negative safe-integer revision, or 0 when it cannot round-trip exactly.
    function settingsRevisionOf(data) {
        var value = data && data.revision;
        if (typeof value !== "number" || value !== value || value < 0 || value > 9007199254740991 || Math.floor(value) !== value) return 0;
        return value;
    }

    // Adopt server settings, cores and revision as the draft baseline; false leaves it unchanged.
    function settingsAdopt(data) {
        var settings = data && data.settings;
        var trades = settings && settings.trades;
        var down = settings && settings.down;
        if (!trades || !down) return false;
        var scope = trades.cores || {};
        var ids = [];
        if (scope.kind === "only" && Array.isArray(scope.ids)) {
            var i;
            for (i = 0; i < scope.ids.length; i++) {
                var id = scope.ids[i];
                if (typeof id === "number" && id === id && id !== Infinity && id !== -Infinity) ids.push(id);
            }
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
            minutesText: typeof minutes === "number" ? String(minutes) : "5"
        };
        settingsForm = form;
        settingsCores = Array.isArray(data.cores) ? data.cores : [];
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
            // Telegram's header back arrow already returns to the list; the text link is only
            // for a client without that button.
            if (!headerBackAvailable()) {
                var back = button("settings-back", tr("mini_back"), function () { settingsNavigate(null); });
                back.setAttribute("data-settings-back", "");
                host.appendChild(back);
            }
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
        chartVisibility();
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
        // A scope result: cores already in the asked state count as done; cores that are not
        // connected were skipped, never queued.
        var done = typeof data.sent === "number" ? data.sent + (data.already || 0) : null;
        var offline = data.offline || 0;
        // Partial when part of the scope was done, or part of it (not all) was offline: the
        // offline count is shown even when the rest was refused.
        var partial = done != null && typeof data.requested === "number"
            && done !== data.requested && (done > 0 || (offline > 0 && offline < data.requested));
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
            showCmdLine(offline
                ? trf("mini_cmd_partial_offline", { sent: done, n: data.requested, offline: offline })
                : trf("mini_cmd_partial", { sent: done, n: data.requested }));
            reloadAfterCommand(true);
            return;
        }
        var failed = tr("mini_cmd_failed");
        if (data.error === "offline") {
            failed = tr("mini_cmd_offline");
        } else if (data.error === "not_found") {
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
