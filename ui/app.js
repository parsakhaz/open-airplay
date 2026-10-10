// open-airplay UI: one window, two modes.
//   compact: a frameless, frosted tray popover (the primary surface)
//   full:    the "big-screen media" view with a device sidebar and a large stage (on demand)
const T = window.__TAURI__;
const { invoke } = T.core;
const { listen } = T.event;
const appWindow = T.window ? T.window.getCurrentWindow() : null;
const dpi = T.dpi || T.window || {};
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const use = (id) => `<svg><use href="#${id}"/></svg>`;

const COMPACT = { w: 340, h: 540 };
const FULL = { w: 1100, h: 700 };

// ---------- app preferences (saved by the Rust side in the app's config folder) ----------
const prefs = { openFullOnMirror: false, startAtLogin: false };

async function loadPrefs() {
  try {
    const p = await invoke("get_prefs");
    prefs.openFullOnMirror = !!p.open_full_view_on_mirror;
    prefs.startAtLogin = !!p.start_at_login;
  } catch { /* keep defaults */ }
}

async function savePrefs() {
  const p = await invoke("set_prefs", { openFullViewOnMirror: prefs.openFullOnMirror, startAtLogin: prefs.startAtLogin });
  prefs.openFullOnMirror = !!p.open_full_view_on_mirror;
  prefs.startAtLogin = !!p.start_at_login;
}

// ---------- state ----------
const ui = {
  mode: "compact",          // "compact" | "full"
  status: { running: false },
  view: "receive",          // full view: "receive" | "settings" | "send"
  page: "home",             // compact view: "home" | "settings" | "cast"
  target: null,
  devices: [],
  scanned: false,
  scanning: false,
  scanError: null,
  busy: null,
  mirrorSince: null,
  lastPhase: null,
  cast: { state: "idle", message: "" },
  castUrl: "",
  castFile: "",
};

// ---------- receiver phase ----------
// The shell may send `phase`; otherwise derive it the same way from the raw fields.
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

function btn(name, label, cls = "") {
  const busy = ui.busy === name;
  return `<button class="oai-btn ${cls} ${busy ? "is-busy" : ""}" data-action="${name}" ${ui.busy ? "disabled" : ""}>${esc(label)}</button>`;
}

// ---------- window modes ----------
async function win(fn) {
  if (!appWindow) return;
  try { await fn(appWindow); } catch (err) { console.warn("[open-airplay] window op failed", err); }
}

function setMode(mode) {
  ui.mode = mode;
  document.documentElement.dataset.mode = mode;
  render();
}

async function expand(fullscreen = false) {
  setMode("full");
  await win(async (w) => {
    await w.setSkipTaskbar(false);
    await w.setAlwaysOnTop(false);
    await w.setResizable(true);
    if (dpi.LogicalSize) await w.setSize(new dpi.LogicalSize(FULL.w, FULL.h));
    await w.center();
    await w.show();
    await w.setFocus();
    if (fullscreen) await w.setFullscreen(true);
  });
}

async function toggleFullscreen() {
  if (ui.mode !== "full") return expand(true);
  await win(async (w) => {
    if (await w.isFullscreen()) {
      await w.setFullscreen(false);
      await collapse();       // leaving full screen returns to the compact popover
    } else {
      await w.setFullscreen(true);
    }
  });
}

async function collapse() {
  await win(async (w) => {
    if (await w.isFullscreen()) await w.setFullscreen(false);
    await w.setResizable(false);
    if (dpi.LogicalSize) await w.setSize(new dpi.LogicalSize(COMPACT.w, COMPACT.h));
    // Bottom-right of the work area, just above the tray.
    if (dpi.LogicalPosition) {
      const x = Math.max(0, (screen.availLeft || 0) + screen.availWidth - COMPACT.w - 8);
      const y = Math.max(0, (screen.availTop || 0) + screen.availHeight - COMPACT.h - 8);
      await w.setPosition(new dpi.LogicalPosition(x, y));
    }
    await w.setAlwaysOnTop(true);
    await w.setSkipTaskbar(true);
  });
  setMode("compact");
}

