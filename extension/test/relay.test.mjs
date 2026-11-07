import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

function relay(fetch) {
  const token = "a".repeat(64);
  const messages = [];
  const parent = { postMessage: (message) => { messages.push(message); } };
  let listener;
  const context = vm.createContext({
    parent, location: { hash: `#${token}` }, ArrayBuffer, AbortController, setTimeout, clearTimeout, fetch,
    addEventListener: (_, handler) => { listener = handler; },
  });
  vm.runInContext(readFileSync(new URL("../relay.js", import.meta.url), "utf8"), context);
  const send = (type, extra = {}, source = parent) => listener({
    source, origin: "https://www.youtube.com", data: { source: "rdm-rec", token, type, ...extra },
  });
  return { send, messages };
}

test("recording relay accepts messages only from its embedding window", async () => {
  let calls = 0;
  const app = relay(async () => { calls++; return { ok: true }; });
  app.send("progress", { fraction: 0.5 }, {});
  await new Promise(setImmediate);
  assert.equal(calls, 0);
  app.send("progress", { fraction: 0.5 });
  await new Promise(setImmediate);
  assert.equal(calls, 1);
});

test("recording relay bounds retained chunks when RDM stops answering", async () => {
  let cancelled = false;
  const app = relay((_, { signal }) => new Promise((_, reject) => {
    signal.addEventListener("abort", () => { cancelled = true; reject(signal.reason); }, { once: true });
  }));
  app.send("chunk", { ms: 1, track: "video", data: new ArrayBuffer(1) });
  await new Promise(setImmediate);
  // Retaining this chunk would exceed the queue's 64 MiB limit.
  app.send("chunk", { ms: 1, track: "video", data: new ArrayBuffer(64 << 20) });
  await new Promise(setImmediate);
  assert.ok(cancelled);
  assert.ok(app.messages.some(({ type, reason }) => type === "error" && reason.includes("queue is full")));
});

test("recording relay bounds queued control messages too", async () => {
  const app = relay(async () => ({ ok: true }));
  for (let i = 0; i < 257; i++) app.send("progress", { fraction: 0.5 });
  await new Promise(setImmediate);
  assert.ok(app.messages.some(({ type }) => type === "error"));
});
