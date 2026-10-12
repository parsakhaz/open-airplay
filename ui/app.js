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
  art: { id: null, url: null }, // the cover art shown, by the status's art number
  speakers: [],             // the PC's sound outputs: [{ id, name }]
};

// The header says the status once; the body never repeats it.
const HEADER = { stopped: "Off", starting: "Starting…", ready: "Ready", connecting: "Connecting…", audio: "Playing audio", pin: "Enter the code", restarting: "Restarting…", error: "Needs attention" };
const LIVE_HEADER = { mirroring: "Mirroring from", audio: "Playing audio from", video: "Playing video from" };

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
const inSession = (phase) => phase === "connecting" || phase === "mirroring" || phase === "audio" || phase === "video" || phase === "pin";
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
  // No services at all: added by address and never answered a cast scan, so it's offered for mirroring.
  const audioOnly = d.services.length > 0 && !d.services.includes("AirPlay");
  if (audioOnly || /speaker|homepod|five/i.test(d.model || "")) return "i-speaker";
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
  return `<div class="oai-banner is-info"><p class="oai-grow">${esc(n)}</p><button class="oai-icon-btn is-small" data-action="dismiss-notice" aria-label="Dismiss" title="Dismiss">${use("i-x")}</button></div>${errorLine("dismiss-notice")}`;
}

/** The "Playing audio" card: what's playing, the phone's volume, its controls and Disconnect. */
function audioHtml(s) {
  const a = s.audio || {};
  const art = ui.art.url && ui.art.id === a.art ? `<img src="${esc(ui.art.url)}" alt="">` : use("i-music");
  const title = a.title || (a.playing ? "Playing" : "Nothing playing");
  const detail = [a.artist, a.album].filter(Boolean).join(" — ");
  const ctl = (cmd, icon, label) => `<button class="oai-btn is-icon" data-action="control:${cmd}" aria-label="${label}" title="${label}">${use(icon)}</button>`;
  const controls = s.can_control ? `<div class="controls">${ctl("previous", "i-prev", "Previous")}${ctl("play_pause", "i-playpause", "Play or pause")}${ctl("next", "i-next", "Next")}</div>` : "";
  const volume = s.volume == null ? "" : `<div class="volume" role="meter" aria-label="Volume on the phone" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${s.volume}" title="Volume: ${s.volume}% (change it on the phone)">${use("i-volume")}<span class="bar"><i style="width:${s.volume}%"></i></span></div>`;
  const controlError = ["previous", "play_pause", "next"].map((c) => errorLine("control:" + c)).join("");
  return `<div class="pop-hero is-live is-audio"><div class="top"><div class="art">${art}</div><div class="who"><h3>${esc(title)}</h3>${detail ? `<p>${esc(detail)}</p>` : ""}</div></div><div class="bottom">${controls}${volume}${btn("disconnect", "Disconnect", "is-danger is-small")}</div></div>${controlError}${errorLine("disconnect")}`;
}

// The receive block: a dot (or a spinner while something is happening) next to one sentence and a hint.
const rxBlock = (lead, text, hint) => `<div class="oai-rx">${lead}<div>${text}<small>${hint}</small></div></div>`;
const DOT_ON = '<span class="oai-dot is-live"></span>', DOT_OFF = '<span class="oai-dot"></span>', SPINNER = '<span class="oai-spinner is-small"></span>';

