// Headless preview of the Telegram Mini App: renders every screen from fixture payloads and
// checks entry-volume rendering and the owner commands the page sends. No core, no bot, no network.
//
//   node tools/miniapp_preview/preview.mjs [--out <dir>] [--locale ru|en|es|uk] [--only <screen>]
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
            console.log("usage: node tools/miniapp_preview/preview.mjs [--out <dir>] [--locale ru|en|es|uk] [--only <screen>]");
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
        console.error("[FAIL] dependencies missing: run `npm install` once inside tools/miniapp_preview");
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

/**
 * Load preview labels from English `telegram*` areas, stripping the `telegram.` key prefix.
 * Read overrides from locales/<locale>/<area>.<locale>.yml, falling back per key to English
 * when the locale file is missing or its value is not a string. Locale-only keys are ignored.
 * @param {object} yaml YAML parser used to load each dictionary.
 * @param {string} locale Requested language code, also returned as the `locale` label.
 * @returns {object} Label map for the preview page.
 * @throws {Error} If the English directory or a dictionary cannot be read or parsed.
 */
function labels(yaml, locale) {
    const out = { locale };
    const enDir = path.join(LOCALES, "en");
    const files = fs.readdirSync(enDir).filter((f) => /^telegram.*\.en\.yml$/.test(f)).sort();
    for (const file of files) {
        const area = file.slice(0, -".en.yml".length);
        const enDoc = yaml.load(fs.readFileSync(path.join(enDir, file), "utf8")) || {};
        const localeFile = path.join(LOCALES, locale, `${area}.${locale}.yml`);
        const localeDoc = locale === "en" || !fs.existsSync(localeFile)
            ? {}
            : (yaml.load(fs.readFileSync(localeFile, "utf8")) || {});
        for (const [key, fallback] of Object.entries(enDoc)) {
            if (!key.startsWith("telegram.") || typeof fallback !== "string") continue;
            const local = localeDoc[key];
            const text = typeof local === "string" ? local : fallback;
            out[key.slice("telegram.".length)] = text;
        }
    }
    return out;
}

const fixture = (name) => JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf8"));

