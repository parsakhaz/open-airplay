// Advanced, folded in the Mirror sheet: the defaults for every mirror (approved device page, state 5).
// Self-contained so the sheet only has to call MirrorAdvanced.mount(container). Every row starts on
// Automatic; a change is saved at once with set_mirror_options and used by the next mirror_start.
// Sound appears only when the sender says it can capture this PC's sound (hello.capabilities.audio).
(function () {
  const { invoke } = window.__TAURI__.core;

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
    return `<div class="grow-row"><label for="adv-${row.field}">${esc(row.label)}${small}</label><span class="sel"><select id="adv-${row.field}" data-field="${row.field}">${options}</select>${DOWN}</span></div>`;
  }

  function html(o, caps, open) {
    const head = `<button type="button" class="disclose${open ? " open" : ""}" aria-expanded="${open}">${CHEV}<span>Advanced</span><span class="sp"></span>${!open && allAutomatic(o) ? "<em>Automatic</em>" : ""}</button>`;
    if (!open) return head;
    return `${head}<div class="group">
      ${ROWS.map((r) => selectRow(r, o[r.field])).join("")}
      <div class="grow-row"><label for="adv-video_codec">Video format<small>The only one on Windows today</small></label><span class="sel dim"><select id="adv-video_codec" disabled><option>H.264</option></select>${DOWN}</span></div>
      <div class="grow-row"><label for="adv-show_cursor">Show the pointer</label><input type="checkbox" class="oai-switch sm" id="adv-show_cursor" data-switch="show_cursor"${o.show_cursor !== false ? " checked" : ""}></div>
      ${caps.audio ? `<div class="grow-row"><label for="adv-audio">Sound</label><input type="checkbox" class="oai-switch sm" id="adv-audio" data-switch="audio"${o.audio !== false ? " checked" : ""}></div>` : ""}
    </div>`;
  }

  async function mount(container) {
    const state = { options: {}, caps: { audio: false }, open: false };
    try {
      state.options = (await invoke("get_prefs")).mirror || {};
    } catch (_) {}
    const render = () => {
      container.innerHTML = html(state.options, state.caps, state.open);
    };
    render();
    // The sender may still be starting; Sound shows once it has said hello.
    invoke("mirror_capabilities")
      .then((caps) => {
        state.caps = caps || { audio: false };
        render();
      })
      .catch(() => {});

    const save = async () => {
      try {
        state.options = (await invoke("set_mirror_options", { options: state.options })).mirror || state.options;
      } catch (_) {}
      render();
    };
    container.addEventListener("click", (e) => {
      if (e.target.closest(".disclose")) {
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
  }

  window.MirrorAdvanced = { mount };
})();
