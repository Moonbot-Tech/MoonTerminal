// Headless preview of the Telegram Mini App: renders every screen from fixture payloads and
// checks the owner commands the page sends. No core, no bot, no network.
//
//   node tools/miniapp_preview/preview.mjs [--out <dir>] [--locale ru|en|es] [--only <screen>]
//
// Exits non-zero when an interaction check fails or the page throws.

import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..", "..");
const WEB = path.join(ROOT, "crates", "moon-core", "src", "telegram", "web");
const LOCALES = path.join(ROOT, "locales");
const FIXTURES = path.join(HERE, "fixtures");

// Command-line options: output folder, label locale, one screen by name.
function args() {
    const out = { out: path.join(HERE, "out"), locale: "ru", only: null };
    const argv = process.argv.slice(2);
    for (let i = 0; i < argv.length; i++) {
        const key = argv[i];
        const value = argv[i + 1];
        if (key === "--out") out.out = path.resolve(value);
        else if (key === "--locale") out.locale = value;
        else if (key === "--only") out.only = value;
        else if (key === "--help" || key === "-h") {
            console.log("usage: node tools/miniapp_preview/preview.mjs [--out <dir>] [--locale ru|en|es] [--only <screen>]");
            process.exit(0);
        } else throw new Error(`unknown argument ${key}`);
        i++;
    }
    return out;
}

// playwright-core and js-yaml from the tool's own node_modules.
function loadDeps() {
    try {
        return { chromium: require("playwright-core").chromium, yaml: require("js-yaml") };
    } catch {
        console.error("[FAIL] dependencies missing: run `npm install --prefix tools/miniapp_preview` once");
        process.exit(2);
    }
}

// System Chrome; CHROME_PATH wins when set.
function chromePath() {
    const env = process.env.CHROME_PATH;
    if (env) return env;
    const candidates = {
        win32: [
            path.join(process.env["PROGRAMFILES"] || "C:/Program Files", "Google/Chrome/Application/chrome.exe"),
            path.join(process.env["PROGRAMFILES(X86)"] || "C:/Program Files (x86)", "Google/Chrome/Application/chrome.exe"),
            path.join(process.env.LOCALAPPDATA || "", "Google/Chrome/Application/chrome.exe"),
        ],
        darwin: ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"],
        linux: ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/usr/bin/chromium", "/usr/bin/chromium-browser"],
    }[process.platform] || [];
    const found = candidates.find((p) => p && fs.existsSync(p));
    if (!found) {
        console.error("[FAIL] Chrome not found; set CHROME_PATH to a Chrome or Chromium binary");
        process.exit(2);
    }
    return found;
}

// The labels the terminal hands the page: every `telegram.*` key, prefix stripped, in one locale.
function labels(yaml, locale) {
    const out = { locale };
    for (const file of fs.readdirSync(LOCALES).filter((f) => /^telegram.*\.yml$/.test(f)).sort()) {
        const doc = yaml.load(fs.readFileSync(path.join(LOCALES, file), "utf8")) || {};
        for (const [key, value] of Object.entries(doc)) {
            if (!key.startsWith("telegram.") || !value || typeof value !== "object") continue;
            const text = value[locale] ?? value.en;
            if (typeof text === "string") out[key.slice("telegram.".length)] = text;
        }
    }
    return out;
}

const fixture = (name) => JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf8"));

const VIEWPORTS = [
    { width: 421, height: 900 },
    { width: 390, height: 844 },
];
const THEMES = ["light", "dark"];
const OK = { ok: true, armed: null, error: null };

// One page with a stubbed Telegram.WebApp and every request answered from `api`.
async function openPage(browser, html, viewport, theme, api, log) {
    const context = await browser.newContext({ viewport, deviceScaleFactor: 2 });
    const page = await context.newPage();
    page.on("pageerror", (error) => log.errors.push(String(error)));
    page.on("console", (msg) => {
        if (msg.type() === "error") log.errors.push(msg.text());
    });
    await page.addInitScript((scheme) => {
        const noop = () => {};
        window.Telegram = {
            WebApp: {
                colorScheme: scheme,
                initData: "preview",
                ready: noop,
                expand: noop,
                onEvent: noop,
                BackButton: { show: noop, hide: noop, onClick: noop, offClick: noop },
                showConfirm: (_text, done) => done(true),
            },
        };
    }, theme);
    await page.route("**/*", async (route) => {
        const request = route.request();
        const url = new URL(request.url());
        if (url.hostname === "telegram.org") return route.fulfill({ body: "", contentType: "text/javascript" });
        if (url.pathname === "/") return route.fulfill({ body: html, contentType: "text/html" });
        if (url.pathname === "/app.css") return route.fulfill({ path: path.join(WEB, "app.css"), contentType: "text/css" });
        if (url.pathname === "/app.js") return route.fulfill({ path: path.join(WEB, "app.js"), contentType: "text/javascript" });
        const raw = request.postData() || "";
        log.sent.push({ path: url.pathname, raw });
        const handler = api[url.pathname];
        if (!handler) return route.fulfill({ status: 404, body: "" });
        const reply = typeof handler === "function" ? handler(raw) : { body: handler };
        return route.fulfill({
            status: reply.status || 200,
            body: JSON.stringify(reply.body ?? {}),
            contentType: "application/json",
        });
    });
    await page.goto("http://miniapp.preview/");
    return { page, context };
}

