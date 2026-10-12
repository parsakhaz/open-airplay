// open-airplay: casting from this PC (links and files to Apple TVs and speakers).
//
// A self-contained piece: it owns the conversation with the cast helper (requests, replies, the
// helper going away) and the "Cast media" controls on a device page. app.js mounts it into a
// container, tells it which device is open, and re-renders when it says something changed.
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
  // identifier -> { state: "connecting" | "playing" | "paused" | "stopping" | "done" | "error",
  //                 message, id (the live request), cmd (for Retry), name, timer }
  const casts = new Map();
  let changed = () => {};

  const castOf = (identifier) => casts.get(identifier) || { state: "idle", message: "" };
  const isActive = (c) => c && (c.state === "connecting" || c.state === "playing" || c.state === "paused" || c.state === "stopping");

  function update(identifier, fields) {
    const c = { ...castOf(identifier), ...fields };
    casts.set(identifier, c);
    if (current && current.identifier === identifier) renderLive();
    changed();
    return c;
  }

  function clearTimer(identifier) {
    const c = casts.get(identifier);
    if (c && c.timer) { clearTimeout(c.timer); c.timer = null; }
  }

  /** Casts a link or a file path. A speaker gets the audio stream; everything else plays it itself. */
  function cast(device, media, speaker) {
    const identifier = device.identifier, name = device.name;
    if (castOf(identifier).state === "connecting") return; // a double-click sends one request
    const cmd = speaker
      ? { cmd: "stream_file", identifier, name, path: media }
      : { cmd: "play_url", identifier, name, url: media };
    start(cmd, name);
  }

  function start(cmd, name) {
    const identifier = cmd.identifier;
    clearTimer(identifier);
    const id = send(cmd, (msg) => onCastEvent(identifier, id, msg));
    const timer = setTimeout(() => {
      if (castOf(identifier).id !== id || castOf(identifier).state !== "connecting") return;
      update(identifier, { state: "error", message: `${name} didn't answer. Check it's on and on the same network.`, timer: null });
      invoke("sender_stop", { identifier }).catch(() => {});
    }, CONNECT_TIMEOUT_MS);
    update(identifier, { state: "connecting", message: `Connecting to ${name}…`, id, cmd, name, timer });
  }

  function onCastEvent(identifier, id, msg) {
    if (castOf(identifier).id !== id) return; // a newer cast on this device replaced this one
    const name = castOf(identifier).name;
    switch (msg.event) {
      case "playing": clearTimer(identifier); return update(identifier, { state: "playing", message: `Playing on ${name}` });
      case "ok": clearTimer(identifier); return update(identifier, { state: "done", message: "Finished playing." });
      case "stopped": clearTimer(identifier); return update(identifier, { state: "idle", message: "" });
      case "error": clearTimer(identifier); return update(identifier, { state: "error", message: msg.message || "Couldn't cast. Try again." });
    }
  }

  /** Stops what this PC casts to one device. */
  async function stop(identifier) {
    const c = castOf(identifier);
    if (!isActive(c) || c.state === "stopping") return;
    clearTimer(identifier);
    const id = c.id;
    update(identifier, { state: "stopping", message: "Stopping…" });
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

  async function pause(identifier) {
    const c = castOf(identifier);
    if (c.state !== "playing" && c.state !== "paused") return;
    const pausing = c.state === "playing", id = c.id;
    update(identifier, { pending: true });
    try {
      await request({ cmd: pausing ? "pause" : "resume", identifier, name: c.name });
      if (castOf(identifier).id === id) update(identifier, { state: pausing ? "paused" : "playing", message: pausing ? `Paused on ${c.name}` : `Playing on ${c.name}`, pending: false, note: "" });
    } catch (err) {
      if (castOf(identifier).id === id) update(identifier, { pending: false, note: String(err) });
    }
  }

  function retry(identifier) {
    const c = castOf(identifier);
    if (c.cmd && !isActive(c)) start(c.cmd, c.name); // goes to the device it came from
  }

  // ---------- the "Cast media" controls ----------
  let root = null;
  let current = null; // { identifier, name, speaker }
  const drafts = new Map(); // identifier -> what was typed or chosen on that device's page

  /** Builds the controls once inside `container`. `onChange` runs whenever any cast changes. */
  function mount(container, onChange) {
    changed = onChange || (() => {});
    container.innerHTML = `
      <div class="cast-media">
        <div class="oai-list-label">Cast media</div>
        <div class="row">
          <input class="oai-input" data-cast-field type="text" spellcheck="false" autocomplete="off" aria-label="Link or file to cast" placeholder="Paste a link or choose a file">
        </div>
        <div class="row actions">
          <button class="oai-btn is-small" data-cast-action="choose">Choose file…</button>
          <button class="oai-btn is-primary is-small" data-cast-action="cast">Cast</button>
        </div>
        <div class="cast-live" data-cast-live aria-live="polite"></div>
      </div>`;
    root = container;
    const field = root.querySelector("[data-cast-field]");
    root.addEventListener("click", (e) => {
      const el = e.target.closest("[data-cast-action]");
      if (!el || el.disabled || !current) return;
      switch (el.dataset.castAction) {
        case "choose": return chooseFile();
        case "cast": return castField();
        case "stop": return stop(current.identifier);
        case "pause": return pause(current.identifier);
        case "retry": return retry(current.identifier);
      }
    });
    field.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { e.preventDefault(); castField(); }
    });
    field.addEventListener("input", () => {
      if (current && castOf(current.identifier).state === "error" && !isActive(castOf(current.identifier))) {
        update(current.identifier, { state: "idle", message: "" });
      }
      syncButtons();
    });
  }

  function castField() {
    const media = root.querySelector("[data-cast-field]").value.trim();
    if (!media) {
      return update(current.identifier, { state: "error", message: "Paste a link or choose a file first.", cmd: null });
    }
    cast(current, media, current.speaker);
  }

  async function chooseFile() {
    const dialog = T.dialog;
    if (!dialog) return update(current.identifier, { state: "error", message: "Choosing a file isn't available. Paste its path instead.", cmd: null });
    const target = current.identifier;
    let picked;
    try {
      picked = await dialog.open({ multiple: false, directory: false, title: `Choose a file to cast to ${current.name}`, filters: [{ name: "Video and audio", extensions: MEDIA }, { name: "All files", extensions: ["*"] }] });
    } catch (err) {
      return update(target, { state: "error", message: `Couldn't open the file picker. ${err}`, cmd: null });
    }
    const path = picked && (typeof picked === "string" ? picked : picked.path);
    if (!path) return;
    drafts.set(target, path);
    if (!current || current.identifier !== target) return; // the user moved on to another device
    root.querySelector("[data-cast-field]").value = path;
    syncButtons();
    root.querySelector('[data-cast-action="cast"]').focus();
  }

  /** Shows the controls for `device` (null hides nothing; the page does). */
  function show(device, speaker) {
    const field = root && root.querySelector("[data-cast-field]");
    if (field && current) drafts.set(current.identifier, field.value);
    current = device ? { identifier: device.identifier, name: device.name, speaker: !!speaker } : null;
    if (!root || !current) return;
    field.value = drafts.get(current.identifier) || "";
    field.placeholder = speaker ? "Paste an audio link or choose a file" : "Paste a link or choose a file";
    renderLive();
  }

  let shownLive = null;
  function renderLive() {
    if (!root || !current) return;
    const c = castOf(current.identifier);
    let html = "";
    const btn = (action, label, cls = "") => `<button class="oai-btn is-small ${cls}" data-cast-action="${action}">${esc(label)}</button>`;
    if (c.state === "connecting" || c.state === "stopping") {
      html = `<div class="msg"><span class="oai-spinner is-small"></span><span>${esc(c.message)}</span></div>`;
      if (c.state === "connecting") html += `<div class="oai-btn-row">${btn("stop", "Stop")}</div>`;
    } else if (c.state === "playing" || c.state === "paused") {
      html = `<div class="msg is-ok"><span class="oai-dot is-live"></span><span>${esc(c.message)}</span></div>
        <div class="oai-btn-row">${btn("pause", c.state === "paused" ? "Resume" : "Pause")}${btn("stop", "Stop", "is-danger")}</div>
        ${c.note ? `<p class="inline-error" role="alert">${esc(c.note)}</p>` : ""}`;
    } else if (c.state === "error") {
      html = `<div class="msg is-bad" role="alert"><span>${esc(c.message)}</span>${c.cmd ? btn("retry", "Retry") : ""}</div>`;
    } else if (c.state === "done") {
      html = `<div class="msg"><span>${esc(c.message)}</span></div>`;
    }
    const live = root.querySelector("[data-cast-live]");
    if (html !== shownLive) {
      shownLive = html;
      live.innerHTML = html;
    }
    syncButtons();
  }

  function syncButtons() {
    if (!root || !current) return;
    const c = castOf(current.identifier);
    const castBtn = root.querySelector('[data-cast-action="cast"]');
    const connecting = c.state === "connecting" || c.state === "stopping";
    castBtn.disabled = connecting;
    castBtn.classList.toggle("is-busy", c.state === "connecting");
    const pauseBtn = root.querySelector('[data-cast-action="pause"]');
    if (pauseBtn) { pauseBtn.disabled = !!c.pending; pauseBtn.classList.toggle("is-busy", !!c.pending); }
  }

  /** "Playing" / "Connecting…" for a device row, or null when nothing is being cast to it. */
  function rowStatus(identifier) {
    const c = casts.get(identifier);
    if (!c) return null;
    if (c.state === "connecting") return "Connecting…";
    if (c.state === "playing") return "Playing";
    if (c.state === "paused") return "Paused";
    return null;
  }

  return { scan, mount, show, stop, rowStatus, isActive: (identifier) => isActive(casts.get(identifier)) };
})();
