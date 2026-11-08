import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import vm from "node:vm";
import * as shared from "../shared.js";

// Exercise the worker through browser events, with only the browser/network boundaries mocked.
function worker({ stores = [], cookieError = false, source, firefox = false, cookies = [{ name: "secret", value: "profile-cookie", sameSite: "strict" }] } = {}) {
  const listeners = {};
  const event = (name) => ({ addListener: (fn) => { listeners[name] = fn; } });
  const requests = [];
  const cookieQueries = [];
  const resumed = [];
  const paused = [];
  const data = { config: { captured: "zip" } };
  const storage = {
    get: async (key) => ({ [key]: data[key] }),
    set: async (values) => Object.assign(data, values),
    remove: async (key) => { delete data[key]; },
  };
  const chrome = {
    i18n: { getMessage: (key) => key },
    runtime: { id: "rdm-test", onMessage: event("message"), onInstalled: event("installed"), onStartup: event("startup") },
    storage: { local: storage, session: storage },
    cookies: {
      getAllCookieStores: async () => stores,
      getAll: (query) => {
        cookieQueries.push(query);
        if (cookieError) throw new Error("Cookie API unavailable");
        return Promise.resolve(cookies);
      },
    },
    action: {
      onClicked: event("action"), setBadgeText: async () => {}, setBadgeBackgroundColor: async () => {}, setTitle: async () => {},
    },
    contextMenus: { onClicked: event("menu") },
    tabs: { onRemoved: event("removed"), onUpdated: event("updated"), query: async () => [], sendMessage: async () => {} },
    alarms: { onAlarm: event("alarm") },
    downloads: {
      onCreated: event("download"),
      pause: async (id) => { paused.push(id); },
      resume: async (id) => { resumed.push(id); },
      cancel: async () => {}, erase: async () => {},
    },
  };
  // Firefox takes responses over as their headers arrive (`blocking`); the other listener sniffs media.
  if (firefox) {
    chrome.webRequest = {
      OnHeadersReceivedOptions: { BLOCKING: "blocking" },
      onHeadersReceived: { addListener: (fn, _filter, options) => { listeners[options.includes("blocking") ? "response" : "sniff"] = fn; } },
    };
  }
  const context = vm.createContext({
    ...shared, chrome, navigator: { userAgent: "Chrome/140" }, URL, Response, AbortSignal, setTimeout, clearTimeout,
    fetch: async (url, options) => {
      requests.push({ url, body: options.body && JSON.parse(options.body) });
      return url.endsWith("/config")
        ? new Response(JSON.stringify({ captured: "zip" }))
        : new Response(null, { status: 202 });
    },
  });
  // `\r?`: a Windows checkout (core.autocrlf) ends lines with CRLF.
  vm.runInContext(source ?? readFileSync(new URL("../background.js", import.meta.url), "utf8").replace(/^import .*\r?\n/, ""), context);
  const download = (tab, referrer = "https://files.example.com/page") => new Promise((resolve) => {
    listeners.message({ kind: "download", url: "https://files.example.com/a.zip" },
      { id: chrome.runtime.id, tab: { ...tab, url: referrer }, url: referrer }, resolve);
  });
  return { listeners, requests, cookieQueries, resumed, paused, download };
}

test("an unknown private cookie store never reads the normal profile", async () => {
  const app = worker();
  assert.equal(await app.download({ id: 7, incognito: true }), true);
  assert.equal(app.cookieQueries.length, 0);
  assert.equal(app.requests.at(-1).body.cookies, "");
});

const gitSh = path.join(process.env.ProgramFiles ?? "C:/Program Files", "Git/bin/sh.exe");
test("Safari's bundled plain worker includes the public suffix rules and handles a download", { skip: process.platform === "win32" && !existsSync(gitSh) }, async (t) => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "rdm-safari-worker-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const output = path.join(directory, "background.js");
  const bundled = spawnSync(process.platform === "win32" ? gitSh : "sh", [path.join(root, "packaging/safari/bundle-worker.sh"), root, output], { encoding: "utf8", windowsHide: true });
  assert.equal(bundled.status, 0, bundled.stderr);
  const source = readFileSync(output, "utf8");
  assert.doesNotMatch(source, /^(?:import|export) /m);
  const app = worker({ source });
  assert.equal(await app.download({ id: 7 }), true);
  assert.equal(app.requests.at(-1).body.cookies, "secret=profile-cookie");
});

test("a known private store is used for the hand-off", async () => {
  const app = worker({ stores: [{ id: "private", tabIds: [7] }] });
  assert.equal(await app.download({ id: 7, incognito: true }), true);
  assert.equal(app.cookieQueries[0].storeId, "private");
  assert.equal(app.requests.at(-1).body.cookies, "secret=profile-cookie");
});

test("an HTTP referrer cannot forward HTTPS SameSite=Strict cookies", async () => {
  const app = worker();
  assert.equal(await app.download({ id: 7 }, "http://files.example.com/page"), true);
  assert.equal(app.requests.at(-1).body.cookies, "");
});

test("a download in a frame of another site gets none of that site's own cookies (Firefox)", async () => {
  // Cookies Firefox does send to a frame of another site when not partitioned: kept out all the same.
  const app = worker({ firefox: true, cookies: [{ name: "sid", value: "1", sameSite: "no_restriction" }, { name: "pref", value: "2", sameSite: "lax" }] });
  const framed = (page) => app.listeners.response({
    url: "https://files.example.com/a.zip", type: "sub_frame", method: "GET", statusCode: 200,
    responseHeaders: [{ name: "Content-Disposition", value: "attachment; filename=a.zip" }],
    originUrl: page, documentUrl: page, frameAncestors: [{ url: page, frameId: 0 }],
  });
  assert.equal((await framed("https://ads.example.net/")).cancel, true, "taken over by RDM");
  assert.equal(app.requests.at(-1).body.cookies, "", "embedded by another site: partitioned, Firefox sent none");
  await framed("https://www.example.com/");
  assert.equal(app.requests.at(-1).body.cookies, "sid=1; pref=2", "a frame of the page's own site");
});

test("a failed hand-off resumes a download already paused by the worker", async () => {
  const app = worker({ cookieError: true });
  await app.listeners.download({ id: 42, url: "https://files.example.com/a.zip", filename: "a.zip" });
  assert.deepEqual(app.paused, [42]);
  assert.deepEqual(app.resumed, [42]);
});
