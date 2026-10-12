// open-airplay UI: the tray panel, the app's only window.
//
// Rendering is by region: each region's markup is rebuilt from state, and the DOM is touched only
// when that markup changed. Inputs live in static markup and are never re-created, so typing,
// focus and clicks survive status updates. Busy state and the mirroring clock are patched in place.
const T = window.__TAURI__;
const { invoke } = T.core;
const { listen } = T.event;
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const use = (id) => `<svg><use href="#${id}"/></svg>`;

// ---------- app preferences (saved by the Rust side in the app's config folder) ----------
const prefs = { startAtLogin: false };

async function loadPrefs() {
  try {
    const p = await invoke("get_prefs");
    prefs.startAtLogin = !!p.start_at_login;
  } catch { /* keep defaults */ }
}

async function savePrefs() {
  const p = await invoke("set_prefs", { startAtLogin: prefs.startAtLogin });
  prefs.startAtLogin = !!p.start_at_login;
}

// ---------- state ----------
const ui = {
  status: { seq: 0, running: false },
  page: "home",             // "home" | "settings" | "cast"
  target: null,
  devices: [],
  scanned: false,
  scanning: false,
  scanError: null,
  busy: new Set(),          // actions in flight; only their own controls show busy
  errors: {},               // action -> plain message shown next to its control
  findings: [],
  mirrorSince: null,
  cast: { state: "idle", message: "" },
  settingsNote: null,       // { ok, text } under the settings form
  speakers: [],             // the PC's sound outputs: [{ id, name }]
};

// The header says the status once; the body never repeats it.
const HEADER = { stopped: "Off", starting: "Starting…", ready: "Ready", connecting: "Connecting…", pin: "Enter the code", restarting: "Restarting…", error: "Needs attention" };
const LIVE_HEADER = { mirroring: "Mirroring from", video: "Playing video from" };

// ---------- receiver phase ----------
// The shell sends `phase`; otherwise derive it the same way from the raw fields.
function phaseOf(s) {
  if (s.phase) return s.phase;
  if (s.pin) return "pin";
  if (s.connections > 0) return s.last_device ? "mirroring" : "connecting";
  if (s.running && s.advertised) return "ready";
  if (s.running) return "starting";
  if (s.last_error) return "error";
  return "stopped";
}
const nameOf = (s) => (s.settings && s.settings.name) || "this PC";
const deviceOf = (s) => (s.last_device ? s.last_device.replace(/ \(.*\)$/, "") : null);
const isOn = (phase) => phase !== "stopped" && phase !== "error";
const inSession = (phase) => phase === "connecting" || phase === "mirroring" || phase === "video" || phase === "pin";
const capitalise = (s) => s.charAt(0).toUpperCase() + s.slice(1);

/** Takes a status snapshot unless it's older than the one shown (pushes and replies can cross). */
function acceptStatus(s) {
  if (!s || (s.seq || 0) < (ui.status.seq || 0)) return false;
  ui.status = s;
  return true;
}

/** The saved receiver settings, every field filled in. */
function currentSettings() {
  const s = ui.status.settings || {};
  return {
    name: s.name || "",
    pin_mode: !!s.pin_mode,
    h265: !!s.h265,
    remember_pin: !!s.remember_pin,
    video_links: !!s.video_links,
    switch_to_new: !!s.switch_to_new,
    blocked: s.blocked || [],
    speaker: s.speaker || null,
  };
}