if (appWindow) {
  // The compact popover behaves like a tray flyout: it goes away when you click elsewhere.
  if (appWindow.onFocusChanged) {
    appWindow.onFocusChanged(({ payload: focused }) => {
      if (!focused && ui.mode === "compact") win((w) => w.hide());
    });
  }
  const syncMax = () => win(async (w) => {
    const max = await w.isMaximized();
    $("win-max-icon").setAttribute("href", max ? "#i-restore" : "#i-max");
    $("win-max").setAttribute("aria-label", max ? "Restore" : "Maximize");
  });
  window.addEventListener("resize", () => { if (ui.mode === "full") syncMax(); });
}

$("win-min").addEventListener("click", () => win((w) => w.minimize()));
$("win-max").addEventListener("click", () => win((w) => w.toggleMaximize()));
// Closing the full view tucks the app back into the tray; Quit lives in the tray menu.
$("win-close").addEventListener("click", async () => { await collapse(); win((w) => w.hide()); });
$("win-collapse").addEventListener("click", () => collapse());
$("win-fullscreen").addEventListener("click", () => toggleFullscreen());
$("pop-expand").addEventListener("click", () => expand(false));
$("pop-settings").addEventListener("click", () => { ui.page = ui.page === "settings" ? "home" : "settings"; render(); });

document.addEventListener("keydown", (e) => {
  if (e.key === "F11") { e.preventDefault(); toggleFullscreen(); }
  else if (e.key === "Escape") {
    if (ui.mode === "full") collapse();
    else if (ui.page !== "home") { ui.page = "home"; render(); }
    else win((w) => w.hide());
  }
});

// ---------- full view rendering ----------
let lastKey = "";
function setStage(key, html) {
  const stage = $("stage");
  stage.innerHTML = html;
  // Re-animate only when the screen actually changes.
  if (key === lastKey) stage.querySelectorAll(".oai-view").forEach((el) => el.classList.remove("oai-view"));
  lastKey = key;
}

function setToolbar(title, sub, tools = "") {
  $("tb-title").textContent = title;
  $("tb-sub").textContent = sub;
  $("tb-tools").innerHTML = tools;
}

function renderSidebar() {
  const s = ui.status, phase = phaseOf(s);
  $("receive-dot").className = "oai-dot " + ({ ready: "is-live", mirroring: "is-live", connecting: "is-busy", pin: "is-busy", starting: "is-busy", error: "is-bad" }[phase] || "");
  const session = (phase === "mirroring" || phase === "connecting") && deviceOf(s);
  $("receive-label").textContent = session || "This PC";
  $("receive-icon").setAttribute("href", session ? "#i-phone" : "#i-pc");
  $("foot-name").textContent = nameOf(s);
  document.querySelectorAll("#full .oai-sidebar-item[data-view]").forEach((b) => {
    b.setAttribute("aria-current", String(b.dataset.view === ui.view));
  });
  $("devices").innerHTML = ui.devices.map((d, i) => {
    const current = ui.view === "send" && ui.target && ui.target.identifier === d.identifier;
    const kind = deviceIcon(d) === "i-tv" ? "TV" : deviceIcon(d) === "i-laptop" ? "Mac" : "";
    return `<button class="oai-sidebar-item" data-device="${i}" aria-current="${current}">${use(deviceIcon(d))}<span class="oai-label">${esc(d.name)}</span><span class="oai-meta">${kind}</span></button>`;
  }).join("");
  const empty = $("devices-empty");
  empty.hidden = ui.devices.length > 0 && !ui.scanning;
  empty.innerHTML = devicesEmptyText();
  $("scan").classList.toggle("is-busy", ui.scanning);
  $("scan").disabled = ui.scanning;
}

function devicesEmptyText() {
  if (ui.scanning) return '<span class="oai-spinner is-small" style="display:inline-block;vertical-align:-2px;margin-right:6px"></span>Searching…';
  if (ui.scanError) return `Couldn't search: ${esc(ui.scanError)}`;
  return ui.scanned ? "No AirPlay devices found on this network." : "Press ↻ to find Apple TVs and speakers.";
}

