// Extension-origin frame embedded in the YouTube tab being recorded: forwards the player's data to
// RDM in order. It runs with the extension's origin, the only one RDM's bridge accepts.
(() => {
  const BRIDGE = "http://127.0.0.1:9614";
  const PAGES = new Set(["https://www.youtube.com", "https://m.youtube.com"]);
  const CALL_TIMEOUT_MS = 60_000; // RDM stuck: the recording stops, chunks must not pile up in memory
  const MAX_QUEUED_BYTES = 64 << 20;
  const MAX_QUEUED_MESSAGES = 256;
  const token = location.hash.slice(1);
  let queue = Promise.resolve();
  let failed = false;
  let queuedBytes = 0;
  let queuedMessages = 0;
  const stopping = new AbortController();

  // Only to the YouTube page embedding this frame: a message aimed at an origin the parent does not
  // have is dropped by the browser, so the token never reaches anyone else.
  const tell = (type, extra = {}) => {
    for (const origin of PAGES) parent.postMessage({ source: "rdm-relay", token, type, ...extra }, origin);
  };

  async function call(path, body, json = false) {
    const timeout = setTimeout(() => stopping.abort(new Error("Recording relay timed out")), CALL_TIMEOUT_MS);
    try {
      const res = await fetch(`${BRIDGE}/record/${token}/${path}`, {
        method: "POST",
        headers: json ? { "content-type": "application/json" } : undefined,
        body: json ? JSON.stringify(body) : body,
        signal: stopping.signal,
      });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
    } finally {
      clearTimeout(timeout);
    }
  }

  // Sequential: chunks must reach the file in the order the player appended them.
  const fail = (reason) => {
    if (failed) return;
    failed = true;
    stopping.abort();
    tell("error", { reason });
  };

  const enqueue = (task, bytes = 0) => {
    if (failed) return;
    if (queuedBytes + bytes > MAX_QUEUED_BYTES || queuedMessages >= MAX_QUEUED_MESSAGES) {
      fail("Recording relay queue is full");
      return;
    }
    queuedBytes += bytes;
    queuedMessages++;
    queue = queue.then(async () => {
      try {
        if (!failed) await task();
      } catch (e) {
        fail(String(e.message ?? e));
      } finally {
        queuedBytes -= bytes;
        queuedMessages--;
      }
    });
  };

  addEventListener("message", (e) => {
    const msg = e.data;
    if (e.source !== parent || !PAGES.has(e.origin) || msg?.source !== "rdm-rec" || msg.token !== token || !/^[0-9a-f]{64}$/.test(token)) return;
    switch (msg.type) {
      case "chunk":
        if (msg.data instanceof ArrayBuffer && Number.isSafeInteger(msg.ms) && msg.ms >= 0 && (msg.track === "video" || msg.track === "audio")) {
          enqueue(() => call(`append?ms=${msg.ms}&track=${msg.track}`, msg.data), msg.data.byteLength);
        }
        break;
      case "progress":
        if (Number.isFinite(msg.fraction)) enqueue(() => call("progress", { fraction: msg.fraction }, true));
        break;
      case "end":
        enqueue(async () => {
          await call("finish");
          tell("finished");
        });
        break;
      case "abort":
        enqueue(() => call("cancel"));
        break;
    }
  });

  tell("loaded");
})();