// Every read route answered from the fixtures; commands succeed.
function defaultApi() {
    return {
        "/api/session": { ok: true },
        "/api/report": (raw) => {
            const period = (JSON.parse(raw || "{}").period) || "today";
            return { body: fixture(period === "month" || period === "last_month" ? "report_month" : "report_today") };
        },
        "/api/cores": fixture("cores"),
        "/api/balances": fixture("balances"),
        "/api/orders": fixture("orders"),
        "/api/trades": fixture("trades"),
        "/api/strategies": fixture("strategies"),
        "/api/strategy/toggle": OK,
        "/api/order/cancel": OK,
    };
}

const settle = (page) => page.waitForTimeout(350);
const nav = async (page, tab) => {
    await page.click(`#app-nav button[data-tab="${tab}"]`);
    await settle(page);
};
const openFirstGroup = async (page, tab) => {
    await page.waitForSelector(`section[data-tab="${tab}"] .group-head`);
    await page.click(`section[data-tab="${tab}"] .group-head`);
    await settle(page);
};

const openTrade = async (page, nth) => {
    await nav(page, "deals"); await page.click('[data-seg="trades"]');
    const head = await page.$('section[data-tab="trades"] .group-head');
    if (head) { await head.click(); await settle(page); }
    await page.locator('section[data-tab="trades"] .trade-row').nth(nth).click(); await settle(page);
};

// Each screen starts from a fresh page; `api` overrides replace fixture routes.
const SCREENS = [
    { name: "report-today", run: async () => {} },
    { name: "report-month", run: async (p) => { await p.click('[data-period="month"]'); await settle(p); } },
    { name: "cores", run: async (p) => { await nav(p, "cores"); } },
    { name: "cores-group-open", run: async (p) => { await nav(p, "cores"); await openFirstGroup(p, "cores"); } },
    { name: "core-detail", run: async (p) => {
        await nav(p, "cores"); await openFirstGroup(p, "cores");
        await p.click('section[data-tab="cores"] .core-row'); await settle(p);
    } },
    { name: "trades-open-group-open", run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="orders"]'); await openFirstGroup(p, "orders");
    } },
    { name: "trades-closed", run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="trades"]');
        await p.waitForSelector('section[data-tab="trades"] .trade-row, section[data-tab="trades"] .group-head');
        await settle(p);
    } },
    { name: "trade-sheet", run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="trades"]');
        const head = await p.$('section[data-tab="trades"] .group-head');
        if (head) { await head.click(); await settle(p); }
        await p.click('section[data-tab="trades"] .trade-row'); await settle(p);
    } },
    // Row 2 names its strategy, row 6 carries none and is not marked manual (row 1 is manual).
    { name: "trade-sheet-strategy", run: async (p) => { await openTrade(p, 1); } },
    { name: "trade-sheet-unknown", run: async (p) => { await openTrade(p, 5); } },
    { name: "strategies-open", run: async (p) => { await nav(p, "strategies"); await openFirstGroup(p, "strategies"); } },
    { name: "balances-masked", run: async (p) => { await nav(p, "balances"); await openFirstGroup(p, "balances"); } },
    { name: "balances-unmasked", run: async (p) => {
        await nav(p, "balances"); await p.click(".eye-btn"); await openFirstGroup(p, "balances");
    } },
    { name: "empty-orders", api: { "/api/orders": { orders: [], can_control: true } }, run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="orders"]'); await settle(p);
    } },
    { name: "empty-strategies", api: { "/api/strategies": { cores: [], can_control: true } }, run: async (p) => { await nav(p, "strategies"); } },
    { name: "error-cores", api: { "/api/cores": () => ({ status: 500, body: {} }) }, run: async (p) => { await nav(p, "cores"); await p.waitForTimeout(600); } },
    { name: "session-denied", api: { "/api/session": () => ({ status: 403, body: {} }) }, ready: "#startup", run: async () => {} },
];

