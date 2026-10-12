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
const prefs = { startAtLogin: false, addedDevices: [] };

async function loadPrefs() {
  try {
    const p = await invoke("get_prefs");
    prefs.startAtLogin = !!p.start_at_login;
    prefs.addedDevices = Array.isArray(p.added_devices) ? p.added_devices : [];
  } catch { /* keep defaults */ }
}

/** After a device is added or removed by address: reload the list of added devices and redraw. */
async function refreshAddedDevices() {
  await loadPrefs();
  render();
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
  lastScan: 0,              // when the device list was last refreshed (ms)
  busy: new Set(),          // actions in flight; only their own controls show busy
  errors: {},               // action -> plain message shown next to its control
  findings: [],
  mirrorSince: null,
  settingsNote: null,       // { ok, text } under the settings form
};

// The header says the status once; the body never repeats it.
const HEADER = { stopped: "Off", starting: "Starting…", ready: "Ready", connecting: "Connecting…", pin: "Enter the code", restarting: "Restarting…", error: "Needs attention" };

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
const inSession = (phase) => phase === "connecting" || phase === "mirroring" || phase === "pin";
const capitalise = (s) => s.charAt(0).toUpperCase() + s.slice(1);

/** Takes a status snapshot unless it's older than the one shown (pushes and replies can cross). */
function acceptStatus(s) {
  if (!s || (s.seq || 0) < (ui.status.seq || 0)) return false;
  ui.status = s;
  return true;
}