function elapsed(since) {
  const t = Math.max(0, Math.floor((Date.now() - (since || Date.now())) / 1000));
  const h = Math.floor(t / 3600), m = Math.floor((t % 3600) / 60), sec = t % 60;
  const mm = String(m).padStart(2, "0"), ss = String(sec).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function deviceIcon(d) {
  if (!d.services.includes("AirPlay") || /speaker|homepod|five/i.test(d.model || "")) return "i-speaker";
  if (/mac/i.test(d.model || "")) return "i-laptop";
  return "i-tv";
}
const deviceKind = (d) => ({ "i-tv": "Apple TV", "i-laptop": "Mac", "i-speaker": "Speaker" })[deviceIcon(d)];

// Busy and disabled states are applied by syncBusy(), not baked into region markup.
const btn = (name, label, cls = "") => `<button class="oai-btn ${cls}" data-action="${name}">${esc(label)}</button>`;
const errorLine = (action) => (ui.errors[action] ? `<p class="inline-error" role="alert">${esc(ui.errors[action])}</p>` : "");

// ---------- regions ----------
const shown = new Map();
/** Replaces a region's content only when its markup changed. */
function patch(id, html) {
  if (shown.get(id) === html) return false;
  shown.set(id, html);
  $(id).innerHTML = html;
  return true;
}

function noticeHtml() {
  const n = ui.status.notice;
  if (!n) return "";
  return `<div class="oai-banner notice"><p class="oai-grow" style="margin:0">${esc(n)}</p><button class="oai-ghost" data-action="dismiss-notice" aria-label="Dismiss">✕</button></div>${errorLine("dismiss-notice")}`;
}

function statusHtml() {
  const s = ui.status, phase = phaseOf(s), name = nameOf(s), device = deviceOf(s) || "An iPhone";
  const disconnect = btn("disconnect", "Disconnect", "is-small");
  // Only a device that said who it is can be blocked.
  const block = s.device_id ? `<button class="oai-ghost block-link" data-action="block">Block this device</button>${errorLine("block")}` : "";
  switch (phase) {
    case "starting":
      return `<div class="pop-hero"><div class="oai-spinner"></div><p>Getting ${esc(name)} ready for iPhones.</p></div>`;
    case "ready":
      return `<div class="pop-status"><div class="line"><span class="oai-dot is-live"></span><div>On your iPhone, open Control Center → Screen Mirroring → <b>${esc(name)}</b>.</div></div></div>`;
    case "connecting":
      return `<div class="pop-hero"><div class="dots" style="justify-content:center;margin:6px 0 10px"><i></i><i></i><i></i></div><p>${esc(device)} is connecting. The video window opens in a moment.</p><div class="oai-btn-row">${disconnect}</div>${errorLine("disconnect")}</div>${block}`;
    case "mirroring":
      // The clock's text is filled in by tick(), outside the compared markup.
      return `<div class="pop-hero is-live"><div class="who"><h3>${esc(device)}</h3><p data-elapsed></p></div><div class="acts">${btn("show-video", "Show video", "is-small")}${btn("disconnect", "Disconnect", "is-danger is-small")}</div></div>${errorLine("show-video")}${errorLine("disconnect")}${block}`;
    case "video": {
      // A YouTube video the phone sent: the same card as mirroring, once the first frame is in.
      const playing = !!s.video;
      const sub = playing ? "<p data-elapsed></p>" : "<p>Loading the video…</p>";
      const show = playing ? btn("show-video", "Show video", "is-small") : "";
      return `<div class="pop-hero is-live"><div class="who"><h3>${esc(device)}</h3>${sub}</div><div class="acts">${show}${btn("disconnect", "Disconnect", "is-danger is-small")}</div></div>${errorLine("show-video")}${errorLine("disconnect")}${block}`;
    }
    case "pin": {
      const digits = String(s.pin).split("").map((d) => `<span>${esc(d)}</span>`).join("");
      return `<div class="pop-hero"><p>${esc(device)} is asking for this code.</p><div class="pop-pin">${digits}</div><div class="oai-btn-row">${disconnect}</div>${errorLine("disconnect")}</div>${block}`;
    }
    case "restarting": {
      const why = s.restart_reason ? `${capitalise(s.restart_reason)}, so receiving is restarting. ` : "";
      return `<div class="pop-hero"><div class="oai-spinner"></div><p>${esc(why)}${esc(name)} will be back in a moment.</p></div>`;
    }
    case "error":
      return `<div class="pop-hero is-bad"><p>${esc(s.last_error || `iPhones can't see ${name} right now.`)}</p><p>Turn receiving back on to retry.</p></div>`;
    default:
      return `<div class="pop-hero"><p>Turn on receiving to let iPhones mirror to <b>${esc(name)}</b>.</p></div>`;
  }
}

function findingsHtml() {
  const card = (f) => {
    switch (f.kind) {
      case "public_network":
        return [`Windows treats “${esc(f.network)}” as a public network, so iPhones may not find this PC.`, "fix:public_network", btn("fix:public_network", "Open network settings", "is-small")];
      case "firewall_blocks":
        return ["Windows Firewall is blocking open-airplay, so iPhones can't connect. Windows will ask for permission.", "fix:firewall_blocks", btn("fix:firewall_blocks", "Allow in Windows Firewall", "is-small is-primary")];
      case "guest_network":
        return [`“${esc(f.network)}” looks like a guest Wi-Fi. Guest networks often stop devices from seeing each other; if your iPhone can't find this PC, put both on your main Wi-Fi.`, null, ""];
      case "other_receiver":
        return [`${esc(f.name)} is also running and may take the connection. Quit it if your iPhone connects to the wrong one.`, null, ""];
      default:
        return null;
    }
  };
  const items = ui.findings.map(card).filter(Boolean).map(([text, action, button]) =>
    `<div class="oai-banner doctor"><div class="oai-grow"><p style="margin:0">${text}</p>${button ? `<div class="oai-btn-row" style="margin-top:8px;justify-content:flex-start">${button}</div>` : ""}${action ? errorLine(action) : ""}</div></div>`);
  if (!items.length && !ui.errors["doctor-check"]) return "";
  return `<div class="doctor-list">${items.join("")}<button class="oai-ghost recheck" data-action="doctor-check">Check again</button>${errorLine("doctor-check")}</div>`;
}

function devicesEmptyText() {
  if (ui.scanning) return '<span class="oai-spinner is-small" style="display:inline-block;vertical-align:-2px;margin-right:6px"></span>Searching…';
  if (ui.scanError) return `Couldn't search: ${esc(ui.scanError)}`;
  return ui.scanned ? "No AirPlay devices found on this network." : "Press ↻ to find Apple TVs and speakers.";
}

function devicesHtml() {
  // Rows carry the device's identifier, never its list position, so a click during a scan opens the right one.
  const rows = ui.devices.map((d) => `<button class="oai-list-row" data-device="${esc(d.identifier)}"><span class="oai-ico">${use(deviceIcon(d))}</span><span class="oai-grow"><span style="display:block">${esc(d.name)}</span></span><span class="oai-meta">${deviceKind(d)}</span></button>`).join("");
  const empty = ui.devices.length && !ui.scanning ? "" : `<div class="oai-list-row oai-sub" style="color:var(--oai-ink-2)">${devicesEmptyText()}</div>`;
  return `<div class="oai-list-label">Send to <button class="oai-ghost scan-btn ${ui.scanning ? "is-spinning" : ""}" data-action="scan" style="height:22px;padding:0 6px" aria-label="Find AirPlay devices">${use("i-refresh")}</button></div>${rows}${empty}`;
}

function castHeadHtml() {
  const d = ui.target;
  return `<div class="oai-list-row"><span class="oai-ico">${use(deviceIcon(d))}</span><span class="oai-grow"><span style="display:block;font-weight:600">${esc(d.name)}</span><span class="oai-sub" style="display:block">${esc(d.model || deviceKind(d))}</span></span></div>`;
}

function castMsgHtml() {
  const c = ui.cast;
  if (c.state === "working") return `<span class="oai-spinner is-small"></span>${esc(c.message)}`;
  if (c.state === "error") return `${esc(c.message)} ${btn("retry-cast", "Retry", "is-small")}`;
  return esc(c.message);
}

function blockedHtml() {
  const list = currentSettings().blocked;
  if (!list.length) return "";
  const rows = list.map((d) => `<div class="oai-list-row"><span class="oai-grow"><span>${esc(d.name || d.id)}</span></span><button type="button" class="oai-btn is-small" data-action="unblock" data-id="${esc(d.id)}">Unblock</button></div>`).join("");
  return `<div class="oai-list-label">Blocked devices</div>${rows}`;
}

/** The Speaker choices: the Windows default, the PC's outputs, and a saved one that is unplugged. */
function speakerOptions(saved) {
  const options = [["", "Windows default"], ...ui.speakers.map((sp) => [sp.id, sp.name])];
  if (saved && !ui.speakers.some((sp) => sp.id === saved)) options.push([saved, "A speaker that isn't connected"]);
  return options.map(([id, label]) => `<option value="${esc(id)}">${esc(label)}</option>`).join("");
}

/** Copies saved values into the static settings inputs, except the one being edited. */
function syncSettingsInputs() {
  const s = currentSettings();
  const set = (sel, apply) => {
    const el = document.querySelector(sel);
    if (el && el !== document.activeElement) apply(el);
  };
  set('[data-set="name"]', (el) => { el.value = s.name; });
  set('[data-set="pin_mode"]', (el) => { el.checked = s.pin_mode; });
  set('[data-set="remember_pin"]', (el) => { el.checked = s.remember_pin; });
  set('[data-set="h265"]', (el) => { el.checked = s.h265; });
  set('[data-set="video_links"]', (el) => { el.checked = s.video_links; });
  set('[data-set="switch_to_new"]', (el) => { el.value = s.switch_to_new ? "1" : ""; });
  set('[data-set="speaker"]', (el) => {
    const html = speakerOptions(s.speaker);
    if (el.dataset.options !== html) { el.innerHTML = html; el.dataset.options = html; }
    el.value = s.speaker || "";
  });
  set('[data-pref="startAtLogin"]', (el) => { el.checked = prefs.startAtLogin; });
  // The PIN's options show only with a PIN; "Forget" only while devices are remembered.
  $("pin-options").hidden = !s.pin_mode;
  document.querySelector('[data-action="forget-paired"]').hidden = !s.remember_pin;
}

function syncBusy() {
  document.querySelectorAll("[data-action]").forEach((el) => {
    const busy = ui.busy.has(el.dataset.action);
    el.classList.toggle("is-busy", busy && el.classList.contains("oai-btn"));
    el.disabled = busy;
  });
  // The switch is only busy while turning receiving on or off.
  $("rx-switch").disabled = ui.busy.has("start") || ui.busy.has("stop");
  document.querySelectorAll("[data-set]").forEach((el) => { el.disabled = ui.busy.has("save-settings"); });
}

function tick() {
  if (!ui.mirrorSince) return;
  document.querySelectorAll("[data-elapsed]").forEach((el) => {
    const text = elapsed(ui.mirrorSince);
    if (el.textContent !== text) el.textContent = text;
  });
}

function render() {
  const s = ui.status, phase = phaseOf(s);
  if (phase === "mirroring" || (phase === "video" && s.video)) ui.mirrorSince = s.mirroring_since_ms || ui.mirrorSince || Date.now();
  else ui.mirrorSince = null;

  const sub = LIVE_HEADER[phase] ? `${LIVE_HEADER[phase]} ${deviceOf(s) || "an iPhone"}` : HEADER[phase];
  if ($("pop-sub").textContent !== sub) $("pop-sub").textContent = sub;
  const sw = $("rx-switch");
  if (sw.checked !== isOn(phase)) sw.checked = isOn(phase);
  const tip = isOn(phase) ? "Turn off receiving" : "Turn on receiving";
  if (sw.title !== tip) sw.title = tip;
  const switchError = ui.errors.start || ui.errors.stop || "";
  const swErr = $("r-switch-error");
  swErr.hidden = !switchError;
  if (swErr.textContent !== switchError) swErr.textContent = switchError;

  if (ui.page === "cast" && !ui.target) ui.page = "home";
  for (const page of ["home", "settings", "cast"]) $("page-" + page).hidden = ui.page !== page;

  if (ui.page === "home") {
    patch("r-notice", noticeHtml());
    patch("r-status", statusHtml());
    patch("r-findings", findingsHtml());
    patch("r-devices", devicesHtml());
  } else if (ui.page === "settings") {
    const note = ui.settingsNote || (s.settings_pending && inSession(phase) ? { ok: true, text: "Applies when this session ends." } : null);
    patch("r-settings-status", note ? `<span class="${note.ok ? "" : "is-bad"}">${esc(note.text)}</span>` : "");
    patch("r-blocked", blockedHtml());
    syncSettingsInputs();
  } else if (ui.page === "cast") {
    patch("r-cast-head", castHeadHtml());
    $("cast-url-row").hidden = !ui.target.services.includes("AirPlay");
    const msg = $("r-cast-msg");
    msg.className = "msg " + (ui.cast.state === "error" ? "is-bad" : ui.cast.state === "ok" ? "is-ok" : "");
    patch("r-cast-msg", castMsgHtml());
  }
  syncBusy();
  tick();
  reportHeight();
}

// Keep the mirroring clock ticking without re-rendering.
setInterval(tick, 1000);

// ---------- the panel's size follows its content ----------
let reportedHeight = 0;
function reportHeight() {
  const body = $("pop-body"), cs = getComputedStyle(body);
  const pad = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
  const height = (id) => $(id).getBoundingClientRect().height;   // fractional: the footer has a half-pixel border
  const h = Math.ceil(height("panel-head") + height("pop-content") + pad + height("panel-foot"));
  if (Math.abs(h - reportedHeight) < 1) return;
  reportedHeight = h;
  invoke("panel_fit", { height: h }).catch(() => {});
}
new ResizeObserver(reportHeight).observe($("pop-content"));

function goTo(page) {
  ui.page = page;
  render();
}

// ---------- actions ----------
/** Runs one action: only its own control shows busy, and a failure is shown next to it. */
async function run(action, fn) {
  if (ui.busy.has(action)) return;
  ui.busy.add(action);
  delete ui.errors[action];
  render();
  try {
    return await fn();
  } catch (err) {
    ui.errors[action] = String(err);
  } finally {
    ui.busy.delete(action);
    render();
  }
}

const startReceiver = (settings) => run("start", async () => { acceptStatus(await invoke("receiver_start", { settings })); });
const stopReceiver = () => run("stop", async () => { acceptStatus(await invoke("receiver_stop")); });

// ---------- sender ----------
let nextId = 1;
const pending = new Map();

function request(cmd) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    invoke("sender_send", { command: { id, ...cmd } }).catch((err) => {
      pending.delete(id);
      reject(err);
    });
  });
}

