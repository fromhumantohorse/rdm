// Floating "download this video" button over <video>/<audio>, IDM-style, with a quality menu.
(() => {
  if (window.__rdm) return;
  window.__rdm = true;
  const ext = globalThis.browser ?? globalThis.chrome; // Firefox / Chrome
  /** The browser's language (English by default, see `_locales`). */
  const t = (key, ...subs) => ext.i18n.getMessage(key, subs.map(String)) || key;

  const MIN_W = 160;
  const MIN_H = 90;
  const HIDE_DELAY = 1500;
  const YT_TIMEOUT = 20_000;
  const onYouTube = /(^|\.)youtube(-nocookie)?\.com$/.test(location.hostname);

  // Built when first shown: most pages and frames (ads, widgets) never need any of it.
  let host, btn, menu, label;
  function buildOverlay() {
    if (host) return;
    host = document.createElement("rdm-overlay");
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
    <style>
      :host { all: initial; position: fixed; z-index: 2147483647; display: none;
              font: 13px/1.4 Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
      .btn { display: flex; align-items: center; gap: 8px; padding: 8px 15px 8px 11px; border: 1px solid #ffffff2e;
             border-radius: 999px; background: linear-gradient(135deg, #3d8bff, #6a5cff 55%, #b24cff); color: #fff;
             cursor: pointer; font: inherit; font-weight: 650; letter-spacing: .1px;
             box-shadow: 0 8px 24px #6a5cff73, inset 0 1px 0 #ffffff40;
             transition: transform .15s ease, box-shadow .15s ease, filter .15s ease; }
      .btn:hover { transform: translateY(-1px); filter: brightness(1.08); box-shadow: 0 12px 30px #6a5cff8c, inset 0 1px 0 #ffffff4d; }
      .btn:active { transform: scale(.97); }
      .btn svg { width: 18px; height: 18px; flex: none; }
      .menu { position: absolute; right: 0; top: calc(100% + 10px); width: min(560px, 92vw); max-height: min(460px, 70vh);
              overflow: auto; margin: 0; padding: 8px; list-style: none; color: #eaeef8; background: #111626eb;
              -webkit-backdrop-filter: blur(18px) saturate(1.4); backdrop-filter: blur(18px) saturate(1.4);
              border: 1px solid #ffffff17; border-radius: 16px; box-shadow: 0 24px 60px #0000008c;
              animation: pop .16s ease-out; scrollbar-width: thin; scrollbar-color: #ffffff30 transparent; }
      @keyframes pop { from { opacity: 0; transform: translateY(-6px) scale(.98); } }
      .menu[hidden] { display: none; }
      li { padding: 9px 12px; border-radius: 10px; }
      li.action { cursor: pointer; transition: background .12s ease; }
      li.action:hover { background: linear-gradient(135deg, #3d8bff47, #b24cff38); }
      li.section { padding: 10px 12px 4px; color: #8d97b3; font-size: 10.5px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
      li.note { color: #8d97b3; }
      .title { overflow-wrap: anywhere; font-weight: 600; }
      .detail { color: #8d97b3; font-size: 12px; margin-top: 2px; }
      li.action:hover .detail { color: #d4dbff; }
      .btn.mini { padding: 7px; gap: 0; }
      .btn.mini span { display: none; }
    </style>
    <button class="btn"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"
      stroke-linejoin="round" aria-hidden="true"><path d="M12 4v11"/><path d="m7 11 5 5 5-5"/><path d="M5 20h14"/></svg>
      <span></span></button>
    <ul class="menu" hidden></ul>`;
    btn = root.querySelector(".btn");
    menu = root.querySelector(".menu");
    label = btn.querySelector("span");
    // RDM's "Mini download button" setting, cached by the service worker (see `appConfig`).
    const setMini = (on) => btn.classList.toggle("mini", on === true);
    ext.storage.local.get("mini").then(({ mini }) => setMini(mini), () => {});
    ext.storage.onChanged.addListener((changes, area) => area === "local" && "mini" in changes && setMini(changes.mini.newValue));
    host.addEventListener("pointerenter", () => clearTimeout(hideTimer));
    host.addEventListener("pointerleave", scheduleHide);
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (menu.hidden) openMenu();
      else menu.hidden = true;
    });
    document.addEventListener("click", (e) => e.composedPath().includes(host) || (menu.hidden = true), true);
  }

  let target = null;
  let hideTimer = 0;
  let frame = 0;
  let generation = 0; // stale async menu builds must not overwrite a newer one
  let view = ""; // which menu is open: live refreshes only apply to the generic list

  // ── Atoms ─────────────────────────────────────────────────────────────
  const isHttp = (u) => /^https?:\/\//i.test(u ?? "");
  const human = (n) => {
    if (!n) return "";
    const units = t("sizeUnits").split(" ");
    const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
    return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${units[i]}`;
  };
  const safeName = (s, fallback = "video") =>
    // The cut can fall inside an emoji: half of one is not JSON RDM can read, and the download is refused.
    s.replace(/[<>:"/\\|?*\p{Cc}]/gu, "_").replace(/\s+/g, " ").trim().slice(0, 150).replace(/\p{Cs}/gu, "") || fallback;
  const pageTitle = () => safeName(top === self ? document.title : "");
  const nameOf = (url) => {
    const { pathname, hostname } = new URL(url);
    const last = pathname.split("/").pop() || hostname;
    try {
      return decodeURIComponent(last);
    } catch {
      return last; // malformed %-escape: keep it raw rather than break the menu
    }
  };
  const UNPAIRED_NOTE = `✗ ${t("unpaired")}`;
  const send = (msg) => ext.runtime.sendMessage(msg).catch(() => undefined);

  const li = (className, title, detail, onClick) => {
    const el = document.createElement("li");
    el.className = className;
    el.append(Object.assign(document.createElement("div"), { className: "title", textContent: title }));
    if (detail) el.append(Object.assign(document.createElement("div"), { className: "detail", textContent: detail }));
    if (onClick) el.addEventListener("click", onClick);
    return el;
  };
  const section = (text) => li("section", text);
  const note = (text) => li("note", text);
  const show = (...items) => {
    menu.replaceChildren(...items);
    menu.hidden = false;
  };

  /** What the menu says once a download was handed to RDM, or not. */
  const handOffNote = (ok) => {
    if (ok === true) return t("sent");
    if (ok === "unpaired") return UNPAIRED_NOTE;
    if (ok === "busy") return t("busy");
    return t("notRunningShort");
  };

  // `via`: how RDM must fetch the link — `{ user_agent, bare }` for YouTube, `{}` elsewhere.
  const download = (url, filename, audio_url, via = {}) => async () => {
    const ok = await send({ kind: "download", url, audio_url, filename, ...via });
    show(note(handOffNote(ok)));
    setTimeout(hide, ok === true ? 1400 : 5000);
  };
  const action = (title, detail, url, filename, audio_url, via) =>
    li("action", title, detail, download(url, filename, audio_url, via));

  // ── HLS: quality picker (resolved by RDM) ────────────────────────────
  /** Items for an HLS stream already probed by RDM; only what RDM can deliver with sound. */
  function hlsItems(url, info, base, via) {
    const container = info?.fmp4 ? "mp4" : "ts";
    const usable = (info?.variants ?? []).filter((v) => !v.audio || info.fmp4);
    if (!usable.length) return [action(`${base}.${container}`, t("bestQuality"), url, `${base}.${container}`, undefined, via)];
    return usable.map((v) => {
      const kbps = `${Math.round(v.bandwidth / 1000)} kb/s`;
      const quality = v.height ? `${v.height}p` : kbps;
      const filename = `${base} (${quality}).${container}`;
      return action(filename, `${quality} · ${kbps}${v.audio ? ` · ${t("separateAudio")}` : ""}`, v.url, filename, v.audio ?? undefined, via);
    });
  }

  async function hlsMenu(url, base, via = {}) {
    const gen = ++generation;
    view = "hls";
    show(note(t("readingQualities")));
    const info = await send({ kind: "probe", url, ...via });
    if (gen !== generation) return;
    if (!info) {
      show(note(t("unreadableStream")));
      return;
    }
    show(section(t("chooseQuality")), ...hlsItems(url, info, base, via));
  }

  // ── YouTube: formats resolved in-page by youtube.js ──────────────────
  function youtubeFormats() {
    return new Promise((resolve) => {
      const nonce = crypto.randomUUID();
      const listening = new AbortController();
      const timer = setTimeout(() => done({ error: t("timedOut") }), YT_TIMEOUT);
      function done(result) {
        listening.abort();
        clearTimeout(timer);
        resolve(result);
      }
      addEventListener(
        "message",
        (e) => {
          if (e.source === window && e.data?.source === "rdm-yt" && e.data.nonce === nonce) done(e.data.result ?? {});
        },
        { signal: listening.signal },
      );
      window.postMessage({ source: "rdm", type: "yt-resolve", nonce }, location.origin);
    });
  }

  // Only YouTube's stream servers: a page script could otherwise answer with arbitrary URLs.
  const isYouTubeMedia = (u) => {
    try {
      const { protocol, hostname } = new URL(u);
      return protocol === "https:" && hostname.endsWith(".googlevideo.com");
    } catch {
      return false;
    }
  };

  /** Menu items for one client's direct links (all verified to download with `via`). */
  function formatItems(formats, base, via) {
    const items = [];
    const bestAudio = formats.filter((f) => f.audio && !f.video && f.container === "mp4").sort((a, b) => b.bitrate - a.bitrate)[0];
    const videos = new Map(); // height → best video-only MP4
    for (const video of formats.filter((f) => f.video && !f.audio && f.container === "mp4")) {
      const cur = videos.get(video.height);
      if (!cur || video.bitrate > cur.bitrate) videos.set(video.height, video);
    }
    if (bestAudio && videos.size) {
      items.push(section(t("videoAudioMerged")));
      for (const v of [...videos.values()].sort((a, b) => b.height - a.height)) {
        const label = v.label || `${v.height}p`;
        const filename = `${base} (${label}).mp4`;
        const detail = [label, human(v.size + bestAudio.size), "MP4"].filter(Boolean).join(" · ");
        items.push(action(filename, detail, v.url, filename, bestAudio.url, via));
      }
    }
    // The container becomes the file's extension: only the ones YouTube serves, whatever a page
    // script answers in youtube.js's place.
    const muxed = formats.filter((f) => f.video && f.audio && /^(mp4|webm|3gpp)$/.test(f.container)).sort((a, b) => b.height - a.height);
    if (muxed.length) {
      items.push(section(t("videoWithSound")));
      for (const m of muxed) {
        const filename = `${base} (${m.label || `${m.height}p`}).${m.container}`;
        const detail = [m.label, human(m.size), m.container.toUpperCase()].filter(Boolean).join(" · ");
        items.push(action(filename, detail, m.url, filename, undefined, via));
      }
    }
    const audios = formats.filter((f) => f.audio && !f.video).sort((a, b) => b.bitrate - a.bitrate);
    if (audios.length) {
      items.push(section(t("audioOnly")));
      for (const a of audios.slice(0, 2)) {
        const suffix = a.container === "mp4" ? "m4a" : "weba";
        const filename = `${base}.${suffix}`;
        const detail = [`${Math.round(a.bitrate / 1000)} kb/s`, human(a.size), suffix.toUpperCase()].filter(Boolean).join(" · ");
        items.push(action(filename, detail, a.url, filename, undefined, via));
      }
    }
    return items;
  }

  const describeRefusal = (res) => (res?.status ? `HTTP ${res.status}` : t("unreachable"));

  async function youtubeMenu() {
    const gen = ++generation;
    view = "youtube";
    show(note(t("findingQualities")));
    const yt = await youtubeFormats();
    if (gen !== generation) return;
    if (yt.error === "no-video") {
      show(note(t("openWatchPage")));
      return;
    }
    const base = safeName(yt.title || pageTitle());
    const notes = [...(yt.errors ?? []), ...(yt.error ? [String(yt.error)] : [])];
    const items = [];

    // Direct links: the first client whose links really download from RDM wins.
    show(note(t("checkingLinks")));
    for (const c of yt.clients ?? []) {
      const formats = c.formats.filter((f) => isYouTubeMedia(f.url));
      const sample = formats.find((f) => f.audio && !f.video) ?? formats[0];
      if (!sample) continue;
      const via = { user_agent: c.ua, bare: true };
      const res = await send({ kind: "check", url: sample.url, ...via });
      if (gen !== generation) return;
      if (res?.ok) {
        items.push(...formatItems(formats, base, via));
        break;
      }
      notes.push(t("linksRefused", c.client, describeRefusal(res)));
    }

    // Refused to the page's clients: RDM's YouTube module (yt-dlp with the anti-bot token).
    if (!items.length) {
      const extra = await moduleItems(gen, base, notes);
      if (!extra) return; // a newer menu took over
      items.push(...extra);
    }

    // HLS as an alternative (or the only way): listed once RDM could read it.
    for (const c of yt.clients ?? []) {
      if (!c.hls || !isYouTubeMedia(c.hls)) continue;
      const via = { user_agent: c.ua, bare: true };
      const info = await send({ kind: "probe", url: c.hls, ...via });
      if (gen !== generation) return;
      if (info?.variants) {
        items.push(section(t("hlsStream")), ...hlsItems(c.hls, info, base, via));
        break;
      }
      notes.push(t("hlsUnreadable", c.client));
    }

    if (yt.drm) {
      items.push(note(t("drm")));
    } else {
      // Why, in short (the last refusals): otherwise nothing tells a network block from a bug.
      if (!items.length) items.push(li("note", t("directRefused"), notes.slice(-3).join(" · ")));
      // Always available: records what YouTube's own player receives (see capture.js).
      const minutes = target?.duration ? Math.ceil(target.duration / 2 / 60) : null;
      const detail = t("recordDetail", minutes ? t("aboutMinutes", minutes) : "");
      items.push(section(t("recording")), li("action", `${base}.mp4`, detail, () => startRecording(`${base}.mp4`)));
    }
    show(...items);
  }

  const videoIdOf = (href) => {
    const url = new URL(href);
    const id = url.searchParams.get("v") ?? url.pathname.match(/^\/(?:shorts|live)\/([\w-]{11})/)?.[1];
    return /^[\w-]{11}$/.test(id ?? "") ? id : null;
  };

  /**
   * Items from RDM's YouTube module: its links (verified like the others), or the offer to install
   * it. `null` when a newer menu took over meanwhile.
   */
  async function moduleItems(gen, base, notes) {
    const id = videoIdOf(location.href);
    if (!id) return [];
    const mod = await send({ kind: "yt-module" });
    if (gen !== generation) return null;
    if (mod === "unpaired") return [note(UNPAIRED_NOTE)];
    if (!mod?.state) return [];
    if (mod.state === "installing") return [note(t("ytModuleInstalling"))];
    if (mod.state !== "ready") {
      const offer = [section(t("ytModule")), li("action", t("ytModuleInstall"), t("ytModuleDetail"), installModule)];
      return mod.state === "failed" ? [note(t("ytModuleFailed", String(mod.error ?? ""))), ...offer] : offer;
    }
    show(note(t("ytModuleResolving")));
    const res = await send({ kind: "yt-extract", id });
    if (gen !== generation) return null;
    if (!res) return [];
    if (!Array.isArray(res.formats)) {
      notes.push(t("ytModuleError", String(res.error ?? "")));
      return [];
    }
    const formats = res.formats.filter((f) => isYouTubeMedia(f.url));
    const sample = formats.find((f) => f.audio && !f.video) ?? formats[0];
    if (!sample) return [];
    const via = { user_agent: res.ua, bare: true };
    const checked = await send({ kind: "check", url: sample.url, ...via });
    if (gen !== generation) return null;
    if (checked?.ok) return formatItems(formats, base, via);
    notes.push(t("linksRefused", "yt-dlp", describeRefusal(checked)));
    return [];
  }

  async function installModule() {
    show(note(t("ytModuleStarting")));
    const mod = await send({ kind: "yt-module", action: "install" });
    if (mod === "unpaired") show(note(UNPAIRED_NOTE));
    else if (!mod?.state) show(note(t("notRunningShort")));
    else show(note(mod.state === "ready" ? t("ytModuleReady") : t("ytModuleInstalling")));
  }

  // ── YouTube recording: page reload in recording mode, relay to RDM, progress banner ──
  async function startRecording(filename) {
    const videoId = new URL(location.href).searchParams.get("v");
    if (!videoId) {
      show(note(t("openWatchPage")));
      return;
    }
    show(note(t("preparingRecording")));
    const token = await send({ kind: "record-start", filename });
    if (typeof token !== "string" || token === "unpaired") {
      show(note(token === "unpaired" ? UNPAIRED_NOTE : t("notRunningShort")));
      return;
    }
    sessionStorage.setItem("rdm-record", JSON.stringify({ token, videoId, at: Date.now() }));
    location.reload(); // the player must start afresh with the recorder in place (capture.js)
  }

  function recorder(token) {
    const banner = document.createElement("rdm-recorder");
    const shadow = banner.attachShadow({ mode: "closed" });
    shadow.innerHTML = `
      <style>
        :host { all: initial; position: fixed; top: 14px; left: 50%; transform: translateX(-50%); z-index: 2147483647;
                font: 13px/1.4 Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
        .bar { display: flex; align-items: center; gap: 12px; padding: 10px 12px 10px 16px; border-radius: 999px;
               color: #eaeef8; background: #111626eb; -webkit-backdrop-filter: blur(18px) saturate(1.4);
               backdrop-filter: blur(18px) saturate(1.4); border: 1px solid #ffffff1f;
               box-shadow: 0 16px 40px #00000080, 0 0 0 1px #6a5cff33; animation: drop .25s ease-out; }
        @keyframes drop { from { opacity: 0; transform: translateY(-8px); } }
        .dot { width: 10px; height: 10px; border-radius: 50%; background: #fb7185; box-shadow: 0 0 0 4px #fb718533;
               animation: pulse 1.2s ease-in-out infinite; }
        .done .dot { background: #34d399; box-shadow: 0 0 0 4px #34d39933; animation: none; }
        @keyframes pulse { 50% { box-shadow: 0 0 0 7px #fb718500; opacity: .6; } }
        button { border: 1px solid #ffffff26; background: #ffffff14; color: #eaeef8; border-radius: 999px; padding: 4px 12px;
                 cursor: pointer; font: inherit; font-weight: 600; transition: background .12s ease; }
        button:hover { background: #ffffff26; }
      </style>
      <div class="bar"><span class="dot"></span><span class="text"></span><button></button></div>`;
    const bar = shadow.querySelector(".bar");
    const text = shadow.querySelector(".text");
    const cancel = shadow.querySelector("button");
    text.textContent = t("rdmPreparing");
    cancel.textContent = t("cancel");
    document.documentElement.append(banner);

    const relay = document.createElement("iframe");
    relay.src = `${ext.runtime.getURL("relay.html")}#${token}`;
    relay.style.display = "none";
    const relayOrigin = new URL(relay.src).origin;
    const control = (type) => window.postMessage({ source: "rdm-rec-control", token, type }, location.origin);
    const toRelay = (msg, transfer = []) => relay.contentWindow?.postMessage(msg, relayOrigin, transfer);
    const started = Date.now();
    let over = false;
    const timeLeft = (secs) => {
      if (secs === null) return "";
      return secs >= 60 ? t("minutesLeft", Math.ceil(secs / 60)) : t("secondsLeft", secs);
    };

    const finish = (message, ok) => {
      over = true;
      text.textContent = message;
      cancel.remove();
      bar.classList.toggle("done", ok);
      // The relay goes too, what it was still sending done by then: nothing stays behind in the tab.
      setTimeout(() => {
        banner.remove();
        relay.remove();
      }, ok ? 8000 : 15000);
    };

    addEventListener("message", (e) => {
      const msg = e.data;
      if (msg?.token !== token) return;
      if (e.source === window && msg.source === "rdm-rec") {
        // Player → RDM (zero-copy transfer of the media chunk).
        toRelay(msg, msg.data instanceof ArrayBuffer ? [msg.data] : []);
        if (msg.type === "progress" && !over && msg.ad) {
          text.textContent = t("adPlaying");
        } else if (msg.type === "progress" && !over) {
          const f = Math.min(Math.max(msg.fraction, 0), 1);
          const left = f > 0.02 ? Math.round(((Date.now() - started) / 1000) * (1 - f) / f) : null;
          text.textContent = msg.paused && f < 0.01 ? t("clickToStart") : t("recordingProgress", Math.floor(f * 100), timeLeft(left));
        }
        if (msg.type === "end") text.textContent = t("merging");
        // capture.js (page world, no translations) sends reason codes.
        if (msg.type === "abort") finish(t("recordingAborted", msg.reason === "video-changed" ? t("videoChanged") : String(msg.reason)), false);
      } else if (e.source === relay.contentWindow && msg.source === "rdm-relay") {
        if (msg.type === "loaded") control("ready");
        if (msg.type === "finished") finish(t("recordingDone"), true);
        if (msg.type === "error") {
          control("cancel");
          finish(t("recordingInterrupted", msg.reason), false);
        }
      }
    });
    cancel.addEventListener("click", () => {
      control("cancel");
      toRelay({ source: "rdm-rec", token, type: "abort" });
      finish(t("recordingCancelled"), false);
    });
    document.documentElement.append(relay);
  }

  const recordingToken = document.documentElement.getAttribute("data-rdm-recording");
  if (onYouTube && top === self && recordingToken && /^[0-9a-f]{64}$/.test(recordingToken)) recorder(recordingToken);

  // ── Generic pages: <video> sources + media seen on the network ───────
  const elementSources = (el) =>
    [el.currentSrc, el.src, ...[...el.querySelectorAll("source")].map((s) => s.src)]
      .filter(isHttp)
      .map((url) => ({ url, size: 0, type: el.tagName.toLowerCase() }));

  async function genericMenu() {
    const gen = ++generation;
    view = "generic";
    const network = ((await send({ kind: "list" })) ?? []).filter((m) => !m.model);
    if (gen !== generation) return;
    const items = unique([...(target ? elementSources(target) : []), ...network.sort((a, b) => b.size - a.size)]);
    if (!items.length) {
      show(note(t("noStream")));
      return;
    }
    const base = pageTitle();
    show(
      section(t("detectedVideos")),
      ...items.map((m) =>
        m.hls
          ? li("action", t("hlsName", base), t("chooseQualityMore"), () => hlsMenu(m.url, base))
          : action(nameOf(m.url), [human(m.size), m.type].filter(Boolean).join(" · "), m.url, nameOf(m.url)),
      ),
    );
  }

  // ── 3D models: <model-viewer>, WebGL viewers (Sketchfab…), model links ──
  // Mirrors `isModelUrl` in shared.js (content scripts are not modules).
  const MODEL_FILE =
    /\.(3dm|3ds|3mf|abc|blend|c4d|dae|drc|fbx|glb|gltf|iges|igs|ifc|ksplat|lwo|ma|max|mb|obj|off|ply|skp|splat|spz|stl|step|stp|usd|usda|usdc|usdz|vrm|vrml|wrl|x3d|x3db|x3dv|osgjs(\.gz)?)$|\/model_file(?!_wireframe)[^/]*\.(binz|bin(\.gz)?)$/i;
  const MODEL_SOURCES = "model-viewer, a[href], a-asset-item[src], [gltf-model], [obj-model], [ply-model], inline[url], object[data], embed[src]";
  const SCAN_EVERY = 2000;
  const isModelTarget = (el) => el?.tagName === "CANVAS" || el?.tagName === "MODEL-VIEWER";
  const absolute = (u) => {
    try {
      return new URL(u, document.baseURI).href;
    } catch {
      return "";
    }
  };
  const isModelFile = (u) => {
    try {
      return MODEL_FILE.test(new URL(u).pathname);
    } catch {
      return false;
    }
  };
  // Attribute values: a URL, `url(…)` (A-Frame) or a quoted list (X3D `inline url`).
  const urlsIn = (value = "") => {
    const wrapped = [...value.matchAll(/url\(\s*['"]?([^'")]+)/g)].map((m) => m[1]);
    return wrapped.length ? wrapped : value.split(/["'\s]+/).filter(Boolean);
  };

  let hasModels = false; // the network already delivered a 3D model to this tab
  let asked = false;
  let scan = { at: 0, items: [] };
  /** Models the page itself points to (cached: called while the pointer moves). */
  function domModels() {
    if (Date.now() - scan.at < SCAN_EVERY) return scan.items;
    const items = [];
    for (const el of document.querySelectorAll(MODEL_SOURCES)) {
      if (el.tagName === "MODEL-VIEWER") {
        // Its sources are models whatever their address looks like.
        for (const attr of ["src", "ios-src"]) if (el.getAttribute(attr)) items.push(absolute(el.getAttribute(attr)));
        continue;
      }
      const values = ["href", "src", "data", "url", "gltf-model", "obj-model", "ply-model"].map((a) => el.getAttribute(a)).filter(Boolean);
      for (const u of values.flatMap(urlsIn).map(absolute)) if (isModelFile(u)) items.push(u);
    }
    scan = { at: Date.now(), items: items.filter(isHttp).map((url) => ({ url, size: 0, type: "", model: true })) };
    return scan.items;
  }
  const modelsHere = () => hasModels || domModels().length > 0;

  const unique = (items) => {
    const seen = new Set();
    return items.filter(({ url }) => isHttp(url) && !seen.has(url) && seen.add(url));
  };
  // Sketchfab titles: "Name - Download Free 3D model by Author - Sketchfab" → "Name".
  const modelBase = () => safeName(document.title.replace(/\s+-\s+(Download Free\s+)?3D model by\s.*$/i, ""), "model");
  /** Generic file names (`file.binz`, `scene.gltf`, hashes) get the page's title in front. */
  const modelName = (url, base) => {
    const name = nameOf(url);
    return /^(file|model_file|model|scene|mesh|object|untitled|default|asset|data)[._-]|^[\da-f-]{20,}\./i.test(name)
      ? safeName(`${base} - ${name}`, name)
      : name;
  };
  const formatOf = (name) => (/\.([^.]+(\.gz)?)$/i.exec(name)?.[1] ?? "").toUpperCase();

  async function modelMenu() {
    const gen = ++generation;
    view = "model";
    const network = ((await send({ kind: "list" })) ?? []).filter((m) => m.model);
    if (gen !== generation) return;
    if (network.length) hasModels = true;
    const items = unique([...domModels(), ...network.sort((a, b) => b.size - a.size)]);
    if (!items.length) {
      show(note(t("noModel")));
      return;
    }
    const base = modelBase();
    show(
      section(t("detectedModels")),
      ...items.map((m) => {
        const name = modelName(m.url, base);
        return action(name, [human(m.size), formatOf(name)].filter(Boolean).join(" · "), m.url, name);
      }),
    );
  }

  const openMenu = () => {
    if (isModelTarget(target)) return modelMenu();
    return onYouTube ? youtubeMenu() : genericMenu();
  };

  // ── Positioning ───────────────────────────────────────────────────────
  function place() {
    if (!target?.isConnected) {
      hide();
      return;
    }
    const r = target.getBoundingClientRect();
    host.style.top = `${Math.max(4, r.top + 8)}px`;
    host.style.left = `${Math.max(4, r.right - btn.offsetWidth - 8)}px`;
  }

  function showButton(el) {
    buildOverlay();
    clearTimeout(hideTimer);
    const container = document.fullscreenElement?.tagName === "VIDEO" ? null : document.fullscreenElement;
    const parent = container ?? document.documentElement;
    if (host.parentNode !== parent) parent.append(host);
    if (target !== el) menu.hidden = true;
    target = el;
    label.textContent = t(isModelTarget(el) ? "downloadModel" : "downloadVideo");
    btn.title = label.textContent; // the mini button's only text
    btn.setAttribute("aria-label", label.textContent);
    host.style.display = "block";
    place();
  }

  function hide() {
    host.style.display = "none";
    menu.hidden = true;
    target = null;
    generation++;
  }

  const scheduleHide = () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => menu.hidden && hide(), HIDE_DELAY);
  };

  // Live collections: no DOM query per mouse move, and a free "this page has no media" test.
  const videos = document.getElementsByTagName("video");
  const audios = document.getElementsByTagName("audio");
  const viewers = document.getElementsByTagName("model-viewer");
  // WebGL viewers draw on a <canvas>: only a 3D target once this page is known to carry models.
  const canvases = document.getElementsByTagName("canvas");

  // Players cover <video> with overlays (sometimes `pointer-events: none`), so hit-test by geometry.
  const mediaAt = (x, y) => {
    if (canvases.length && !hasModels && !asked) {
      asked = true; // models fetched before this script ran
      send({ kind: "list" }).then((list) => list?.some((m) => m.model) && (hasModels = true));
    }
    const lists = [videos, audios, viewers];
    if (canvases.length && modelsHere()) lists.push(canvases);
    for (const list of lists) {
      for (const el of list) {
        const r = el.getBoundingClientRect();
        if (r.width >= MIN_W && r.height >= MIN_H && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return el;
      }
    }
    return null;
  };

  document.addEventListener(
    "pointermove",
    ({ clientX, clientY }) => {
      // Most pages have no media at all: nothing to do, not even a layout read.
      if (frame || (!target && !videos.length && !audios.length && !viewers.length && !canvases.length)) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        const el = mediaAt(clientX, clientY);
        if (el) showButton(el);
        else if (target) scheduleHide();
      });
    },
    { passive: true, capture: true },
  );

  addEventListener("scroll", () => target && place(), { passive: true, capture: true });
  addEventListener("resize", () => target && place(), { passive: true });

  // ── Toasts: what happened to a download handed to RDM (top frame only) ──
  let toastEl = null;
  function toast(text) {
    if (top !== self || typeof text !== "string") return;
    toastEl?.remove();
    const el = document.createElement("rdm-toast");
    const shadow = el.attachShadow({ mode: "closed" });
    shadow.innerHTML = `
      <style>
        :host { all: initial; position: fixed; right: 18px; bottom: 18px; z-index: 2147483647;
                font: 13px/1.4 Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
        .t { max-width: min(420px, 80vw); padding: 11px 16px; border-radius: 14px; color: #eaeef8; background: #111626f0;
             -webkit-backdrop-filter: blur(18px) saturate(1.4); backdrop-filter: blur(18px) saturate(1.4);
             border: 1px solid #ffffff1f; box-shadow: 0 16px 40px #00000073, 0 0 0 1px #6a5cff33;
             overflow-wrap: anywhere; animation: rise .22s ease-out; cursor: default; }
        @keyframes rise { from { opacity: 0; transform: translateY(8px); } }
      </style>
      <div class="t"></div>`;
    shadow.querySelector(".t").textContent = text;
    el.addEventListener("click", () => el.remove());
    document.documentElement.append(el);
    toastEl = el;
    setTimeout(() => el.remove(), text.startsWith("✗") ? 7000 : 4000);
  }

  ext.runtime.onMessage.addListener((msg) => {
    if (msg?.kind === "media-changed") {
      if (msg.model) hasModels = true;
      if (menu?.hidden === false && view === "generic" && !msg.model) genericMenu();
      if (menu?.hidden === false && view === "model" && msg.model) modelMenu();
    }
    if (msg?.kind === "toast") toast(msg.text);
  });

  // ── Safari: no downloads API, so the clicks on links to captured types come to RDM from here ──
  if (!ext.runtime.getManifest().permissions?.includes("downloads")) {
    let types = new Set();
    const setTypes = (list) => (types = new Set((list ?? "").toLowerCase().split(/[\s,;.]+/).filter(Boolean)));
    ext.storage.local.get("links").then(({ links }) => setTypes(links), () => {});
    ext.storage.onChanged.addListener((changes, area) => area === "local" && "links" in changes && setTypes(changes.links.newValue));
    // Mirrors `isCapturable` in shared.js (content scripts are not modules).
    const wanted = (name) => {
      const type = /\.([^./\\]+)$/.exec(name)?.[1].toLowerCase() ?? "";
      return types.has(type) || /^([rz]\d{1,3}|\d{3})$/.test(type);
    };
    document.addEventListener(
      "click",
      (e) => {
        // A click made by the page's own script (`link.click()`): left to Safari and its per-site
        // download permission. Only the user's own clicks go to RDM.
        if (!e.isTrusted || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
        const link = e.composedPath().find((n) => n.tagName === "A" && isHttp(n.href));
        if (!link || !wanted(link.download || new URL(link.href).pathname)) return;
        e.preventDefault();
        const { href, download } = link;
        send({ kind: "download", url: href, filename: download || undefined }).then((sent) => {
          if (sent === true) toast(t("sentName", download || nameOf(href)));
          else location.href = href; // RDM absent or refusing: Safari downloads it, as always
        });
      },
      true,
    );
  }
})();