function statusHtml() {
  const s = ui.status, phase = phaseOf(s), name = `<b>${esc(nameOf(s))}</b>`, device = esc(deviceOf(s) || "An iPhone");
  const phone = `<span class="oai-art">${use("i-phone")}</span>`;
  // Only a device that said who it is can be blocked.
  const block = s.device_id ? `<button class="oai-ghost block-link" data-action="block">Block this device</button>${errorLine("block")}` : "";
  switch (phase) {
    case "starting":
      return rxBlock(SPINNER, `Getting ready to receive as ${name}`, "This takes a few seconds.");
    case "ready":
      return rxBlock(DOT_ON, `Ready to receive as ${name}`, "On your iPhone, open Control Center → Screen Mirroring, or pick it as the speaker to play music.");
    case "connecting":
      return `<div class="oai-live"><div class="oai-media">${phone}<div><b>${device}</b><small>The video window opens in a moment.</small></div></div>${btn("disconnect", "Disconnect", "is-danger is-wide")}${errorLine("disconnect")}</div>${block}`;
    case "mirroring":
      // The clock's text is filled in by tick(), outside the compared markup.
      return `<div class="oai-live"><div class="oai-media">${phone}<div><b>${device}</b><small class="is-live"><span class="oai-pulse"></span><span data-elapsed></span></small></div></div><div class="oai-btns">${btn("show-video", "Show video")}${btn("disconnect", "Disconnect", "is-danger")}</div>${errorLine("show-video")}${errorLine("disconnect")}</div>${block}`;
    case "audio":
      return audioHtml(s);
    case "video": {
      // A YouTube video the phone sent: the same card as mirroring, once the first frame is in.
      const playing = !!s.video;
      const sub = playing ? '<small class="is-live"><span class="oai-pulse"></span><span data-elapsed></span></small>' : "<small>Loading the video…</small>";
      const show = playing ? btn("show-video", "Show video") : "";
      return `<div class="oai-live"><div class="oai-media">${phone}<div><b>${device}</b>${sub}</div></div><div class="oai-btns">${show}${btn("disconnect", "Disconnect", "is-danger")}</div>${errorLine("show-video")}${errorLine("disconnect")}</div>${block}`;
    }
    case "pin": {
      const digits = String(s.pin).split("").map((d) => `<span>${esc(d)}</span>`).join("");
      return `<div class="oai-code"><h4>Enter this code on ${device}</h4><div class="oai-digits">${digits}</div><button class="oai-ghost" data-action="disconnect">Disconnect</button>${errorLine("disconnect")}</div>${block}`;
    }
    case "restarting": {
      const why = s.restart_reason ? `${esc(capitalise(s.restart_reason))}, so receiving is restarting.` : "Receiving is restarting.";
      return rxBlock(SPINNER, why, `${name} will be back in a moment.`);
    }
    case "error":
      return `<div class="oai-banner is-bad">${use("i-warn")}<div class="oai-grow">${esc(s.last_error || `iPhones can't see ${nameOf(s)} right now.`)}<div class="oai-mini">Turn receiving back on to retry.</div></div></div>`;
    default:
      return rxBlock(DOT_OFF, `iPhones can't see ${name}`, "Turn on receiving to mirror an iPhone or play music here.");
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
    `<div class="oai-banner doctor">${use("i-warn")}<div class="oai-grow"><p>${text}</p>${button ? `<div class="oai-btn-row">${button}</div>` : ""}${action ? errorLine(action) : ""}</div></div>`);
  if (!items.length && !ui.errors["doctor-check"]) return "";
  return `<div class="doctor-list">${items.join("")}<button class="oai-ghost recheck" data-action="doctor-check">Check again</button>${errorLine("doctor-check")}</div>`;
}

function devicesEmptyText() {
  if (ui.scanning) return '<span class="oai-spinner is-small"></span>Searching…';
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
  const empty = list.length && !ui.scanning ? "" : `<div class="devices-empty">${devicesEmptyText()}</div>`;
  // ux-add-device adds its "Add device by address…" row after the rows.
  return `<div class="oai-list-label">Send to <button class="oai-icon-btn ${ui.scanning ? "is-spinning" : ""}" data-action="scan" aria-label="Find devices" title="Find devices">${use("i-refresh")}</button></div>${rows}${empty}`;
}