async function scan() {
  if (ui.scanning) return;
  ui.scanning = true;
  ui.scanError = null;
  render();
  try {
    const { devices } = await request({ cmd: "scan", timeout: 5 });
    ui.devices = devices.filter((d) => d.services.includes("AirPlay") || d.services.includes("RAOP"));
    ui.scanned = true;
  } catch (err) {
    ui.scanError = String(err);
  } finally {
    ui.scanning = false;
    render();
  }
}

let lastCast = null;
async function cast(cmd) {
  if (!ui.target || ui.busy.has("cast")) return;
  lastCast = cmd;
  const target = ui.target;
  ui.busy.add("cast");
  ui.cast = { state: "working", message: `Connecting to ${target.name}…` };
  render();
  try {
    await request({ ...cmd, identifier: target.identifier });
    ui.cast = { state: "ok", message: (cmd.cmd === "play_url" ? "Playing on " : "Streaming to ") + target.name };
  } catch (err) {
    ui.cast = { state: "error", message: `Couldn't cast: ${err}` };
  } finally {
    ui.busy.delete("cast");
  }
  render();
}
const castUrl = () => document.querySelector('[data-cast="url"]').value.trim();
const castFile = () => document.querySelector('[data-cast="file"]').value.trim();

function selectDevice(identifier) {
  const d = ui.devices.find((x) => x.identifier === identifier);
  if (!d) return;
  ui.target = d;
  ui.cast = { state: "idle", message: "" };
  goTo("cast");
}