function renderReceive() {
  const s = ui.status, phase = phaseOf(s), name = nameOf(s), device = deviceOf(s) || "An iPhone";
  switch (phase) {
    case "starting":
      setToolbar("This PC", "Starting the receiver…");
      setStage("starting", `<div class="oai-hero oai-view"><div class="oai-spinner" style="margin:0 auto 18px"></div><h2>Starting…</h2><p>Getting ${esc(name)} ready for AirPlay.</p></div>`);
      break;
    case "ready":
      setToolbar("This PC", `Visible as ${name}`);
      setStage("ready", `<div class="oai-hero oai-view">
        <div class="rings"><span></span><span></span><span></span><svg><use href="#i-app"/></svg></div>
        <h2>Ready to receive</h2>
        <p>${esc(name)} is visible to iPhones, iPads and Macs on this network.</p>
        <div class="oai-chips"><span class="oai-chip">Control Center</span>→<span class="oai-chip">Screen Mirroring</span>→<span class="oai-chip"><b>${esc(name)}</b></span></div>
        ${s.settings && s.settings.pin_mode ? `<p class="note">A PIN is required for every connection.</p>` : ""}
      </div>`);
      break;
    case "connecting":
      setToolbar("This PC", "Incoming connection");
      setStage("connecting", `<div class="oai-hero oai-view">
        <div class="pair"><div class="dev">${use("i-phone")}</div><div class="dots"><i></i><i></i><i></i></div><div class="dev">${use("i-pc")}</div></div>
        <h2 style="font-size:var(--oai-text-xl)">${esc(device)} is connecting</h2>
        <p>The video window opens as soon as the picture arrives.</p></div>`);
      break;
    case "mirroring":
      setToolbar(`Mirroring ${device}`, "Video plays in its own open-airplay window", btn("stop", "Stop receiving", "is-danger is-small"));
      setStage("mirroring", `<div class="oai-view" style="width:100%">
        <div class="now">
          <div class="art"><div class="screen ${orientation()}"><svg><use href="#i-app"/></svg></div></div>
          <div class="bar"><div class="who"><b>${esc(device)}</b><small data-elapsed>Mirroring · ${elapsed(ui.mirrorSince)}</small>${videoSize()}</div>${btn("show-video", "Show video window", "is-small")}</div>
        </div>
        <p class="note">The video plays in its own open-airplay window, sized to the phone's screen. It follows rotation. Stop mirroring from the phone, or with Stop.</p></div>`);
      break;
    case "pin": {
      const digits = String(s.pin).split("").map((d) => `<span>${esc(d)}</span>`).join("");
      setToolbar("This PC", "PIN required");
      setStage("pin", `<div class="oai-hero oai-view" style="max-width:520px"><h2 style="font-size:var(--oai-text-xl)">Enter this code on ${esc(deviceOf(s) || "your iPhone")}</h2>
        <p style="margin:0">Your iPhone is asking for an AirPlay code.</p>
        <div class="digits">${digits}</div>
        <p class="note" style="margin:0">PIN is on in Settings, so every new connection needs a code.</p></div>`);
      break;
    }
    case "error":
      setToolbar("This PC", "The receiver hit a problem");
      setStage("error", `<div class="oai-hero oai-view"><div class="glyph is-bad">${use("i-warn")}</div>
        <h2 style="font-size:var(--oai-text-xl)">AirPlay receiver stopped</h2>
        <p>iPhones can't see ${esc(name)} right now.</p>
        <div class="detail">${esc(s.last_error)}</div>
        <div class="oai-btn-row">${btn("settings", "Settings")}${btn("start", "Try again", "is-primary")}</div></div>`);
      break;
    default:
      setToolbar("This PC", "Receiver off");
      setStage("stopped", `<div class="oai-hero oai-view"><div class="glyph">${use("i-pc")}</div>
        <h2 style="font-size:var(--oai-text-xl)">Receiving is off</h2>
        <p>Turn it on to let iPhones mirror to ${esc(name)}.</p>
        <div class="oai-btn-row">${btn("start", "Start receiving", "is-primary")}</div></div>`);
  }
}