// Every screen at every viewport and theme; returns the files and unexpected page errors.
async function shoot(browser, html, opts) {
    const shots = [];
    const log = { errors: [], sent: [] };
    for (const screen of SCREENS) {
        if (opts.only && screen.name !== opts.only) continue;
        for (const viewport of VIEWPORTS) for (const theme of THEMES) {
            const api = Object.assign(defaultApi(), screen.api || {});
            const { page, context } = await openPage(browser, html, viewport, theme, api, log);
            await page.waitForSelector(screen.ready || "#app-nav:not([hidden])");
            await settle(page);
            await screen.run(page);
            const file = path.join(opts.out, `${screen.name}-${viewport.width}x${viewport.height}-${theme}.png`);
            await page.screenshot({ path: file });
            shots.push(file);
            await context.close();
        }
    }
    // A 500 on one route is part of the error screen, not a page fault.
    return { shots, errors: log.errors.filter((e) => !/status of 500|status of 403/.test(e)) };
}

// Owner commands: the exact bytes the page sends, and the line it shows on a refusal.
async function interactions(browser, html, texts) {
    const failures = [];
    const expect = (cond, what) => { if (!cond) failures.push(what); };
    const log = { errors: [], sent: [] };
    const strategies = fixture("strategies");
    const core = strategies.cores[0];
    const target = core.folders[0].strategies[0];
    const orders = fixture("orders");
    const order = orders.orders[0];
    let reject = false;
    const api = Object.assign(defaultApi(), {
        "/api/strategies": { cores: [core], can_control: true },
        "/api/orders": { orders: [order], can_control: true },
        "/api/strategy/toggle": () => (reject ? { status: 400, body: { error: "json" } } : { body: OK }),
    });
    const { page, context } = await openPage(browser, html, VIEWPORTS[0], "light", api, log);
    await page.waitForSelector("#app-nav:not([hidden])");
    await nav(page, "strategies");
    await openFirstGroup(page, "strategies");
    const pill = `section[data-tab="strategies"] button.pill[aria-label="${target.name}"]`;
    await page.click(pill);
    await page.waitForTimeout(500);
    const toggles = log.sent.filter((s) => s.path === "/api/strategy/toggle");
    const wanted = `{"core":${core.core},"id":"${target.id}","on":${!target.checked}}`;
    expect(toggles.length === 1, `strategy toggle sent ${toggles.length} times, expected 1`);
    expect(toggles[0] && toggles[0].raw === wanted, `strategy toggle body ${toggles[0] && toggles[0].raw} != ${wanted}`);

    reject = true;
    await page.waitForSelector(`${pill}:not([disabled])`);
    await page.click(pill);
    await page.waitForTimeout(500);
    const line = await page.evaluate(() => {
        const node = document.getElementById("cmd-status");
        return node && !node.hidden ? node.textContent : null;
    });
    expect(line === texts.mini_cmd_failed, `400 reply shows ${JSON.stringify(line)}, expected ${JSON.stringify(texts.mini_cmd_failed)}`);
    reject = false;

    await nav(page, "deals");
    await page.click('[data-seg="orders"]');
    await openFirstGroup(page, "orders");
    await page.waitForSelector(".cmd-cancel:not([disabled])");
    await page.click(".cmd-cancel");
    await page.waitForTimeout(500);
    const cancels = log.sent.filter((s) => s.path === "/api/order/cancel");
    const cancelWanted = `{"core":${order.core},"uid":"${order.uid}"}`;
    expect(cancels.length === 1 && cancels[0].raw === cancelWanted,
        `order cancel sent ${JSON.stringify(cancels.map((c) => c.raw))}, expected [${cancelWanted}]`);
    await context.close();
    // The refused toggle's own 400 is logged by the browser; that one is expected.
    for (const error of log.errors.filter((e) => !/status of 400/.test(e))) failures.push(`page error: ${error}`);
    return failures;
}

// Render, then check; the exit code is the verdict.
async function main() {
    const opts = args();
    const { chromium, yaml } = loadDeps();
    if (opts.only && !SCREENS.some((s) => s.name === opts.only)) {
        console.error(`[FAIL] unknown screen ${opts.only}; known: ${SCREENS.map((s) => s.name).join(", ")}`);
        process.exit(2);
    }
    fs.mkdirSync(opts.out, { recursive: true });
    const texts = labels(yaml, opts.locale);
    const html = fs.readFileSync(path.join(WEB, "index.html"), "utf8")
        .replace("__TELEGRAM_LABELS__", () => JSON.stringify(texts).replace(/</g, "\\u003c"));
    const browser = await chromium.launch({ executablePath: chromePath() });
    try {
        const { shots, errors } = await shoot(browser, html, opts);
        console.log(`[OK] ${shots.length} screenshots -> ${opts.out}`);
        const failures = errors.map((e) => `page error: ${e}`);
        if (!opts.only) failures.push(...(await interactions(browser, html, texts)));
        for (const failure of failures) console.error(`[FAIL] ${failure}`);
        if (failures.length) process.exitCode = 1;
        else console.log("[OK] interaction checks passed");
    } finally {
        await browser.close();
    }
}

main().catch((error) => {
    console.error(`[FAIL] ${error && error.stack ? error.stack : error}`);
    process.exit(1);
});