// ---------- events ----------
document.addEventListener("click", (e) => {
  const dev = e.target.closest("[data-device]");
  if (dev) return selectDevice(dev.dataset.device);
  const action = e.target.closest("[data-action]");
  if (!action || action.disabled) return;
  const name = action.dataset.action;
  if (name.startsWith("fix:")) return run(name, () => invoke("doctor_fix", { kind: name.slice(4) }));
  switch (name) {
    case "disconnect": return run("disconnect", () => invoke("receiver_disconnect"));
    case "dismiss-notice": return run("dismiss-notice", async () => { acceptStatus(await invoke("receiver_dismiss_notice")); });
    case "show-video": return run("show-video", () => invoke("show_video_window"));
    case "scan": return scan();
    case "home": return goTo("home");
    case "settings": return ui.page === "settings" ? goTo("home") : openSettings();
    case "block": return run("block", async () => { acceptStatus(await invoke("receiver_block_device")); });
    case "unblock": return saveSettings({ blocked: currentSettings().blocked.filter((d) => d.id !== action.dataset.id) });
    case "forget-paired": return forgetPaired();
    case "doctor-check": return run("doctor-check", () => invoke("doctor_check"));
    case "play-url": return castUrl() && cast({ cmd: "play_url", url: castUrl() });
    case "stream-file": return castFile() && cast({ cmd: "stream_file", path: castFile() });
    case "retry-cast": return lastCast && cast(lastCast);
  }
});