function settingsRows(rowClass) {
  const s = currentSettings();
  return `
    <label class="${rowClass}"><span class="oai-grow"><span>Name</span><small class="oai-sub" style="display:block">Shown in Screen Mirroring</small></span><input class="oai-input" data-set="name" required maxlength="63" value="${esc(s.name)}"></label>
    <label class="${rowClass}"><span class="oai-grow"><span>Require a PIN</span><small class="oai-sub" style="display:block">Every new connection enters a code</small></span><input type="checkbox" class="oai-switch" data-set="pin_mode" ${s.pin_mode ? "checked" : ""}></label>
    <label class="${rowClass}"><span class="oai-grow"><span>Prefer H.265</span><small class="oai-sub" style="display:block">Try this if iOS 27 won't connect</small></span><input type="checkbox" class="oai-switch" data-set="h265" ${s.h265 ? "checked" : ""}></label>
    <label class="${rowClass}"><span class="oai-grow"><span>Open full view when mirroring starts</span><small class="oai-sub" style="display:block">Otherwise the app stays in the tray</small></span><input type="checkbox" class="oai-switch" data-pref="openFullOnMirror" ${prefs.openFullOnMirror ? "checked" : ""}></label>
    <label class="${rowClass}"><span class="oai-grow"><span>Start at login</span><small class="oai-sub" style="display:block">Open quietly in the tray when you sign in</small></span><input type="checkbox" class="oai-switch" data-pref="startAtLogin" ${prefs.startAtLogin ? "checked" : ""}></label>`;
}

function renderSettings() {
  const running = isOn(phaseOf(ui.status));
  setToolbar("Settings", "Receiver changes apply when it restarts");
  setStage("settings", `<form class="form oai-view settings-form" autocomplete="off">
    <div class="oai-group-label">Receiving</div>
    <div class="oai-group">${settingsRows("oai-row")}</div>
    <div class="actions">
      ${running ? btn("stop", "Stop receiving") : ""}
      <button type="submit" class="oai-btn is-primary ${ui.busy === "apply" ? "is-busy" : ""}" ${ui.busy ? "disabled" : ""}>${running ? "Apply and restart" : "Apply and start"}</button>
    </div>
    <div class="status-line" data-settings-status></div>
  </form>`);
}

function castControls(compact) {
  const d = ui.target, c = ui.cast, audioOnly = !d.services.includes("AirPlay");
  const msg = c.state === "working" ? `<span class="oai-spinner is-small"></span>${esc(c.message)}`
    : c.state === "error" ? `${esc(c.message)} ${btn("retry-cast", "Retry", "is-small")}`
    : esc(c.message);
  const cls = c.state === "error" ? "is-bad" : c.state === "ok" ? "is-ok" : "";
  if (compact) {
    return `<div class="pop-cast">
      ${audioOnly ? "" : `<div class="row"><input class="oai-input" data-cast="url" type="url" placeholder="Video or audio link" value="${esc(ui.castUrl)}">${btn("play-url", "Play", "is-primary is-small")}</div>`}
      <div class="row"><input class="oai-input" data-cast="file" placeholder="C:\\Music\\song.mp3" value="${esc(ui.castFile)}">${btn("stream-file", "Stream", (audioOnly ? "is-primary " : "") + "is-small")}</div>
      <div class="msg ${cls}">${msg}</div></div>`;
  }
  return `
    ${audioOnly ? "" : `<div class="oai-group-label">Play a video or audio link</div>
    <div class="oai-group"><div class="cast-row"><input class="oai-input" data-cast="url" type="url" placeholder="https://example.com/video.mp4" value="${esc(ui.castUrl)}">${btn("play-url", "Play", "is-primary")}</div></div>`}
    <div class="oai-group-label">Stream an audio file from this PC</div>
    <div class="oai-group"><div class="cast-row"><input class="oai-input" data-cast="file" placeholder="C:\\Users\\me\\Music\\song.mp3" value="${esc(ui.castFile)}">${btn("stream-file", "Stream", audioOnly ? "is-primary" : "")}</div></div>
    <div class="status-line ${cls}">${msg}</div>`;
}

function renderSend() {
  const d = ui.target;
  if (!d) { ui.view = "receive"; return renderReceive(); }
  const audioOnly = !d.services.includes("AirPlay");
  setToolbar(d.name, [d.model, d.address].filter(Boolean).join(" · "));
  setStage("send:" + d.identifier, `<div class="form oai-view">
    <div class="target"><div class="dev">${use(audioOnly ? "i-speaker" : "i-tv")}</div><div><b>Cast to ${esc(d.name)}</b><small>${audioOnly ? "Audio only" : "Video, photos and audio"}</small></div></div>
    ${castControls(false)}</div>`);
}

