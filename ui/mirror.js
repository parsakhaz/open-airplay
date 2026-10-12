// open-airplay: mirroring this PC to Apple TVs (the Mirror screen tile, the Mirror sheet, the code sheet).
//
// A self-contained piece, like cast.js: it owns the mirror status the shell pushes ("mirror-status"),
// builds the sheet once inside a container and patches its regions when they change. app.js asks it
// for the tile on a device page, for a device's live state, and for the sheet's header.
// What the sheet shows comes from MirrorSheet.mirrorSheet (mirror-sheet.js), a pure view model.
// Slots (data-slot="mirror-…") are static elements other pieces mount into; they survive updates.
window.OapMirror = (() => {
  const T = window.__TAURI__;
  const { invoke } = T.core;
  const { listen } = T.event;
  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const use = (id) => `<svg><use href="#${id}"/></svg>`;
  const SENDING = new Set(["connecting", "code", "mirroring"]);

  const state = {
    snapshot: { targets: [] },
    ticked: [],          // TV addresses ticked in the sheet
    displays: [],        // this PC's displays; the picker shows with two or more
    display: null,       // the chosen display's index
    busy: new Set(),     // "start", "stop", "code"
    error: null,         // a failed command, in plain words
    codeKey: null,       // which prompt the code boxes belong to, so a new prompt starts empty
    adding: false,       // while mirroring: the Add a device picker is open
  };
  let root = null;
  let host = { changed: () => {}, devices: () => [], back: () => {} };

  // ---------- what a tick, Mirror, × and Stop do ----------
  // Several TVs at once: ticks add up, each TV starts on its own, × removes one TV, Stop all ends all.
  const actions = {
    tick(ip) {
      state.ticked = state.ticked.includes(ip) ? state.ticked.filter((t) => t !== ip) : [...state.ticked, ip];
    },
    async start(targets) {
      for (const d of targets) {
        await invoke("mirror_start", { target: d.ip, port: d.port ?? null, name: d.name, display: state.displays.length > 1 ? state.display : null });
      }
    },
    remove: (target) => invoke("mirror_stop", { target: target ?? null }),
    stopAll: () => invoke("mirror_stop", { target: null }),
    sendCode: (target, code) => invoke("mirror_code", { target: target ?? null, code }),
  };

  // ---------- status ----------
  const targets = () => state.snapshot.targets || [];
  const entryFor = (ip) => targets().find((s) => s.target === ip) || null;
  const sending = () => targets().some((s) => SENDING.has(s.phase));
  const clock = (since) => `<span data-mirror-elapsed="${since}"></span>`;

  /** A device's live mirror state for its row and header, or null: { html, cls, short }. */
  function rowStatus(ip) {
    const s = ip && entryFor(ip);
    if (!s) return null;
    switch (s.phase) {
      case "mirroring": return { html: `<span class="oai-dot is-live"></span>Mirroring · ${clock(s.since_ms)}`, short: `<span class="oai-dot is-live"></span>Mirroring`, cls: "s-live" };
      case "connecting": return { html: `<span class="oai-spinner is-small"></span>Connecting…`, short: `<span class="oai-spinner is-small"></span>Connecting…`, cls: "s-busy" };
      case "code": return { html: "Enter the code", short: "Enter the code", cls: "s-busy" };
      default: return null;
    }
  }

  /** The Mirror screen tile for a device page. Its clock fills in place, so the markup stays stable. */
  function tile(d) {
    const s = entryFor(d.address);
    let small = "Show this PC on the TV";
    if (s && s.phase === "mirroring") small = `<span class="m-tile-live"><span class="m-pulse"></span>Mirroring · ${clock(s.since_ms)}</span>`;
    else if (s && s.phase === "connecting") small = "Connecting…";
    else if (s && s.phase === "code") small = "Enter the code";
    return `<button class="send-tile primary" data-action="mirror"><span class="ti">${use("i-mirror")}</span><span><b>Mirror screen</b><small>${small}</small></span></button>`;
  }

  // ---------- the sheet ----------
  /** The TVs the sheet offers: { ip, port, name, type, icon }, from the device list. */
  const offered = () => host.devices();

  function model() {
    const devices = offered().map((d) => ({ name: d.name, ip: d.ip }));
    return MirrorSheet.mirrorSheet({ snapshot: state.snapshot, devices, ticked: state.ticked, adding: state.adding });
  }

  /** The sheet's header: { icon, title, sub } (sub is markup). */
  function head() {
    const m = model();
    if (m.mode === "code") {
      return { title: "Mirror screen", sub: m.code.credential === "password" ? `${use("i-lock")}Password needed` : "Enter the code" };
    }
    if (m.mode === "live") {
      if (m.live.since_ms != null) {
        const count = m.live.count > 1 ? ` · ${m.live.count} devices` : "";
        return { title: "Mirroring", sub: `<span class="oai-dot is-live"></span>${clock(m.live.since_ms)}${count}` };
      }
      return { title: "Mirror screen", sub: `<span class="oai-spinner is-small"></span>Connecting…` };
    }
    return { title: "Mirror screen", sub: "Choose where" };
  }

  function mount(container, options) {
    host = { ...host, ...options };
    container.innerHTML = `
      <div class="send-body mirror-sheet">
        <div data-m="ban"></div>
        <div data-m="hero"></div>
        <div data-m="sect"></div>
        <div class="m-rows" data-m="rows" data-slot="mirror-targets"></div>
        <div data-slot="mirror-add"></div>
        <div data-m="displays"></div>
        <div data-slot="mirror-advanced"></div>
        <div data-m="note"></div>
        <div class="m-code" data-m-code hidden>
          <h4 data-m-code-title></h4>
          <p data-m-code-help></p>
          <p class="m-code-msg" data-m-code-msg role="alert" hidden></p>
          <div class="m-digits" data-m-digits>
            <input class="m-digit" inputmode="numeric" maxlength="1" autocomplete="off" aria-label="Digit 1">
            <input class="m-digit" inputmode="numeric" maxlength="1" autocomplete="off" aria-label="Digit 2">
            <input class="m-digit" inputmode="numeric" maxlength="1" autocomplete="off" aria-label="Digit 3">
            <input class="m-digit" inputmode="numeric" maxlength="1" autocomplete="off" aria-label="Digit 4">
          </div>
          <form class="m-password" data-m-password autocomplete="off">
            <input class="oai-input" type="password" aria-label="Password" data-m-password-field>
            <div class="m-btns"><button type="button" class="oai-btn" data-mirror="cancel-code">Cancel</button><button class="oai-btn is-primary" data-mirror="send-password">Continue</button></div>
          </form>
          <button class="oai-ghost m-cancel" data-mirror="cancel-code" data-m-pin-cancel>Cancel</button>
        </div>
        <div data-m="button"></div>
      </div>`;
    root = container;
    root.addEventListener("click", onClick);
    const digits = [...root.querySelectorAll(".m-digit")];
    digits.forEach((el, i) => {
      el.addEventListener("input", () => {
        const all = el.value.replace(/\D/g, "");
        if (all.length > 1) {
          // A pasted code fills the boxes from here on.
          [...all].slice(0, digits.length - i).forEach((ch, k) => { digits[i + k].value = ch; });
        } else {
          el.value = all;
        }
        const next = digits.find((d) => !d.value);
        if (next) next.focus();
        else submitCode(digits.map((d) => d.value).join(""));
      });
      el.addEventListener("keydown", (e) => {
        if (e.key === "Backspace" && !el.value && i > 0) { digits[i - 1].value = ""; digits[i - 1].focus(); e.preventDefault(); }
      });
    });
    root.querySelector("[data-m-password]").addEventListener("submit", (e) => {
      e.preventDefault();
      const field = root.querySelector("[data-m-password-field]");
      if (field.value) submitCode(field.value);
    });
  }

  /** Opens the sheet from a device page: that device is ticked, unless a mirror is already running. */
  function open(device) {
    state.error = null;
    state.adding = false;
    if (!sending()) state.ticked = device && device.address ? [device.address] : [];
    loadDisplays();
  }

  async function loadDisplays() {
    try {
      const displays = await invoke("mirror_displays");
      state.displays = displays || [];
      if (!state.displays.some((d) => d.index === state.display)) {
        state.display = (state.displays.find((d) => d.primary) || state.displays[0] || {}).index ?? null;
      }
    } catch {
      state.displays = []; // no picker; the sender uses its default display
    }
    host.changed();
  }

  const shown = new Map();
  function region(name, html) {
    if (shown.get(name) === html) return;
    shown.set(name, html);
    root.querySelector(`[data-m="${name}"]`).innerHTML = html;
  }

  function rowHtml(r, mode) {
    const d = offered().find((x) => x.ip === r.target) || { icon: "i-tv", type: "" };
    const sub = r.sub
      ? `<small class="${r.tone ? `s-${r.tone}` : ""}">${r.tone === "busy" && r.sub.endsWith("…") ? '<span class="oai-spinner is-small"></span>' : ""}${esc(r.sub)}</small>`
      : `<small>${esc(d.type)}</small>`;
    const body = `<span class="ico">${use(d.icon)}</span><span class="grow"><b>${esc(r.name)}</b>${sub}</span>`;
    if (mode === "pick") {
      return `<button class="m-row ${r.ticked ? "is-on" : ""}" data-mirror="tick" data-target="${esc(r.target)}" aria-pressed="${!!r.ticked}">${body}<span class="check ${r.ticked ? "on" : ""}">${r.ticked ? use("i-check") : ""}</span></button>`;
    }
    return `<div class="m-row">${body}<button class="x" data-mirror="remove" data-target="${esc(r.target)}" aria-label="Stop mirroring to ${esc(r.name)}">${use("i-x")}</button></div>`;
  }

  function render() {
    if (!root) return;
    const m = model();
    const code = m.mode === "code";
    const failed = targets().some((s) => s.phase === "error");
    const banner = state.error || m.banner;
    region("ban", banner && !code ? `<div class="send-ban ${failed || state.error ? "" : "is-info"}" role="alert">${use("i-warn")}<span>${esc(banner)}</span></div>` : "");

    if (m.mode === "live") {
      const live = m.live.since_ms != null;
      const tv = (offered().find((d) => d.ip === (m.rows[0] || {}).target) || { icon: "i-tv" }).icon;
      region("hero", `<div class="m-hero"><div class="m-flow"><span class="end">${use("i-pc")}</span><span class="dots3"><i></i><i></i><i></i></span><span class="end ${live ? "is-live" : ""}">${use(tv)}</span></div><div class="mini">${live ? `<b>Mirroring this screen</b> · ${clock(m.live.since_ms)}` : "<b>Connecting…</b>"}</div></div>`);
      region("note", live ? `<div class="m-note">${use("i-sound")}Sound stays on this PC.</div>` : "");
    } else {
      region("hero", "");
      region("note", "");
    }
    region("sect", m.mode === "pick" ? '<div class="m-sect">Mirror to</div>' : "");
    region("rows", code ? "" : m.rows.map((r) => rowHtml(r, m.mode)).join(""));

    const showDisplays = m.mode === "pick" && state.displays.length > 1;
    region("displays", showDisplays ? `<div class="m-sect">Display</div><div class="m-displays">${state.displays.map((d) => `<button class="m-disp ${d.index === state.display ? "on" : ""}" data-mirror="display" data-index="${d.index}" aria-pressed="${d.index === state.display}"><span class="scr">${d.index + 1}</span>${d.width}×${d.height}</button>`).join("")}</div>` : "");
    root.querySelector('[data-slot="mirror-advanced"]').hidden = m.mode !== "pick";
    const add = root.querySelector('[data-slot="mirror-add"]');
    add.hidden = m.mode !== "live";
    if (m.mode !== "live") state.adding = false;
    else renderAdd(add, m.add);

    let button = "";
    if (m.mode === "pick") {
      const busy = state.busy.has("start");
      button = `<button class="oai-btn is-primary m-big ${busy ? "is-busy" : ""}" data-mirror="start" ${m.button.disabled || busy ? "disabled" : ""}>${esc(m.button.label)}</button>`;
    } else if (m.mode === "live") {
      const busy = state.busy.has("stop");
      button = `<button class="oai-btn is-danger m-wide ${busy ? "is-busy" : ""}" data-mirror="stop-all" ${busy ? "disabled" : ""}>${esc(m.button.label)}</button>`;
    }
    region("button", button);
    renderCode(m);
    tick();
  }

  /** Add a device: one row while mirroring; pressed, it brings back the ticks for the other TVs. */
  let addShown = null;
  function renderAdd(slot, add) {
    let html;
    if (!add.open) {
      html = `<div class="m-rows"><button class="m-row m-add" data-mirror="add-open"><span class="ico">${use("i-plus")}</span><span class="grow"><b>Add a device</b></span></button></div>`;
    } else if (!add.rows.length) {
      html = `<div class="m-rows"><div class="m-row m-add-none"><span class="grow"><small>No other devices to add.</small></span><button class="oai-ghost" data-mirror="add-close">Done</button></div></div>`;
    } else {
      const busy = state.busy.has("start");
      html = `<div class="m-sect">Add a device</div><div class="m-rows">${add.rows.map((r) => rowHtml(r, "pick")).join("")}</div>
        <div class="m-btns m-add-btns"><button class="oai-btn" data-mirror="add-close">Cancel</button><button class="oai-btn is-primary ${busy ? "is-busy" : ""}" data-mirror="add" ${add.button.disabled || busy ? "disabled" : ""}>${esc(add.button.label)}</button></div>`;
    }
    if (html === addShown) return;
    addShown = html;
    slot.innerHTML = html;
  }

  function renderCode(m) {
    const box = root.querySelector("[data-m-code]");
    box.hidden = m.mode !== "code";
    if (m.mode !== "code") { state.codeKey = null; return; }
    const c = m.code, password = c.credential === "password";
    const set = (sel, text) => { const el = root.querySelector(sel); if (el.textContent !== text) el.textContent = text; };
    set("[data-m-code-title]", password ? `${c.name} needs its password` : `Enter the code shown on ${c.name}`);
    set("[data-m-code-help]", password ? "The one set under AirPlay → Require Password on the TV." : "You only need it the first time. It continues by itself after the last digit.");
    const msg = root.querySelector("[data-m-code-msg]");
    set("[data-m-code-msg]", c.message || "");
    msg.hidden = !c.message;
    root.querySelector("[data-m-digits]").hidden = password;
    root.querySelector("[data-m-pin-cancel]").hidden = password;
    root.querySelector("[data-m-password]").hidden = !password;
    // A new prompt (another TV, or a new code after a wrong one) starts with empty boxes.
    const key = `${c.target}|${c.credential}|${c.message || ""}`;
    if (key !== state.codeKey) {
      state.codeKey = key;
      root.querySelectorAll(".m-digit").forEach((d) => { d.value = ""; d.disabled = false; });
      root.querySelector("[data-m-password-field]").value = "";
      state.busy.delete("code");
      setTimeout(() => (password ? root.querySelector("[data-m-password-field]") : root.querySelector(".m-digit")).focus(), 0);
    }
  }

  // ---------- actions ----------
  async function act(name, fn) {
    if (state.busy.has(name)) return;
    state.busy.add(name);
    state.error = null;
    host.changed();
    try {
      await fn();
    } catch (err) {
      state.error = String(err);
    } finally {
      state.busy.delete(name);
      host.changed();
    }
  }

  function submitCode(code) {
    const m = model();
    if (m.mode !== "code" || state.busy.has("code")) return;
    root.querySelectorAll(".m-digit").forEach((d) => { d.disabled = true; });
    act("code", () => actions.sendCode(m.code.target, code));
  }

  function onClick(e) {
    const el = e.target.closest("[data-mirror]");
    if (!el || el.disabled) return;
    const target = el.dataset.target;
    switch (el.dataset.mirror) {
      case "tick":
        actions.tick(target);
        return host.changed();
      case "display":
        state.display = Number(el.dataset.index);
        return host.changed();
      case "start": {
        const chosen = offered().filter((d) => state.ticked.includes(d.ip));
        // A TV that failed and has left the device list can still be retried from its row.
        for (const ip of state.ticked) if (!chosen.some((d) => d.ip === ip)) chosen.push({ ip, name: (entryFor(ip) || {}).device || ip });
        return act("start", () => actions.start(chosen));
      }
      case "add-open":
        state.adding = true;
        state.ticked = [];
        return host.changed();
      case "add-close":
        state.adding = false;
        state.ticked = [];
        return host.changed();
      case "add": {
        const chosen = offered().filter((d) => state.ticked.includes(d.ip));
        state.adding = false;
        state.ticked = [];
        return act("start", () => actions.start(chosen));
      }
      case "remove":
        return act("stop", () => actions.remove(target));
      case "stop-all": {
        // Back in the sheet, the TVs that were mirroring stay ticked, so the main button is the retry.
        const was = targets().filter((s) => SENDING.has(s.phase)).map((s) => s.target);
        return act("stop", async () => { await actions.stopAll(); state.ticked = was; });
      }
      case "cancel-code": {
        const m = model();
        // Cancel ends the attempt and returns to the page the sheet was opened from.
        act("stop", () => actions.remove(m.code && m.code.target));
        return host.back();
      }
      case "send-password": {
        e.preventDefault();
        const field = root.querySelector("[data-m-password-field]");
        if (field.value) submitCode(field.value);
        return;
      }
    }
  }

  // ---------- the clock ----------
  function elapsed(since) {
    const t = Math.max(0, Math.floor((Date.now() - since) / 1000));
    const h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), s = t % 60;
    const mm = String(m).padStart(2, "0"), ss = String(s).padStart(2, "0");
    return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
  }
  function tick() {
    document.querySelectorAll("[data-mirror-elapsed]").forEach((el) => {
      const text = elapsed(Number(el.dataset.mirrorElapsed));
      if (el.textContent !== text) el.textContent = text;
    });
  }
  setInterval(tick, 1000);

  listen("mirror-status", (e) => {
    state.snapshot = e.payload || { targets: [] };
    host.changed();
  });
  invoke("mirror_status").then((s) => { state.snapshot = s || { targets: [] }; host.changed(); }).catch(() => {});

  return { mount, open, render, head, tile, rowStatus, tick, actions, isSending: sending };
})();