function currentSettings() {
  const s = ui.status.settings || {};
  return { name: s.name || "", pin_mode: !!s.pin_mode, h265: !!s.h265 };
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
const isSpeaker = (d) => deviceIcon(d) === "i-speaker";
/** A friendly type: the model's name, unless it's a model id (Mac17,9) or unknown. */
const friendlyType = (d) => (d.model && !/^[A-Za-z]+\d+,\d+$/.test(d.model) && !/unknown/i.test(d.model) ? d.model : deviceKind(d));
const canCast = (d) => d.services.includes("AirPlay") || d.services.includes("RAOP");
/** The Send to rows: scanned devices, joined with devices added by address when that code is present. */
const sendRows = () => (window.AddDevice ? AddDevice.merge(ui.devices, prefs.addedDevices) : ui.devices);
/** A row's state line: what's being sent to it, else how it was added, else its type. */
function rowSub(d) {
  const mirror = OapMirror.rowStatus(d.address);
  if (mirror) return { html: mirror.html, cls: mirror.cls };
  const live = OapCast.rowStatus(d.identifier);
  if (live) return { html: live.kind === "busy" ? `<span class="oai-spinner is-small"></span>${esc(live.text)}` : `<span class="oai-dot is-live"></span>${esc(live.text)}`, cls: live.kind === "busy" ? "s-busy" : "s-live" };
  return { html: esc(d.added ? "Added by address" : friendlyType(d)), cls: "" };
}

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
  switch (phase) {
    case "starting":
      return `<div class="pop-hero"><div class="oai-spinner"></div><p>Getting ${esc(name)} ready for iPhones.</p></div>`;
    case "ready":
      return `<div class="pop-status"><div class="line"><span class="oai-dot is-live"></span><div>On your iPhone, open Control Center → Screen Mirroring → <b>${esc(name)}</b>.</div></div></div>`;
    case "connecting":
      return `<div class="pop-hero"><div class="dots" style="justify-content:center;margin:6px 0 10px"><i></i><i></i><i></i></div><p>${esc(device)} is connecting. The video window opens in a moment.</p><div class="oai-btn-row">${disconnect}</div>${errorLine("disconnect")}</div>`;
    case "mirroring":
      // The clock's text is filled in by tick(), outside the compared markup.
      return `<div class="pop-hero is-live"><div class="who"><h3>${esc(device)}</h3><p data-elapsed></p></div><div class="acts">${btn("show-video", "Show video", "is-small")}${btn("disconnect", "Disconnect", "is-danger is-small")}</div></div>${errorLine("show-video")}${errorLine("disconnect")}`;
    case "pin": {
      const digits = String(s.pin).split("").map((d) => `<span>${esc(d)}</span>`).join("");
      return `<div class="pop-hero"><p>${esc(device)} is asking for this code.</p><div class="pop-pin">${digits}</div><div class="oai-btn-row">${disconnect}</div>${errorLine("disconnect")}</div>`;
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
  if (ui.scanError) return esc(ui.scanError);
  return ui.scanned ? "No AirPlay devices found on this network." : "Looking for Apple TVs and speakers…";
}

function devicesHtml() {
  // Rows carry the device's identifier, never its list position, so a click during a scan opens the right one.
  const list = sendRows();
  const rows = list.map((d) => {
    const sub = rowSub(d);
    return `<button class="oai-list-row send-row" data-device="${esc(d.identifier)}"><span class="oai-ico">${use(deviceIcon(d))}</span><span class="oai-grow"><b>${esc(d.name)}</b><small class="${sub.cls}">${sub.html}</small></span><svg class="chev"><use href="#i-chev"/></svg></button>`;
  }).join("");
  const empty = list.length && !ui.scanning ? "" : `<div class="oai-list-row oai-sub" style="color:var(--oai-ink-2)">${devicesEmptyText()}</div>`;
  // ux-add-device adds its "Add device by address…" row after the rows.
  return `<div class="oai-list-label">Send to <button class="oai-ghost scan-btn ${ui.scanning ? "is-spinning" : ""}" data-action="scan" style="height:22px;padding:0 6px" aria-label="Find AirPlay devices">${use("i-refresh")}</button></div>${rows}${empty}`;
}

/** The neutral header of a device page: back, the device, its type or what's being sent to it. */
function sendHeadHtml() {
  const d = ui.target, live = OapCast.rowStatus(d.identifier), mirror = OapMirror.rowStatus(d.address);
  const sub = mirror ? mirror.short : live ? (live.kind === "busy" ? `<span class="oai-spinner is-small"></span>${esc(live.text)}` : `<span class="oai-dot is-live"></span>${esc(live.short || live.text)}`) : esc(friendlyType(d));
  return `<button class="send-back" data-action="home" aria-label="Back">${use("i-back")}</button><span class="dico">${use(deviceIcon(d))}</span><div class="tt"><div class="t">${esc(d.name)}</div><div class="s">${sub}</div></div>`;
}

/** The Mirror sheet's header: back to where it was opened from, and what mirroring is doing. */
function mirrorHeadHtml() {
  const h = OapMirror.head();
  return `<button class="send-back" data-action="back" aria-label="Back">${use("i-back")}</button><span class="dico is-accent">${use("i-mirror")}</span><div class="tt"><div class="t">${esc(h.title)}</div><div class="s">${h.sub}</div></div>`;
}

/** TVs the Mirror sheet offers: everything in Send to except speakers, joined by address. */
const mirrorTargets = () => sendRows().filter((d) => !isSpeaker(d) && d.address)
  .map((d) => ({ ip: d.address, port: d.port ?? null, name: d.name, type: friendlyType(d), icon: deviceIcon(d) }));

/** Copies saved values into the static settings inputs, except the one being edited. */
function syncSettingsInputs() {
  const s = currentSettings();
  const set = (sel, apply) => {
    const el = document.querySelector(sel);
    if (el && el !== document.activeElement) apply(el);
  };
  set('[data-set="name"]', (el) => { el.value = s.name; });
  set('[data-set="pin_mode"]', (el) => { el.checked = s.pin_mode; });
  set('[data-set="h265"]', (el) => { el.checked = s.h265; });
  set('[data-pref="startAtLogin"]', (el) => { el.checked = prefs.startAtLogin; });
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
  if (phase === "mirroring") ui.mirrorSince = s.mirroring_since_ms || ui.mirrorSince || Date.now();
  else ui.mirrorSince = null;

  const sub = phase === "mirroring" ? `Mirroring from ${deviceOf(s) || "an iPhone"}` : HEADER[phase];
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
  for (const page of ["home", "settings", "cast", "mirror"]) $("page-" + page).hidden = ui.page !== page;
  // Send pages have their own header and no footer.
  const sendPage = ui.page === "cast" || ui.page === "mirror";
  $("panel-head").hidden = sendPage;
  $("panel-foot").hidden = sendPage;
  $("send-head").hidden = !sendPage;

  if (ui.page === "home") {
    patch("r-notice", noticeHtml());
    patch("r-status", statusHtml());
    patch("r-findings", findingsHtml());
    patch("r-devices", devicesHtml());
  } else if (ui.page === "settings") {
    const note = ui.settingsNote || (s.settings_pending && inSession(phase) ? { ok: true, text: "Applies when this session ends." } : null);
    patch("r-settings-status", note ? `<span class="${note.ok ? "" : "is-bad"}">${esc(note.text)}</span>` : "");
    syncSettingsInputs();
  } else if (ui.page === "cast") {
    patch("send-head", sendHeadHtml());
  } else if (ui.page === "mirror") {
    patch("send-head", mirrorHeadHtml());
    OapMirror.render();
  }
  syncBusy();
  tick();
  OapMirror.tick();
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
  const h = Math.ceil(height("panel-head") + height("send-head") + height("pop-content") + pad + height("panel-foot"));
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

// ---------- devices (casting itself lives in cast.js) ----------
const RESCAN_AFTER_MS = 60000;

async function scan() {
  if (ui.scanning) return;
  ui.scanning = true;
  ui.scanError = null;
  render();
  try {
    ui.devices = await OapCast.scan();
    ui.scanned = true;
    // Keep the open device page pointing at the fresh entry for the same device.
    if (ui.target) ui.target = sendRows().find((d) => d.identifier === ui.target.identifier) || ui.target;
  } catch (err) {
    ui.scanError = String(err);
  } finally {
    ui.scanning = false;
    ui.lastScan = Date.now();
    render();
  }
}

/** Devices appear by themselves: on start, when the panel opens (if the list is old), after a network change. */
function scanIfStale() {
  if (Date.now() - ui.lastScan > RESCAN_AFTER_MS) scan();
}

function selectDevice(identifier) {
  const d = sendRows().find((x) => x.identifier === identifier);
  if (!d) return;
  ui.target = d;
  showDevicePage();
  goTo("cast");
}

/** Fills the device page. The Mirror screen tile comes from the mirror code (window.OapMirror) for a
 *  device that can take a mirror; until that code is present the page is Cast media alone. */
function showDevicePage() {
  const d = ui.target;
  const mirrorTile = !isSpeaker(d) && window.OapMirror && OapMirror.tile ? OapMirror.tile(d) : null;
  if (canCast(d)) OapCast.show(d, { speaker: isSpeaker(d), mirrorTile });
  $("cast-media").hidden = !canCast(d);
}

/** Opens the Mirror sheet from the open device page. */
function openMirror() {
  ui.mirrorFrom = ui.page;
  OapMirror.open(ui.target);
  goTo("mirror");
}

/** Back goes up one level: the Mirror sheet returns to the page it was opened from. */
function back() {
  goTo(ui.page === "mirror" && ui.mirrorFrom === "cast" && ui.target ? "cast" : "home");
}

/** A mirror status change: refresh the device page's tile (it carries the live state), then render. */
function mirrorChanged() {
  if (ui.page === "cast" && ui.target) showDevicePage();
  render();
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
    case "back": return back();
    case "mirror": return openMirror();
    case "settings": return goTo(ui.page === "settings" ? "home" : "settings");
    case "doctor-check": return run("doctor-check", () => invoke("doctor_check"));
  }
});

$("rx-switch").addEventListener("change", (e) => {
  if (e.target.checked) startReceiver(currentSettings()); else stopReceiver();
});

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (ui.page !== "home") back();
  else invoke("panel_hide").catch(() => {});
});