// ---------- network doctor findings ----------
ui.findings = [];
function findingsHtml() {
  if (!ui.findings.length) return "";
  const card = (f) => {
    switch (f.kind) {
      case "public_network":
        return [`Windows treats “${esc(f.network)}” as a public network, so iPhones may not find this PC.`, btn("fix:public_network", "Open network settings", "is-small")];
      case "firewall_blocks":
        return ["Windows Firewall is blocking the receiver, so iPhones can't connect.", btn("fix:firewall_blocks", "Allow (needs admin)", "is-small is-primary")];
      case "guest_network":
        return [`“${esc(f.network)}” looks like a guest Wi-Fi. Guest networks often stop devices from seeing each other; if your iPhone can't find this PC, put both on your main Wi-Fi.`, ""];
      case "other_receiver":
        return [`${esc(f.name)} is also running and may take the connection. Quit it if your iPhone connects to the wrong one.`, ""];
      default:
        return null;
    }
  };
  const items = ui.findings.map(card).filter(Boolean)
    .map(([text, action]) => `<div class="oai-banner doctor"><div class="oai-grow"><p style="margin:0">${text}</p>${action ? `<div class="oai-btn-row" style="margin-top:8px">${action}</div>` : ""}</div></div>`);
  return `<div class="doctor-list">${items.join("")}<button class="oai-ghost recheck" data-action="doctor-check">Check again</button></div>`;
}

function renderFull() {
  renderSidebar();
  if (ui.view === "settings") renderSettings();
  else if (ui.view === "send") renderSend();
  else {
    renderReceive();
    $("stage").insertAdjacentHTML("afterbegin", findingsHtml());
  }
}

// ---------- compact rendering ----------
function compactStatus() {
  const s = ui.status, phase = phaseOf(s), name = nameOf(s), device = deviceOf(s) || "An iPhone";
  switch (phase) {
    case "starting":
      return `<div class="pop-hero"><div class="oai-spinner"></div><h3>Starting…</h3><p>Getting ${esc(name)} ready</p></div>`;
    case "ready":
      return `<div class="pop-status"><div class="line"><span class="oai-dot is-live"></span><div>Ready to receive, <b>${esc(name)}</b></div></div>
        <div class="hint">On your iPhone, open Control Center → Screen Mirroring.</div></div>`;
    case "connecting":
      return `<div class="pop-hero"><div class="dots" style="justify-content:center;margin:6px 0 10px"><i></i><i></i><i></i></div><h3>${esc(device)} is connecting</h3><p>The video window opens in a moment.</p></div>`;
    case "mirroring":
      return `<div class="pop-hero is-live"><div class="who"><div class="badge"><i></i>MIRRORING</div><h3>${esc(device)}</h3><p data-elapsed>${elapsed(ui.mirrorSince)}</p></div><div class="acts">${btn("show-video", "Show", "is-small")}${btn("stop", "Stop", "is-danger is-small")}</div></div>`;
    case "pin": {
      const digits = String(s.pin).split("").map((d) => `<span>${esc(d)}</span>`).join("");
      return `<div class="pop-hero"><h3>Enter this code on ${esc(deviceOf(s) || "your iPhone")}</h3><div class="pop-pin">${digits}</div><p>PIN mode is on</p></div>`;
    }
    case "error":
      return `<div class="pop-hero is-bad"><h3>Receiver stopped</h3><p>iPhones can't see ${esc(name)} right now.</p><div class="detail">${esc(s.last_error)}</div><div class="oai-btn-row" style="justify-content:flex-start">${btn("start", "Try again", "is-primary is-small")}</div></div>`;
    default:
      return `<div class="pop-hero"><h3>Receiving is off</h3><p>Turn it on so iPhones can mirror here.</p><div class="oai-btn-row">${btn("start", "Start receiving", "is-primary is-small")}</div></div>`;
  }
}

function compactDevices() {
  const rows = ui.devices.map((d, i) => `<button class="oai-list-row" data-device="${i}"><span class="oai-ico">${use(deviceIcon(d))}</span><span class="oai-grow"><span style="display:block">${esc(d.name)}</span></span><span class="oai-meta">${deviceKind(d)}</span></button>`).join("");
  const empty = ui.devices.length && !ui.scanning ? "" : `<div class="oai-list-row oai-sub" style="color:var(--oai-ink-2)">${devicesEmptyText()}</div>`;
  return `<div class="oai-list-label">Send to <button class="oai-ghost scan-btn ${ui.scanning ? "is-busy" : ""}" data-action="scan" style="height:22px;padding:0 6px" aria-label="Find AirPlay devices" ${ui.scanning ? "disabled" : ""}>${use("i-refresh")}</button></div>${rows}${empty}`;
}

