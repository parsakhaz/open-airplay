const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// Tabs
document.querySelectorAll("nav button").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll("nav button").forEach((b) => b.setAttribute("aria-selected", String(b === btn)));
    document.querySelectorAll(".tab").forEach((t) => (t.hidden = t.id !== btn.dataset.tab));
  });
});

// ---- Receive ----
function renderStatus(s) {
  const name = s.settings ? s.settings.name : "this PC";
  const dot = $("status-dot");
  let headline, detail, cls;
  if (s.connections > 0) {
    headline = `Receiving${s.last_device ? " from " + s.last_device : ""}`;
    detail = `Visible as “${name}”`;
    cls = "busy";
  } else if (s.running && s.advertised) {
    headline = `Visible as “${name}”`;
    detail = "Waiting for an iPhone, iPad or Mac";
    cls = "ok";
  } else if (s.running) {
    headline = "Starting receiver…";
    detail = "";
    cls = "";
  } else {
    headline = "Receiver stopped";
    detail = s.engine_path ? "" : "The UxPlay engine is not built yet";
    cls = s.last_error ? "bad" : "";
  }
  $("status-headline").textContent = headline;
  $("status-detail").textContent = detail;
  dot.className = "dot " + cls;

  $("pin-card").hidden = !s.pin;
  $("pin-code").textContent = s.pin || "";

  $("problem-card").hidden = !s.last_error;
  $("problem-card").textContent = s.last_error || "";
}

function fillSettings(settings) {
  $("name").value = settings.name;
  $("pin_mode").checked = settings.pin_mode;
  $("h265").checked = settings.h265;
}

$("settings").addEventListener("submit", async (e) => {
  e.preventDefault();
  const settings = { name: $("name").value.trim(), pin_mode: $("pin_mode").checked, h265: $("h265").checked };
  try {
    renderStatus(await invoke("receiver_start", { settings }));
  } catch (err) {
    renderStatus({ running: false, last_error: String(err), settings });
  }
});
$("stop").addEventListener("click", async () => renderStatus(await invoke("receiver_stop")));

listen("receiver-status", (e) => renderStatus(e.payload));

(async () => {
  const status = await invoke("receiver_status");
  fillSettings(status.settings || (await invoke("default_settings")));
  renderStatus(status);
})();

// ---- Send ----
let nextId = 1;
const pending = new Map();
let target = null;

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

listen("sender-event", (e) => {
  const msg = e.payload;
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  msg.event === "error" ? p.reject(msg.message) : p.resolve(msg);
});

$("scan").addEventListener("click", async () => {
  $("scan").disabled = true;
  $("scan-state").textContent = "Looking for 5 seconds…";
  const list = $("devices");
  list.replaceChildren();
  try {
    const { devices } = await request({ cmd: "scan", timeout: 5 });
    $("scan-state").textContent = devices.length ? `${devices.length} found` : "No AirPlay devices found on this network";
    for (const d of devices.filter((d) => d.services.includes("AirPlay") || d.services.includes("RAOP"))) {
      const li = document.createElement("li");
      const label = document.createElement("div");
      label.innerHTML = `<div class="headline"></div><div class="muted small"></div>`;
      label.children[0].textContent = d.name;
      label.children[1].textContent = [d.model, d.address].filter(Boolean).join(" · ");
      const pick = document.createElement("button");
      pick.textContent = "Cast here";
      pick.addEventListener("click", () => {
        target = d;
        $("cast").hidden = false;
        $("cast-target").textContent = `Cast to ${d.name}`;
        $("cast-state").textContent = "";
      });
      li.append(label, pick);
      list.append(li);
    }
  } catch (err) {
    $("scan-state").textContent = String(err);
  } finally {
    $("scan").disabled = false;
  }
});

async function cast(cmd) {
  if (!target) return;
  $("cast-state").textContent = `Connecting to ${target.name}…`;
  try {
    await request({ ...cmd, identifier: target.identifier });
    $("cast-state").textContent = "Done.";
  } catch (err) {
    $("cast-state").textContent = `Failed: ${err}`;
  }
}

$("cast").addEventListener("submit", (e) => {
  e.preventDefault();
  cast({ cmd: "play_url", url: $("cast-url").value.trim() });
});
$("cast-file-btn").addEventListener("click", () => cast({ cmd: "stream_file", path: $("cast-file").value.trim() }));