document.addEventListener("change", async (e) => {
  if (e.target.dataset.set) return saveSettings(e.target);
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

/** Settings save as you go: the name when you press Enter or leave the field, switches at once. */
async function saveSettings(field) {
  const form = field.closest(".settings-form");
  const val = (k) => form.querySelector(`[data-set="${k}"]`);
  const settings = { name: val("name").value.trim(), pin_mode: val("pin_mode").checked, h265: val("h265").checked };
  const saved = currentSettings();
  if (settings.name === saved.name && settings.pin_mode === saved.pin_mode && settings.h265 === saved.h265) return;
  if (!settings.name) {
    val("name").value = saved.name;
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

// Enter in the name field saves it (there is no Apply button).
document.addEventListener("submit", (e) => {
  if (!e.target.closest(".settings-form")) return;
  e.preventDefault();
  const name = e.target.querySelector('[data-set="name"]');
  if (document.activeElement === name) name.blur(); else saveSettings(name);
});

listen("receiver-status", (e) => {
  if (acceptStatus(e.payload)) render();
});

listen("doctor-findings", (e) => { ui.findings = e.payload || []; render(); });

listen("panel-shown", scanIfStale);
listen("network-changed", () => { ui.lastScan = 0; scan(); });

OapCast.mount($("cast-media"), render);
OapMirror.mount($("mirror-sheet"), { changed: mirrorChanged, devices: mirrorTargets, back });
MirrorAdvanced.mount($("mirror-sheet").querySelector('[data-slot="mirror-advanced"]'));

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
  scan();
})();
