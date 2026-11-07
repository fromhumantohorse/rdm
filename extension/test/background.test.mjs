import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import * as shared from "../shared.js";

// Exercise the worker through browser events, with only the browser/network boundaries mocked.
function worker({ stores = [], cookieError = false } = {}) {
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
        return Promise.resolve([{ name: "secret", value: "profile-cookie", sameSite: "strict" }]);
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
  const context = vm.createContext({
    ...shared, chrome, navigator: { userAgent: "Chrome/140" }, URL, Response, AbortSignal, setTimeout, clearTimeout,
    fetch: async (url, options) => {
      requests.push({ url, body: options.body && JSON.parse(options.body) });
      return url.endsWith("/config")
        ? new Response(JSON.stringify({ captured: "zip" }))
        : new Response(null, { status: 202 });
    },
  });
  vm.runInContext(readFileSync(new URL("../background.js", import.meta.url), "utf8").replace(/^import .*\n/, ""), context);
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

test("a failed hand-off resumes a download already paused by the worker", async () => {
  const app = worker({ cookieError: true });
  await app.listeners.download({ id: 42, url: "https://files.example.com/a.zip", filename: "a.zip" });
  assert.deepEqual(app.paused, [42]);
  assert.deepEqual(app.resumed, [42]);
});