$("rx-switch").addEventListener("change", (e) => {
  if (e.target.checked) startReceiver(currentSettings()); else stopReceiver();
});

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (ui.page !== "home") goTo("home");
  else invoke("panel_hide").catch(() => {});
});

document.addEventListener("change", async (e) => {
  if (e.target.dataset.set) return saveSettings();
  const key = e.target.dataset.pref;
  if (!key) return;
  const before = prefs[key];
  prefs[key] = e.target.checked;
  try {
    await savePrefs();
    ui.settingsNote = null;
  } catch (err) {
    prefs[key] = before;
    e.target.checked = before;
    ui.settingsNote = { ok: false, text: String(err) };
  }
  render();
});

/** The form's values for the settings it holds. */
function formSettings() {
  const val = (k) => document.querySelector(`.settings-form [data-set="${k}"]`);
  return {
    name: val("name").value.trim(),
    pin_mode: val("pin_mode").checked,
    remember_pin: val("remember_pin").checked,
    h265: val("h265").checked,
    video_links: val("video_links").checked,
    switch_to_new: val("switch_to_new").value === "1",
    speaker: val("speaker").value || null,
  };
}

/** Settings save as you go: the name when you press Enter or leave the field, the rest at once.
 *  `change` sets what the form doesn't hold (the blocked list). */