const lastPage = { key: "" };
function renderCompact() {
  const s = ui.status, phase = phaseOf(s);
  $("pop-sub").textContent = ({ starting: "Starting…", ready: "Ready to receive", connecting: "Connecting…", mirroring: "Mirroring", pin: "Waiting for PIN", error: "Problem", stopped: "Off" })[phase];
  const sw = $("rx-switch");
  sw.checked = isOn(phase);
  sw.disabled = !!ui.busy;
  let html;
  if (ui.page === "settings") {
    html = `<div class="pop-page"><button class="oai-ghost oai-back" data-action="home">‹ Settings</button>
      <form class="settings-form" autocomplete="off">${settingsRows("oai-list-row")}
      <div class="oai-btn-row" style="justify-content:flex-end;padding:6px 10px 2px"><button type="submit" class="oai-btn is-primary is-small ${ui.busy === "apply" ? "is-busy" : ""}" ${ui.busy ? "disabled" : ""}>${isOn(phase) ? "Apply and restart" : "Apply and start"}</button></div>
      <div class="status-line" data-settings-status style="padding:0 10px;font-size:var(--oai-text-sm)"></div></form></div>`;
  } else if (ui.page === "cast" && ui.target) {
    const d = ui.target;
    html = `<div class="pop-page"><button class="oai-ghost oai-back" data-action="home">‹ Back</button>
      <div class="oai-list-row"><span class="oai-ico">${use(deviceIcon(d))}</span><span class="oai-grow"><span style="display:block;font-weight:600">${esc(d.name)}</span><span class="oai-sub" style="display:block">${esc(d.model || deviceKind(d))}</span></span></div>
      ${castControls(true)}</div>`;
  } else {
    ui.page = "home";
    html = `<div class="pop-page">${compactStatus()}${findingsHtml()}<div class="oai-sep"></div>${compactDevices()}</div>`;
  }
  const body = $("pop-body");
  body.innerHTML = html;
  const key = ui.page + ":" + (ui.page === "home" ? phase : ui.target && ui.target.identifier);
  if (key === lastPage.key) body.querySelectorAll(".pop-page").forEach((el) => el.classList.remove("pop-page"));
  lastPage.key = key;
}

// Shape and size of the mirrored picture, from the engine's "video WxH" report.
function orientation() {
  const v = ui.status.video;
  return v && v[0] > v[1] ? "is-landscape" : "is-portrait";
}
function videoSize() {
  const v = ui.status.video;
  return v ? `<small class="size">${v[0]} × ${v[1]}</small>` : "";
}

function render() {
  const phase = phaseOf(ui.status);
  if (phase === "mirroring" && ui.status.mirroring_since_ms) ui.mirrorSince = ui.status.mirroring_since_ms;
  if (phase === "mirroring" && !ui.mirrorSince) ui.mirrorSince = Date.now();
  if (phase !== "mirroring") ui.mirrorSince = null;
  if (ui.mode === "full") renderFull(); else renderCompact();
}

// Keep the mirroring clock ticking without re-rendering.
setInterval(() => {
  if (!ui.mirrorSince) return;
  document.querySelectorAll("[data-elapsed]").forEach((el) => {
    el.textContent = (ui.mode === "full" ? "Mirroring · " : "") + elapsed(ui.mirrorSince);
  });
}, 1000);

// ---------- receiver commands ----------
async function runCommand(name, fn) {
  ui.busy = name;
  render();
  try {
    const status = await fn();
    if (status) ui.status = status;
    return true;
  } catch (err) {
    ui.status = { ...ui.status, running: false, phase: "error", last_error: String(err) };
    return false;
  } finally {
    ui.busy = null;
    render();
  }
}
const startReceiver = (settings) => runCommand("start", () => invoke("receiver_start", { settings }));
const stopReceiver = () => runCommand("stop", () => invoke("receiver_stop"));

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
  if (!ui.target) return;
  lastCast = cmd;
  ui.cast = { state: "working", message: `Connecting to ${ui.target.name}…` };
  render();
  try {
    await request({ ...cmd, identifier: ui.target.identifier });
    ui.cast = { state: "ok", message: (cmd.cmd === "play_url" ? "Playing on " : "Streaming to ") + ui.target.name };
  } catch (err) {
    ui.cast = { state: "error", message: `Couldn't cast: ${err}` };
  }
  render();
}