/** The neutral header of a device page: back, the device, its type or what's being sent to it. */
function sendHeadHtml() {
  const d = ui.target, live = OapCast.rowStatus(d.identifier), mirror = OapMirror.rowStatus(d.address);
  const sub = mirror ? mirror.short : live ? (live.kind === "busy" ? `<span class="oai-spinner is-small"></span>${esc(live.text)}` : `<span class="oai-dot is-live"></span>${esc(live.short || live.text)}`) : esc(friendlyType(d));
  return `<button class="send-back" data-action="home" aria-label="Back">${use("i-back")}</button><span class="dico">${use(deviceIcon(d))}</span><div class="tt"><div class="t">${esc(d.name)}</div><div class="s">${sub}</div></div>`;
}

/** What the sub-page header shows: an icon, a title and a subtitle. Home has its own header; a device page has send-head. */
function subHead() {
  if (ui.page === "settings") return { icon: "i-gear", title: "Settings", sub: "AirPlay Receiver" };
  return null;
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
  loadArt(s);
  if ($("pop-sub").textContent !== sub) $("pop-sub").textContent = sub;
  const sw = $("rx-switch");
  if (sw.checked !== isOn(phase)) sw.checked = isOn(phase);
  const tip = isOn(phase) ? "Turn off receiving" : "Turn on receiving";
  if ($("rx-switch-hit").title !== tip) $("rx-switch-hit").title = tip;
  const switchError = ui.errors.start || ui.errors.stop || "";
  const swErr = $("r-switch-error");
  swErr.hidden = !switchError;
  if (swErr.textContent !== switchError) swErr.textContent = switchError;

  if (ui.page === "cast" && !ui.target) ui.page = "home";
  for (const page of ["home", "settings", "cast", "mirror"]) $("page-" + page).hidden = ui.page !== page;
  // Sub-pages swap the home header (and its switch) for back, an icon and a title, and drop the footer.
  // Send pages (a device page, the Mirror sheet) have their own header, send-head.
  const sendPage = ui.page === "cast" || ui.page === "mirror", head = subHead();
  $("panel-head").hidden = sendPage || !!head;
  $("panel-subhead").hidden = !head;
  $("send-head").hidden = !sendPage;
  $("panel-foot").hidden = sendPage || !!head;
  if (head) {
    patch("sub-ico", use(head.icon));
    if ($("sub-title").textContent !== head.title) $("sub-title").textContent = head.title;
    if ($("sub-sub").textContent !== head.sub) $("sub-sub").textContent = head.sub;
  }

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

/** Fetches the cover art when the status names a new picture; the card shows it once it's here. */
async function loadArt(s) {
  const id = (s.audio && s.audio.art) || null;
  if (id === ui.art.id) return;
  ui.art = { id, url: null };
  if (!id) return;
  let url = null;
  try { url = await invoke("receiver_art"); } catch { /* no picture */ }
  if (ui.art.id !== id) return;   // a newer picture arrived meanwhile
  ui.art = { id, url };
  render();
}

// Keep the mirroring clock ticking without re-rendering.
setInterval(tick, 1000);

// ---------- the panel's size follows its content ----------
let reportedHeight = 0;
function reportHeight() {
  const body = $("pop-body"), cs = getComputedStyle(body);
  const pad = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
  const height = (id) => $(id).getBoundingClientRect().height;   // fractional: the footer has a half-pixel border
  const h = Math.ceil(height("panel-head") + height("panel-subhead") + height("send-head") + height("pop-content") + pad + height("panel-foot"));
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

/** A mirror status change: refresh the open device's tile (it carries the live state), then render.
 *  Also while the sheet is open, so going back shows the tile as it is now. */
function mirrorChanged() {
  if (ui.target) showDevicePage();
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
  if (name.startsWith("control:")) return run(name, () => invoke("receiver_control", { command: name.slice(8) }));
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
    case "back": return back();
    case "mirror": return openMirror();
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

listen("doctor-findings", (e) => { ui.findings = e.payload || []; render(); });

listen("panel-shown", scanIfStale);
listen("network-changed", () => { ui.lastScan = 0; scan(); });

OapCast.mount($("cast-media"), render);
OapMirror.mount($("mirror-sheet"), { changed: mirrorChanged, devices: mirrorTargets, back });

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
