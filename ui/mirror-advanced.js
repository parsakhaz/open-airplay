// Advanced, folded in the Mirror sheet: the defaults for every mirror (approved device page, state 5).
// Self-contained: the app calls MirrorAdvanced.mount(container) and the sheet calls
// loadCapabilities() when it opens. Every row starts on Automatic; a change is saved at once with
// set_mirror_options and used by the next mirror_start.
// Sound appears only when the sender says it can capture this PC's sound (hello.capabilities.audio).
(function () {
  /** Saves the options one at a time. A change made while a save is on its way is sent once that
   *  one returns, together with any others (only the newest options go). Replies never replace the
   *  choices on screen, so an older reply can't undo a newer choice. `done(err)` runs after the
   *  last save, with its error or null. */
  function createSaver(send, done) {
    let running = false;
    let next = null;
    return function save(options) {
      next = { ...options };
      if (running) return;
      running = true;
      (async () => {
        let error = null;
        while (next) {
          const options = next;
          next = null;
          try {
            await send(options);
            error = null;
          } catch (err) {
            error = err;
          }
        }
        running = false;
        done(error);
      })();
    };
  }

  const DOWN = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3.5 6 8 10.5 12.5 6"/></svg>';
  const CHEV = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5 10.5 8 6 12.5"/></svg>';

  // Each select row: its field in MirrorOptions and its choices. "" is Automatic (left out of connect).
  const ROWS = [
    { field: "fps", label: "Frame rate", choices: [["", "Automatic"], [30, "30 fps"], [60, "60 fps"]] },
    { field: "bitrate_kbps", label: "Quality", small: "Video bitrate", choices: [["", "Automatic"], [3000, "Lower · 3 Mbps"], [8000, "Higher · 8 Mbps"], [12000, "Highest · 12 Mbps"]] },
    { field: "latency_ms", label: "Delay", small: "Lower is snappier, higher is smoother", choices: [["", "Automatic"], [100, "Low · 0.1 s"], [250, "Medium · 0.25 s"], [500, "High · 0.5 s"]] },
    { field: "hwaccel", label: "Encoder", choices: [["", "Automatic"], ["nvenc", "Graphics card"], ["none", "Processor"]] },
  ];

  const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

  // True when nothing differs from Automatic, so the folded row can say so.
  function allAutomatic(o) {
    return ROWS.every((r) => o[r.field] == null) && o.show_cursor == null && o.audio == null;
  }

  function selectRow(row, value) {
    const options = row.choices
      .map(([v, text]) => `<option value="${esc(v)}"${String(v) === String(value ?? "") ? " selected" : ""}>${esc(text)}</option>`)
      .join("");
    const small = row.small ? `<small>${esc(row.small)}</small>` : "";
    return `<div class="oai-group-row"><label for="adv-${row.field}">${esc(row.label)}${small}</label><span class="oai-sel"><select id="adv-${row.field}" data-field="${row.field}">${options}</select>${DOWN}</span></div>`;
  }

  function html(o, caps, open, error) {
    const head = `<button type="button" class="oai-disclose${open ? " is-open" : ""}" aria-expanded="${open}">${CHEV}<span>Advanced</span><span class="oai-spacer"></span>${!open && allAutomatic(o) ? "<em>Automatic</em>" : ""}</button>`;
    if (!open) return head;
    return `${head}<div class="oai-group is-flush">
      ${ROWS.map((r) => selectRow(r, o[r.field])).join("")}
      <div class="oai-group-row"><label for="adv-video_codec">Video format<small>The only one on Windows today</small></label><span class="oai-sel is-dim"><select id="adv-video_codec" disabled><option>H.264</option></select>${DOWN}</span></div>
      <div class="oai-group-row"><label for="adv-show_cursor">Show the pointer</label><input type="checkbox" class="oai-switch is-small" id="adv-show_cursor" data-switch="show_cursor"${o.show_cursor !== false ? " checked" : ""}></div>
      ${caps.audio ? `<div class="oai-group-row"><label for="adv-audio">Sound</label><input type="checkbox" class="oai-switch is-small" id="adv-audio" data-switch="audio"${o.audio !== false ? " checked" : ""}></div>` : ""}
    </div>${error ? `<div class="inline-error" role="alert">${esc(error)}</div>` : ""}`;
  }

  /** Mounts Advanced in `container`. The sender's capabilities are asked for only by
   *  loadCapabilities() (when the Mirror sheet opens): asking starts the sender, and a boot launch
   *  must stay idle. A failed ask is tried again on the next call. */
  function create(container, invoke) {
    const state = { options: {}, caps: { audio: false }, open: false, error: null };
    let asked = false;
    const render = () => {
      container.innerHTML = html(state.options, state.caps, state.open, state.error);
    };
    render();
    invoke("get_prefs")
      .then((prefs) => {
        state.options = (prefs && prefs.mirror) || {};
        render();
      })
      .catch(() => {});

    function loadCapabilities() {
      if (asked) return;
      asked = true;
      // The sender may still be starting; Sound shows once it has said hello.
      invoke("mirror_capabilities")
        .then((caps) => {
          state.caps = caps || { audio: false };
          render();
        })
        .catch(() => {
          asked = false;
        });
    }

    // True when the next mirror sends this PC's sound: the sender can capture it and Sound is on.
    const sendsSound = () => !!state.caps.audio && state.options.audio !== false;

    const saver = createSaver(
      (options) => invoke("set_mirror_options", { options }),
      (err) => {
        state.error = err ? "Couldn't save this choice. Try again." : null;
        render();
      },
    );
    const save = () => saver(state.options);
    container.addEventListener("click", (e) => {
      if (e.target.closest(".oai-disclose")) {
        state.open = !state.open;
        render();
      }
    });
    container.addEventListener("change", (e) => {
      const t = e.target;
      if (t.dataset.field) {
        const raw = t.value;
        state.options[t.dataset.field] = raw === "" ? null : isNaN(Number(raw)) ? raw : Number(raw);
      } else if (t.dataset.switch) {
        // On is the default for both switches, so on is stored as Automatic.
        state.options[t.dataset.switch] = t.checked ? null : false;
      } else {
        return;
      }
      save();
    });
    return { loadCapabilities, sendsSound };
  }

  if (typeof module !== "undefined" && module.exports) module.exports = { createSaver, create };
  if (typeof window === "undefined" || !window.__TAURI__) return;

  let current = null; // the mounted Advanced
  const mount = (container) => {
    current = create(container, window.__TAURI__.core.invoke);
  };
  const loadCapabilities = () => current && current.loadCapabilities();
  const sendsSound = () => !!current && current.sendsSound();

  window.MirrorAdvanced = { mount, loadCapabilities, sendsSound };
})();
