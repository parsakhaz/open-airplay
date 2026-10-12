// open-airplay: casting from this PC (links and files to Apple TVs and speakers).
//
// A self-contained piece: it owns the conversation with the cast helper (requests, replies, the
// helper going away) and the cast part of a device page (the approved device-page design: Cast media
// tile, drop zone, link fallback, the casting card with one Stop). app.js mounts it into the page,
// tells it which device is open, and re-renders when it says something changed.
// Each device keeps its own cast state, so casting to one never shows on or blocks another.
window.OapCast = (() => {
  const T = window.__TAURI__;
  const { invoke } = T.core;
  const { listen } = T.event;
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  // UI-side limits, a little longer than the helper's own (scan 8 s, connect 12 s, start 2 s).
  const SCAN_TIMEOUT_MS = 12000;
  const CONNECT_TIMEOUT_MS = 18000;
  const STOP_FALLBACK_MS = 3000;
  const REQUEST_TIMEOUT_MS = 15000;
  const FINAL = new Set(["ok", "stopped", "error", "devices", "pong", "health"]);
  const MEDIA = ["mp4", "m4v", "mov", "mp3", "m4a", "aac", "wav", "flac", "aiff", "aif", "alac", "ogg"];

  // ---------- talking to the helper ----------
  let nextId = 1;
  const handlers = new Map(); // request id -> onEvent(msg)

  /** Sends one command; every reply with its id goes to onEvent until a final one. Returns the id. */
  function send(cmd, onEvent) {
    const id = nextId++;
    handlers.set(id, onEvent);
    invoke("sender_send", { command: { id, ...cmd } }).catch((err) => {
      if (handlers.delete(id)) onEvent({ id, event: "error", code: "unavailable", message: String(err) });
    });
    return id;
  }

  /** One command, one final reply: resolves with it, or rejects with a plain sentence. */
  function request(cmd, timeoutMs = REQUEST_TIMEOUT_MS, timeoutMessage = "That took too long. Try again.") {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { handlers.delete(id); reject(timeoutMessage); }, timeoutMs);
      const id = send(cmd, (msg) => {
        if (!FINAL.has(msg.event)) return;
        clearTimeout(timer);
        msg.event === "error" ? reject(msg.message) : resolve(msg);
      });
    });
  }

  listen("sender-event", (e) => {
    const msg = e.payload || {};
    const onEvent = handlers.get(msg.id);
    if (!onEvent) return;
    if (FINAL.has(msg.event)) handlers.delete(msg.id);
    onEvent(msg);
  });

  // The helper went away: everything in flight fails; the next request starts a fresh one.
  listen("sender-exit", (e) => {
    const message = (e.payload && e.payload.message) || "Casting stopped unexpectedly. Try again.";
    const all = [...handlers.entries()];
    handlers.clear();
    for (const [id, onEvent] of all) onEvent({ id, event: "error", code: "exited", message });
  });

  /** Finds Apple TVs and speakers. Never spins longer than SCAN_TIMEOUT_MS. */
  async function scan() {
    const { devices } = await request({ cmd: "scan", timeout: 5 }, SCAN_TIMEOUT_MS, "Searching took too long. Try ↻ again.");
    return devices.filter((d) => d.services.includes("AirPlay") || d.services.includes("RAOP"));
  }

  // ---------- casts, one per device ----------
  // identifier -> { state: "idle" | "connecting" | "playing" | "stopping" | "error", message,
  //                 id (the live request), name, media (what plays, for its label), timer }
  const casts = new Map();
  let changed = () => {};

  const castOf = (identifier) => casts.get(identifier) || { state: "idle", message: "" };
  const isActive = (c) => !!c && (c.state === "connecting" || c.state === "playing" || c.state === "stopping");
  /** What a link or path is called on screen: its file name. */
  const mediaName = (media) => {
    const s = String(media || "").split(/[?#]/)[0].replace(/[\\/]+$/, "");
    const last = s.split(/[\\/]/).pop();
    try { return decodeURIComponent(last) || s; } catch { return last || s; }
  };

  function update(identifier, fields) {
    const c = { ...castOf(identifier), ...fields };
    casts.set(identifier, c);
    if (current && current.identifier === identifier) renderPage();
    changed();
    return c;
  }

  function clearTimer(identifier) {
    const c = casts.get(identifier);
    if (c && c.timer) { clearTimeout(c.timer); c.timer = null; }
  }

  /** Casts a link or a file path. A speaker gets the audio stream; everything else plays it itself. */
  function cast(device, media) {
    const { identifier, name, speaker, address } = device;
    if (castOf(identifier).state === "connecting") return; // a double-click sends one request
    const cmd = speaker
      ? { cmd: "stream_file", identifier, name, path: media }
      : { cmd: "play_url", identifier, name, url: media };
    if (address) cmd.address = address; // reaches a device added by address that no scan has seen
    clearTimer(identifier);
    const id = send(cmd, (msg) => onCastEvent(identifier, id, msg));
    const timer = setTimeout(() => {
      if (castOf(identifier).id !== id || castOf(identifier).state !== "connecting") return;
      update(identifier, { state: "error", message: `${name} didn't answer. Check it's on and on the same network.`, timer: null });
      invoke("sender_stop", { identifier }).catch(() => {});
    }, CONNECT_TIMEOUT_MS);
    update(identifier, { state: "connecting", message: "", id, name, media, timer });
  }

  function onCastEvent(identifier, id, msg) {
    if (castOf(identifier).id !== id) return; // a newer cast on this device replaced this one
    clearTimer(identifier);
    switch (msg.event) {
      case "playing": return update(identifier, { state: "playing" });
      case "ok": case "stopped": return update(identifier, { state: "idle", message: "" });
      case "error": return update(identifier, { state: "error", message: msg.message || "Couldn't cast. Try again.", open: true });
    }
  }

  /** Stops what this PC casts to one device. */
  async function stop(identifier) {
    const c = castOf(identifier);
    if (!isActive(c) || c.state === "stopping") return;
    clearTimer(identifier);
    const id = c.id;
    update(identifier, { state: "stopping" });
    try {
      await invoke("sender_stop", { identifier });
    } catch (err) {
      return update(identifier, { state: "error", message: String(err) });
    }
    // The ended cast reports "stopped"; if the helper had already gone there is nothing to report.
    setTimeout(() => {
      const now = castOf(identifier);
      if (now.id === id && now.state === "stopping") update(identifier, { state: "idle", message: "" });
    }, STOP_FALLBACK_MS);
  }

  // ---------- the device page's cast part ----------
  // Built once inside the page's container; regions are patched only when their markup changes, and
  // the link field is static, so typing survives every update.
  let root = null;
  let current = null; // { identifier, name, speaker, address, mirrorTile }
  const pages = new Map(); // identifier -> { open (Cast media tile), link (link field shown), draft }
  const pageOf = (identifier) => pages.get(identifier) || { open: false, link: false, draft: "" };
  const setPage = (identifier, fields) => pages.set(identifier, { ...pageOf(identifier), ...fields });
  let dragging = null; // { name } while a file is dragged over the panel

  const ICON = {
    cast: '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.3" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M6.7 5.4 10.7 8l-4 2.6z" fill="currentColor"/></svg>',
    drop: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2v7.5M5 6.8 8 9.8l3-3"/><path d="M2.5 10v2a1.5 1.5 0 0 0 1.5 1.5h8a1.5 1.5 0 0 0 1.5-1.5v-2"/></svg>',
    music: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M6 12.2V3.5l7-1.5v8.7"/><circle cx="4.3" cy="12.2" r="1.7"/><circle cx="11.3" cy="10.7" r="1.7"/></svg>',
    warn: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 1.8 15 14H1z"/><path d="M8 6v3.6M8 11.6v.4" stroke-width="1.6"/></svg>',
  };

  /** Builds the page body once inside `container`. `onChange` runs whenever any cast changes. */
  function mount(container, onChange) {
    changed = onChange || (() => {});
    container.innerHTML = `
      <div class="send-body">
        <div data-r="ban"></div>
        <div data-r="live"></div>
        <div data-r="tiles"></div>
        <div data-r="drop"></div>
        <div data-r="link">
          <button class="linkbtn" data-cast-action="link">Paste a link instead</button>
          <div class="cast-link" hidden>
            <div class="field">
              <input class="oai-input" data-cast-field type="text" spellcheck="false" autocomplete="off" aria-label="Link to cast" placeholder="https://">
              <button class="oai-btn is-primary" data-cast-action="cast">Cast</button>
            </div>
            <div class="hint">A direct link to a video or song file. Enter also casts.</div>
          </div>
        </div>
      </div>`;
    root = container;
    const field = root.querySelector("[data-cast-field]");
    root.addEventListener("click", (e) => {
      const el = e.target.closest("[data-cast-action]");
      if (!el || el.disabled || !current) return;
      switch (el.dataset.castAction) {
        case "open": setPage(current.identifier, { open: !pageOf(current.identifier).open }); return renderPage();
        case "choose": return chooseFile();
        case "link": setPage(current.identifier, { link: true }); renderPage(); return field.focus();
        case "cast": return castField();
        case "stop": return stop(current.identifier);
      }
    });
    field.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { e.preventDefault(); castField(); }
    });
    field.addEventListener("input", () => setPage(current.identifier, { draft: field.value }));
    watchDrops();
  }

  /** Files dragged onto the panel: the window reports real paths (a web drop would not). */
  function watchDrops() {
    const webview = T.webview && T.webview.getCurrentWebview && T.webview.getCurrentWebview();
    if (!webview || !webview.onDragDropEvent) return;
    webview.onDragDropEvent(({ payload }) => {
      if (!current || !acceptsCast()) return;
      if (payload.type === "enter") {
        dragging = { name: mediaName((payload.paths || [])[0]) };
        setPage(current.identifier, { open: true });
      } else if (payload.type === "leave") {
        dragging = null;
      } else if (payload.type === "drop") {
        dragging = null;
        const path = (payload.paths || [])[0];
        if (path) cast(current, path);
      } else {
        return;
      }
      renderPage();
    });
  }

  /** Whether the open page currently takes a cast (not while one is running there). */
  const acceptsCast = () => current && !isActive(castOf(current.identifier));

  function castField() {
    const media = root.querySelector("[data-cast-field]").value.trim();
    if (!media) return root.querySelector("[data-cast-field]").focus();
    cast(current, media);
  }

  async function chooseFile() {
    const dialog = T.dialog;
    const target = current;
    if (!dialog) return update(target.identifier, { state: "error", message: "Choosing a file isn't available. Paste a link instead." });
    let picked;
    try {
      picked = await dialog.open({ multiple: false, directory: false, title: `Choose a song or video to cast to ${target.name}`, filters: [{ name: "Songs and videos", extensions: MEDIA }, { name: "All files", extensions: ["*"] }] });
    } catch (err) {
      return update(target.identifier, { state: "error", message: `Couldn't open the file picker. ${err}` });
    }
    const path = picked && (typeof picked === "string" ? picked : picked.path);
    if (path) cast(target, path); // choosing a file is the cast
  }

  /**
   * Shows the page for `device`. `options.speaker`: it plays audio streams. `options.mirrorTile`:
   * markup for the Mirror screen tile when this device can take a mirror (the mirror code supplies
   * it); without one the page is Cast media alone, like a speaker's.
   */
  function show(device, options = {}) {
    const field = root && root.querySelector("[data-cast-field]");
    if (field && current) setPage(current.identifier, { draft: field.value });
    current = device ? { identifier: device.identifier, name: device.name, address: device.address, speaker: !!options.speaker, mirrorTile: options.mirrorTile || null } : null;
    dragging = null;
    if (!root || !current) return;
    field.value = pageOf(current.identifier).draft || "";
    renderPage();
  }

  const shown = new Map();
  function region(name, html) {
    if (shown.get(name) === html) return;
    shown.set(name, html);
    root.querySelector(`[data-r="${name}"]`).innerHTML = html;
  }

  function renderPage() {
    if (!root || !current) return;
    const c = castOf(current.identifier), page = pageOf(current.identifier);
    const active = isActive(c);
    const tiles = !!current.mirrorTile && !active;
    const dropOpen = !active && (!tiles || page.open || c.state === "error" || !!dragging);

    region("ban", c.state === "error" ? `<div class="send-ban" role="alert">${ICON.warn}<span>${esc(c.message)}</span></div>` : "");
    region("live", active ? liveHtml(c) : "");
    region("tiles", tiles ? `<div class="send-tiles">${current.mirrorTile}<button class="send-tile ${dropOpen ? "is-open" : ""}" data-cast-action="open" aria-expanded="${dropOpen}"><span class="ti">${ICON.cast}</span><span><b>Cast media</b><small>A song, video or link</small></span></button></div>` : "");
    region("drop", dropOpen ? dropHtml() : "");
    const linkRegion = root.querySelector('[data-r="link"]');
    linkRegion.hidden = !dropOpen;
    root.querySelector(".linkbtn").hidden = page.link;
    root.querySelector(".cast-link").hidden = !page.link;
    const castBtn = root.querySelector('[data-cast-action="cast"]');
    castBtn.disabled = c.state === "connecting";
    castBtn.classList.toggle("is-busy", c.state === "connecting");
  }

  function dropHtml() {
    if (dragging) {
      return `<div class="send-drop is-hover"><span class="di">${ICON.drop}</span><b>Drop to cast to ${esc(current.name)}</b><span>${esc(dragging.name)}</span></div>`;
    }
    return `<div class="send-drop"><span class="di">${ICON.drop}</span><b>Drop a song or video here</b><button class="oai-btn is-small" data-cast-action="choose">Choose file…</button></div>`;
  }

  function liveHtml(c) {
    const sub = c.state === "connecting" ? `<span class="oai-spinner is-small"></span>Connecting to ${esc(c.name)}…`
      : c.state === "stopping" ? "Stopping…"
      : `Playing on ${esc(c.name)}`;
    return `<div class="send-live"><div class="media"><span class="art">${ICON.music}</span><div><b>${esc(mediaName(c.media))}</b><small>${sub}</small></div></div>
      <button class="oai-btn is-danger wide" data-cast-action="stop" ${c.state === "stopping" ? "disabled" : ""}>Stop</button></div>`;
  }

  /** A device's cast state for its row and header: { text, kind: "live" | "busy" } or null. */
  function rowStatus(identifier) {
    const c = casts.get(identifier);
    if (!c) return null;
    if (c.state === "connecting") return { text: "Connecting…", kind: "busy" };
    if (c.state === "playing") return { text: `Playing ${mediaName(c.media)}`, kind: "live", short: "Playing" };
    return null;
  }

  return { scan, mount, show, stop, rowStatus, isActive: (identifier) => isActive(casts.get(identifier)) };
})();