async function saveSettings(change = {}) {
  const saved = currentSettings();
  const settings = { ...saved, ...formSettings(), ...change };
  if (JSON.stringify(settings) === JSON.stringify(saved)) return;
  if (!settings.name) {
    document.querySelector('[data-set="name"]').value = saved.name;
    ui.settingsNote = { ok: false, text: "Enter a name. iPhones show it in Screen Mirroring." };
    return render();
  }
  ui.settingsNote = null;
  await run("save-settings", async () => { acceptStatus(await invoke("receiver_save_settings", { settings })); });
  if (ui.errors["save-settings"]) {
    ui.settingsNote = { ok: false, text: ui.errors["save-settings"] };
    delete ui.errors["save-settings"];
  }
  render();
}

async function forgetPaired() {
  ui.settingsNote = null;
  await run("forget-paired", async () => { acceptStatus(await invoke("receiver_forget_paired")); });
  const err = ui.errors["forget-paired"];
  delete ui.errors["forget-paired"];
  ui.settingsNote = err ? { ok: false, text: err } : { ok: true, text: "Forgotten. Those devices will be asked for the code again." };
  render();
}

/** Opens Settings with a fresh list of the PC's speakers (one may have been plugged in since). */
async function openSettings() {
  goTo("settings");
  try { ui.speakers = await invoke("list_speakers"); } catch { ui.speakers = []; }
  if (ui.page === "settings") render();
}

// Enter in the name field saves it (there is no Apply button).
document.addEventListener("submit", (e) => {
  if (!e.target.closest(".settings-form")) return;
  e.preventDefault();
  const name = e.target.querySelector('[data-set="name"]');
  if (document.activeElement === name) name.blur(); else saveSettings();
});

listen("receiver-status", (e) => {
  if (acceptStatus(e.payload)) render();
});

listen("sender-event", (e) => {
  const msg = e.payload;
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  msg.event === "error" ? p.reject(msg.message) : p.resolve(msg);
});

listen("doctor-findings", (e) => { ui.findings = e.payload || []; render(); });

// The sender sidecar died: fail everything in flight; the next request starts a fresh one.
listen("sender-exit", (e) => {
  const message = (e.payload && e.payload.message) || "The sender stopped unexpectedly.";
  for (const p of pending.values()) p.reject(message);
  pending.clear();
});

(async () => {
  await loadPrefs();
  try { ui.findings = await invoke("doctor_findings"); } catch { /* none yet */ }
  try {
    const status = await invoke("receiver_status");
    if (!status.settings) status.settings = await invoke("default_settings");
    acceptStatus(status);
  } catch (err) {
    ui.errors.start = String(err);
  }
  render();
})();