const VIEWPORTS = [
    { width: 421, height: 900 },
    { width: 390, height: 844 },
    { width: 320, height: 740 },
    { width: 1024, height: 768 },
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
                BackButton: {
                    isVisible: false,
                    show() { this.isVisible = true; },
                    hide() { this.isVisible = false; },
                    onClick(callback) { window.previewBack = callback; },
                    offClick: noop,
                },
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
// A save on this page echoes into the next read so a later visit is not a stale draft.
function defaultApi() {
    let notifySaved = null;
    return {
        "/api/session": { ok: true },
        "/api/report": (raw) => {
            const period = (JSON.parse(raw || "{}").period) || "today";
            return { body: fixture(period === "month" || period === "last_month" ? "report_month" : "report_today") };
        },
        "/api/cores": fixture("cores"),
        "/api/orders": fixture("orders"),
        "/api/trades": fixture("trades"),
        "/api/strategies": fixture("strategies"),
        "/api/strategy/toggle": OK,
        "/api/order/cancel": OK,
        "/api/notify": () => ({ body: notifySaved || fixture("notify") }),
        "/api/notify/save": (raw) => {
            let posted = {};
            try { posted = JSON.parse(raw || "{}"); } catch { posted = {}; }
            const base = fixture("notify");
            const revision = typeof posted.revision === "number"
                ? posted.revision + 1
                : (typeof base.revision === "number" ? base.revision : 0);
            notifySaved = {
                settings: posted.settings || base.settings,
                cores: base.cores,
                zone: base.zone,
                revision,
                error: null,
            };
            return { body: notifySaved };
        },
    };
}

const settle = (page) => page.waitForTimeout(350);
const nav = async (page, tab) => {
    await page.click(`#app-nav button[data-tab="${tab}"]`);
    await settle(page);
};
/** Enter the notification category so legacy form checks still test the same controls. */
const openNotifications = async (page) => {
    await nav(page, "settings");
    await page.click('[data-settings-category="notifications"]');
    await settle(page);
};
const openFirstGroup = async (page, tab) => {
    await page.waitForSelector(`section[data-tab="${tab}"] .group-head`);
    await page.click(`section[data-tab="${tab}"] .group-head`);
    await settle(page);
};

// The bar is five buttons, in this order, and the removed Balances tab stays gone.
async function assertNav(page) {
    const state = await page.evaluate(() => {
        const buttons = [...document.querySelectorAll("#app-nav button")];
        return {
            tabs: buttons.map((node) => node.getAttribute("data-tab")),
            stray: !!document.querySelector('[data-tab="balances"]'),
        };
    });
    const wanted = "report,cores,deals,strategies,settings";
    if (state.tabs.join(",") !== wanted || state.stray) {
        throw new Error(`nav must be five buttons ${wanted} and no balances tab: ${JSON.stringify(state)}`);
    }
}

const openTrade = async (page, nth) => {
    await nav(page, "deals"); await page.click('[data-seg="trades"]');
    const head = await page.$('section[data-tab="trades"] .group-head');
    if (head) { await head.click(); await settle(page); }
    await page.locator('section[data-tab="trades"] .trade-row').nth(nth).click(); await settle(page);
};

// Reversing the segment order, resetting an explicit choice, or changing only the pressed button
// must fail: the rendered order, selected button and visible pane form one navigation contract.
async function checkDealsSelection(page, expected) {
    const state = await page.evaluate(() => {
        const nodes = [...document.querySelectorAll('[data-seg]')];
        const panes = [...document.querySelectorAll('section[data-tab="orders"], section[data-tab="trades"]')];
        return {
            order: nodes.map((n) => n.getAttribute("data-seg")),
            selected: nodes.filter((n) => n.getAttribute("aria-pressed") === "true").map((n) => n.getAttribute("data-seg")),
            visible: panes.filter((n) => !n.hidden).map((n) => n.getAttribute("data-tab")),
        };
    });
    if (state.order.join(",") !== "trades,orders" || state.selected.join(",") !== expected || state.visible.join(",") !== expected) {
        throw new Error(`Trades navigation must show Closed first and select ${expected}: ${JSON.stringify(state)}`);
    }
}

const SETTINGS_SAVE_ERROR = "Выберите хотя бы одно доступное ядро";

function notifyTradesOn() {
    const base = fixture("notify");
    const settings = JSON.parse(JSON.stringify(base.settings));
    settings.trades.on = true;
    settings.trades.cores = { kind: "only", ids: [1, 2] };
    settings.trades.min_volume_usd = 100;
    settings.trades.profit_at_least_usd = 5;
    settings.trades.loss_at_least_usd = null;
    return Object.assign({}, base, { settings });
}

// Horizontal overflow or a label that ellipsizes. Vertical scroll is allowed.
async function assertSettingsFit(page) {
    const clip = await page.evaluate(() => {
        const root = document.documentElement;
        if (root.scrollWidth > window.innerWidth + 1) return "page";
        const nodes = document.querySelectorAll(
            'section[data-tab="settings"] .name, section[data-tab="settings"] .chip, section[data-tab="settings"] .settings-note, section[data-tab="settings"] .settings-label, section[data-tab="settings"] .settings-caption'
        );
        for (const node of nodes) {
            const rect = node.getBoundingClientRect();
            if (rect.width > 0 && (rect.left < -1 || rect.right > window.innerWidth + 1)) return "wide:" + node.textContent;
            if (node.scrollWidth > node.clientWidth + 1) return "clipped:" + node.textContent;
        }
        return "";
    });
    if (clip) throw new Error(`settings must not clip: ${clip}`);
}

// A long coin quantity ellipsizes, and the value stays inside the row and the viewport.
async function assertCoinQtyClips(page) {
    const fault = await page.evaluate(() => {
        const qty = [...document.querySelectorAll(".coin-qty")].find((node) =>
            (node.textContent || "").includes("123456789")
        );
        if (!qty) return "missing TEST quantity";
        const row = qty.closest(".coin-row");
        if (!row) return "quantity has no row";
        const value = row.querySelector(".balance-figure");
        if (!value) return "row has no value";
        const rowBox = row.getBoundingClientRect();
        const valueBox = value.getBoundingClientRect();
        if (valueBox.right > rowBox.right + 1) return "value past the row";
        if (rowBox.right > window.innerWidth + 1) return "row past the viewport";
        if (window.innerWidth <= 340 && qty.scrollWidth <= qty.clientWidth + 1) {
            return "quantity did not clip";
        }
        return "";
    });
    if (fault) throw new Error(`coin quantity must stay in the row: ${fault}`);
}

async function assertSettingsDefault(page) {
    const state = await page.evaluate(() => {
        const pressed = (sel) => {
            const node = document.querySelector(sel);
            return node ? node.getAttribute("aria-pressed") : null;
        };
        const field = (sel) => {
            const node = document.querySelector(sel);
            return node ? { value: node.value, disabled: node.disabled } : null;
        };
        const save = document.querySelector("[data-settings-save]");
        return {
            cards: document.querySelectorAll("[data-settings-card]").length,
            note: !!document.querySelector("[data-settings-note]"),
            hint: !!document.querySelector("[data-settings-hint]"),
            chips: document.querySelectorAll(".settings-chips .chip").length,
            trades: pressed('[data-settings-card="trades"] [data-settings="card-on"]'),
            down: pressed('[data-settings-card="down"] [data-settings="card-on"]'),
            daily: pressed('[data-settings-card="daily"] [data-settings="card-on"]'),
            all: pressed('[data-settings="scope-all"]'),
            minutes: field('[data-settings="minutes"]'),
            time: field('[data-settings="time"]'),
            saveDisabled: save ? save.disabled : null,
        };
    });
    const time = state.time && (state.time.value === "21:00" || state.time.value === "21:00:00");
    if (state.cards !== 3 || !state.note || !state.hint || state.chips !== 1
        || state.trades !== "false" || state.down !== "false" || state.daily !== "false"
        || state.all !== "true"
        || !state.minutes || state.minutes.value !== "5" || !state.minutes.disabled
        || !time || !state.time.disabled
        || state.saveDisabled !== true) {
        throw new Error(`settings default must be three off cards: ${JSON.stringify(state)}`);
    }
    await assertSettingsFit(page);
}

async function assertSettingsTradesOn(page) {
    const state = await page.evaluate(() => {
        const pressed = (sel) => {
            const node = document.querySelector(sel);
            return node ? node.getAttribute("aria-pressed") : null;
        };
        const field = (sel) => {
            const node = document.querySelector(sel);
            return node ? { value: node.value, disabled: node.disabled } : null;
        };
        return {
            trades: pressed('[data-settings-card="trades"] [data-settings="card-on"]'),
            all: pressed('[data-settings="scope-all"]'),
            c1: pressed('[data-settings-core="1"]'),
            c2: pressed('[data-settings-core="2"]'),
            c3: pressed('[data-settings-core="3"]'),
            volumeOn: pressed('[data-settings="volume-switch"]'),
            volume: field('[data-settings="min-volume"]'),
            profitOn: pressed('[data-settings="profit-switch"]'),
            profit: field('[data-settings="profit"]'),
            lossOn: pressed('[data-settings="loss-switch"]'),
            loss: field('[data-settings="loss"]'),
            hint: !!document.querySelector("[data-settings-hint]"),
        };
    });
    if (state.trades !== "true" || state.all !== "false"
        || state.c1 !== "true" || state.c2 !== "true" || state.c3 !== "false"
        || state.volumeOn !== "true" || !state.volume || state.volume.value !== "100" || state.volume.disabled
        || state.profitOn !== "true" || !state.profit || state.profit.value !== "5" || state.profit.disabled
        || state.lossOn !== "false" || !state.loss || !state.loss.disabled
        || state.hint) {
        throw new Error(`settings trades-on must select two cores and two thresholds: ${JSON.stringify(state)}`);
    }
    await assertSettingsFit(page);
}

// Each screen starts from a fresh page; `api` overrides replace fixture routes.
const SCREENS = [
    { name: "settings-root", run: async (p) => {
        await nav(p, "settings");
        if (await p.locator("[data-settings-category]").count() !== 1
            || await p.locator("[data-settings-card]").count() !== 0) {
            throw new Error("Settings must open its single category list without notification cards");
        }
        await p.click('[data-settings-category="notifications"]');
        await p.reload();
        await p.waitForSelector("#app-nav:not([hidden])");
        await nav(p, "settings");
        if (await p.locator("[data-settings-category]").count() !== 1) throw new Error("refresh must reset category navigation");
        await assertSettingsFit(p);
    } },
    { name: "settings-many-cores", api: {
        "/api/notify": () => ({ body: fixture("notify_many") }),
    }, run: async (p) => {
        await openNotifications(p);
        if (await p.locator("[data-settings-core]").count() !== 40) {
            throw new Error("many-core picker must contain 40 synthetic cores");
        }
        await p.fill("[data-settings-search]", "Synthetic 40");
        if (await p.locator("[data-settings-core]").count() !== 1) throw new Error("search must filter core names");
        await p.click('[data-settings-core="40"]');
        await p.fill("[data-settings-search]", "not-a-core");
        if (await p.locator("[data-settings-core]").count() !== 0) throw new Error("unmatched search must be empty");
        await p.fill("[data-settings-search]", "");
        if (await p.getAttribute('[data-settings-core="40"]', "aria-pressed") !== "true") {
            throw new Error("search must retain offscreen selection");
        }
        await p.locator('[data-settings-core="39"]').scrollIntoViewIfNeeded();
        await p.click('[data-settings-core="39"]');
        if (!(await p.evaluate(() => document.querySelector(".settings-core-list").scrollTop > 0))) {
            throw new Error("core selection must retain the list's scroll position");
        }
        await p.evaluate(() => window.previewBack());
        if (await p.locator("[data-settings-card]").count() !== 0) throw new Error("back must return to registry");
        await p.click('[data-settings-category="notifications"]');
        if (await p.getAttribute('[data-settings-core="40"]', "aria-pressed") !== "true") {
            throw new Error("category back must retain the unsaved core draft");
        }
        await p.evaluate(() => window.previewBack());
        if (await p.locator("[data-settings-category]").count() !== 1) throw new Error("Telegram back must return to list");
        await p.click('[data-settings-category="notifications"]');
        await p.locator("[data-settings-search]").scrollIntoViewIfNeeded();
        await assertSettingsFit(p);
        const bounded = await p.evaluate(() => {
            const list = document.querySelector(".settings-core-list");
            const name = document.querySelector(".settings-core-name");
            return list.clientHeight <= 224 && list.scrollHeight > list.clientHeight
                && getComputedStyle(name).textOverflow === "ellipsis";
        });
        if (!bounded) throw new Error("core list must be bounded with ellipsized names");
    } },
    { name: "report-today", run: async () => {} },
    { name: "report-month", run: async (p) => { await p.click('[data-period="month"]'); await settle(p); } },
    { name: "report-month-scrolled", run: async (p) => {
        await p.click('[data-period="month"]'); await settle(p);
        await p.evaluate(() => {
            const rows = document.querySelectorAll(".day-table tbody tr");
            if (rows.length) rows[Math.floor(rows.length / 2)].scrollIntoView({ block: "center" });
        });
        await settle(p);
    } },
    { name: "cores", run: async (p) => { await nav(p, "cores"); } },
    { name: "cores-group-open", run: async (p) => { await nav(p, "cores"); await openFirstGroup(p, "cores"); } },
    { name: "core-detail", run: async (p) => {
        await nav(p, "cores"); await openFirstGroup(p, "cores");
        await p.click('section[data-tab="cores"] .core-row'); await settle(p);
    } },
    { name: "trades-open-group-open", run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="orders"]'); await openFirstGroup(p, "orders");
    } },
    { name: "deals-default-and-memory", run: async (p) => {
        await nav(p, "deals");
        await checkDealsSelection(p, "trades");
        await p.click('[data-seg="orders"]'); await settle(p);
        await checkDealsSelection(p, "orders");
        await nav(p, "cores"); await nav(p, "deals");
        await checkDealsSelection(p, "orders");
        await p.click('[data-seg="trades"]'); await settle(p);
        await nav(p, "report"); await nav(p, "deals");
        await checkDealsSelection(p, "trades");
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
    { name: "trade-sheet-no-volume", api: {
        "/api/trades": { trades: [{ ...fixture("trades").trades[0], volume_text: null }], limit: 50 },
    }, run: async (p) => { await openTrade(p, 0); } },
    // Row 2 names its strategy, row 6 carries none and is not marked manual (row 1 is manual).
    { name: "trade-sheet-strategy", run: async (p) => { await openTrade(p, 1); } },
    { name: "trade-sheet-unknown", run: async (p) => { await openTrade(p, 5); } },
    { name: "strategies-open", run: async (p) => { await nav(p, "strategies"); await openFirstGroup(p, "strategies"); } },
    // Cores carries the account total. Figures stay masked until the eye toggle.
    { name: "cores-total-masked", run: async (p) => { await nav(p, "cores"); await openFirstGroup(p, "cores"); } },
    { name: "cores-total-unmasked", run: async (p) => {
        await nav(p, "cores"); await p.click(".eye-btn"); await openFirstGroup(p, "cores");
    } },
    { name: "cores-coins-expanded", run: async (p) => {
        await nav(p, "cores");
        await p.click(".eye-btn");
        await openFirstGroup(p, "cores");
        await p.click('section[data-tab="cores"] .coin-chev');
        await settle(p);
        await assertCoinQtyClips(p);
    } },
    { name: "settings-default", run: async (p) => {
        await openNotifications(p);
        await p.waitForSelector('[data-settings-card="trades"]');
        await assertSettingsDefault(p);
    } },
    { name: "settings-trades-on", api: { "/api/notify": () => ({ body: notifyTradesOn() }) }, run: async (p) => {
        await openNotifications(p);
        await p.waitForSelector('[data-settings-card="trades"]');
        await assertSettingsTradesOn(p);
    } },
    { name: "settings-save-error", api: {
        "/api/notify/save": () => ({ body: Object.assign({}, fixture("notify"), { error: SETTINGS_SAVE_ERROR }) }),
    }, run: async (p) => {
        await openNotifications(p);
        await p.waitForSelector('[data-settings-card="trades"] [data-settings="card-on"]');
        await p.click('[data-settings-card="trades"] [data-settings="card-on"]');
        await settle(p);
        await p.click("[data-settings-save]");
        await p.waitForFunction((wanted) => {
            const node = document.querySelector("[data-settings-status]");
            return node && !node.hidden && node.textContent === wanted;
        }, SETTINGS_SAVE_ERROR);
        const pressed = await p.getAttribute('[data-settings-card="trades"] [data-settings="card-on"]', "aria-pressed");
        if (pressed !== "true") throw new Error("a refused save must keep the draft switch on");
        if (await p.locator("[data-settings-save]").isDisabled()) {
            throw new Error("save stays enabled after a refused save while the draft is still valid");
        }
        await p.locator("[data-settings-status]").scrollIntoViewIfNeeded();
        await assertSettingsFit(p);
    } },
    { name: "settings-draft-survives", run: async (p) => {
        await openNotifications(p);
        await p.waitForSelector('[data-settings-card="trades"] [data-settings="card-on"]');
        await p.click('[data-settings-card="trades"] [data-settings="card-on"]');
        await settle(p);
        await p.click('[data-settings="volume-switch"]');
        await settle(p);
        await p.fill('[data-settings="min-volume"]', "42");
        await p.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
        await p.waitForTimeout(600);
        const state = await p.evaluate(() => {
            const card = document.querySelector('[data-settings-card="trades"] [data-settings="card-on"]');
            const input = document.querySelector('[data-settings="min-volume"]');
            return {
                pressed: card ? card.getAttribute("aria-pressed") : null,
                value: input ? input.value : null,
                disabled: input ? input.disabled : null,
            };
        });
        if (state.pressed !== "true" || state.value !== "42" || state.disabled) {
            throw new Error(`a visibility return must keep the settings draft: ${JSON.stringify(state)}`);
        }
        await p.locator('[data-settings="min-volume"]').scrollIntoViewIfNeeded();
        await assertSettingsFit(p);
    } },
    { name: "empty-orders", api: { "/api/orders": { orders: [], can_control: true } }, run: async (p) => {
        await nav(p, "deals"); await p.click('[data-seg="orders"]'); await settle(p);
    } },
    { name: "empty-strategies", api: { "/api/strategies": { cores: [], can_control: true } }, run: async (p) => { await nav(p, "strategies"); } },
    { name: "error-cores", api: { "/api/cores": () => ({ status: 500, body: {} }) }, run: async (p) => { await nav(p, "cores"); await p.waitForTimeout(600); } },
    { name: "session-denied", api: { "/api/session": () => ({ status: 403, body: {} }) }, ready: "#startup", run: async () => {} },
];

// Check the actual rendered entry-volume contract, including omission of unavailable figures.
async function volumeChecks(page, screen, texts) {
    if (screen === "trades-open-group-open") {
        const row = page.locator('section[data-tab="orders"] .order-row').first();
        const text = await row.innerText();
        const expected = fixture("orders").orders[0].volume_text;
        if (!text.includes(texts.mini_orders_volume) || !text.includes(expected)) {
            throw new Error("open order must display localized entry volume");
        }
        if (await row.locator('.order-flow .order-bit').count() !== 2) {
            throw new Error("open order must contain prices and volume without an extra quantity");
        }
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
        if (overflow) throw new Error("open-order volume must wrap without horizontal overflow");
    }
    if (screen === "trade-sheet" || screen === "trade-sheet-no-volume") {
        const text = await page.locator('#sheet').innerText();
        if (!text.includes(texts.mini_trade_qty)) throw new Error("closed card must retain quantity");
        const hasVolume = text.includes(texts.mini_trade_volume);
        if (hasVolume !== (screen === "trade-sheet")) {
            throw new Error("closed card must show volume only when the DTO supplies it");
        }
        if (hasVolume && !text.includes(fixture("trades").trades[0].volume_text)) {
            throw new Error("closed card must render the Rust-formatted volume unchanged");
        }
    }
}

// Every screen at every viewport and theme; returns the files and unexpected page errors.
async function shoot(browser, html, opts, texts) {
    const shots = [];
    const log = { errors: [], sent: [] };
    for (const screen of SCREENS) {
        if (opts.only && screen.name !== opts.only) continue;
        for (const viewport of VIEWPORTS) for (const theme of THEMES) {
            const api = Object.assign(defaultApi(), screen.api || {});
            const { page, context } = await openPage(browser, html, viewport, theme, api, log);
            await page.waitForSelector(screen.ready || "#app-nav:not([hidden])");
            await settle(page);
            await assertNav(page);
            await screen.run(page);
            await volumeChecks(page, screen.name, texts);
            const file = path.join(opts.out, `${screen.name}-${viewport.width}x${viewport.height}-${theme}.png`);
            await page.screenshot({ path: file });
            shots.push(file);
            await context.close();
        }
    }
    // 500 and 403 are the error and denied screens. Unknown routes still answer 404.
    return { shots, errors: log.errors.filter((e) => !/status of 500|status of 403|status of 404/.test(e)) };
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

    await openNotifications(page);
    await page.waitForSelector('[data-settings-card="trades"] [data-settings="card-on"]');
    await page.click('[data-settings-card="trades"] [data-settings="card-on"]');
    await settle(page);
    expect(!!await page.$("[data-settings-hint]"), "trades hint stays while both thresholds are off");
    await page.click("[data-settings-save]");
    await page.waitForFunction((wanted) => {
        const node = document.querySelector("[data-settings-status]");
        return node && !node.hidden && node.textContent === wanted;
    }, texts.mini_settings_saved);
    const firstSaves = log.sent.filter((s) => s.path === "/api/notify/save");
    let firstSave = {};
    try { firstSave = JSON.parse(firstSaves[0] ? firstSaves[0].raw : ""); } catch { firstSave = {}; }
    expect(firstSaves.length === 1 && firstSave.revision === 0 && firstSave.settings && firstSave.settings.trades.on === true,
        `first settings save ${JSON.stringify(firstSaves.map((s) => s.raw))}`);
    await page.click('[data-settings="volume-switch"]');
    await settle(page);
    expect(!await page.$("[data-settings-hint]"), "volume alone hides the trades hint");
    await page.fill('[data-settings="min-volume"]', "10");
    await page.click('[data-settings="profit-switch"]');
    await settle(page);
    expect(!await page.$("[data-settings-hint]"), "profit on hides the trades hint");
    await page.fill('[data-settings="profit"]', "5");
    await page.click("[data-settings-save]");
    await page.waitForFunction((wanted) => {
        const node = document.querySelector("[data-settings-status]");
        return node && !node.hidden && node.textContent === wanted;
    }, texts.mini_settings_saved);
    const secondSaves = log.sent.filter((s) => s.path === "/api/notify/save");
    let secondSave = {};
    try { secondSave = JSON.parse(secondSaves[1] ? secondSaves[1].raw : ""); } catch { secondSave = {}; }
    expect(secondSaves.length === 2 && secondSave.revision === 1,
        `second settings save revision ${secondSave.revision}, bodies ${JSON.stringify(secondSaves.map((s) => s.raw))}`);
    // Losing the search-independent scope or turning an empty explicit scope into All must fail.
    await page.click('[data-settings="scope-all"]');
    expect(await page.locator("[data-settings-save]").isDisabled(), "an empty explicit core scope must not save as All");
    await page.fill("[data-settings-search]", "Beta");
    await page.click('[data-settings-core="2"]');
    await page.evaluate(() => window.previewBack());
    expect((await page.locator(".settings-category-summary").innerText()) === texts.mini_settings_summary_trades,
        "category summary must reflect enabled trades in the retained draft");
    await page.click('[data-settings-category="notifications"]');
    await page.click("[data-settings-save]");
    await page.waitForFunction((wanted) => document.querySelector("[data-settings-status]").textContent === wanted,
        texts.mini_settings_saved);
    const scopeSaves = log.sent.filter((s) => s.path === "/api/notify/save");
    const scopeSave = JSON.parse(scopeSaves[2].raw);
    expect(scopeSave.revision === 2 && scopeSave.settings.trades.cores.kind === "only"
        && JSON.stringify(scopeSave.settings.trades.cores.ids) === "[2]",
        "filtered selection must save the selected core ID with the existing revision contract");
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
        const { shots, errors } = await shoot(browser, html, opts, texts);
        console.log(`[OK] ${shots.length} screenshots -> ${opts.out}`);
        const failures = errors.map((e) => `page error: ${e}`);
        if (!opts.only || opts.only.startsWith("settings-")) failures.push(...(await interactions(browser, html, texts)));
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