function selectDevice(i) {
  ui.target = ui.devices[i];
  ui.cast = { state: "idle", message: "" };
  if (ui.mode === "full") ui.view = "send"; else ui.page = "cast";
  render();
}

// ---------- events ----------
document.addEventListener("click", async (e) => {
  const dev = e.target.closest("[data-device]");
  if (dev) return selectDevice(Number(dev.dataset.device));
  const item = e.target.closest("#full .oai-sidebar-item[data-view]");
  if (item) { ui.view = item.dataset.view; return render(); }
  if (e.target.closest("#scan")) return scan();
  const action = e.target.closest("[data-action]");
  if (action && action.dataset.action.startsWith("fix:")) {
    const kind = action.dataset.action.slice(4);
    ui.busy = action.dataset.action; render();
    try { await invoke("doctor_fix", { kind }); } catch (err) { ui.findingError = String(err); }
    ui.busy = null; render();
    return;
  }
  if (!action || action.disabled) return;
  switch (action.dataset.action) {
    case "start": return startReceiver(currentSettings());
    case "stop": return stopReceiver();
    case "show-video": return invoke("show_video_window").catch(() => {});
    case "scan": return scan();
    case "home": ui.page = "home"; return render();
    case "doctor-check": return invoke("doctor_check").catch(() => {});
    case "settings": if (ui.mode === "full") ui.view = "settings"; else ui.page = "settings"; return render();
    case "play-url": return ui.castUrl.trim() && cast({ cmd: "play_url", url: ui.castUrl.trim() });
    case "stream-file": return ui.castFile.trim() && cast({ cmd: "stream_file", path: ui.castFile.trim() });
    case "retry-cast": return lastCast && cast(lastCast);
  }
});

$("rx-switch").addEventListener("change", (e) => {
  if (e.target.checked) startReceiver(currentSettings()); else stopReceiver();
});

document.addEventListener("input", (e) => {
  if (e.target.dataset.cast === "url") ui.castUrl = e.target.value;
  if (e.target.dataset.cast === "file") ui.castFile = e.target.value;
});

document.addEventListener("change", async (e) => {
  const key = e.target.dataset.pref;
  if (!key) return;
  const before = prefs[key];
  prefs[key] = e.target.checked;
  try {
    await savePrefs();
  } catch (err) {
    prefs[key] = before;
    e.target.checked = before;
    const line = document.querySelector("[data-settings-status]");
    if (line) line.textContent = String(err);
  }
});

document.addEventListener("submit", async (e) => {
  const form = e.target.closest(".settings-form");
  if (!form) return;
  e.preventDefault();
  const val = (k) => form.querySelector(`[data-set="${k}"]`);
  const settings = { name: val("name").value.trim(), pin_mode: val("pin_mode").checked, h265: val("h265").checked };
  if (!settings.name) return;
  ui.status = { ...ui.status, settings };
  const ok = await runCommand("apply", () => invoke("receiver_start", { settings }));
  const line = document.querySelector("[data-settings-status]");
  if (line) {
    line.className = "status-line " + (ok ? "is-ok" : "is-bad");
    line.textContent = ok ? "Saved. The receiver restarted." : `Couldn't restart: ${ui.status.last_error}`;
  }
});

listen("receiver-status", (e) => {
  ui.status = e.payload;
  const phase = phaseOf(ui.status);
  if (phase === "mirroring" && ui.lastPhase !== "mirroring" && prefs.openFullOnMirror && ui.mode === "compact") {
    ui.lastPhase = phase;
    expand(false);
    return;
  }
  ui.lastPhase = phase;
  // Don't clobber a settings form the user is typing in.
  if (document.activeElement && document.activeElement.closest(".settings-form")) {
    if (ui.mode === "full") renderSidebar();
    return;
  }
  render();
});

listen("sender-event", (e) => {
  const msg = e.payload;
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  msg.event === "error" ? p.reject(msg.message) : p.resolve(msg);
});

// The sender sidecar died: fail everything in flight; the next request starts a fresh one.
listen("doctor-findings", (e) => { ui.findings = e.payload || []; render(); });

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
    ui.status = status;
  } catch (err) {
    ui.status = { running: false, last_error: String(err) };
  }
  ui.lastPhase = phaseOf(ui.status);
  render();
})();
