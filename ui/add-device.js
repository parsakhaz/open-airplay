// Devices added by IP address or hostname: joining them into Send to, and the Add device flow.
// The Rust side checks addresses and remembers the list (src-tauri/src/added.rs); this file only
// decides what the list shows and in what order the add steps run. Tests: tests/ui/add-device.test.js.
const AddDevice = (() => {
  // Send to rows: discovered devices first, then added ones discovery can't see.
  // A row with `added` set came from (or matches) an added device; its page offers Remove.
  function merge(discovered, added) {
    const rest = [...added];
    const take = (d) => {
      const i = rest.findIndex((a) => (a.cast_id && a.cast_id === d.identifier) || a.host === d.address);
      return i < 0 ? null : rest.splice(i, 1)[0];
    };
    // A port given when adding wins over discovery's (the cast scan reports none).
    const found = discovered.map((d) => {
      const a = take(d);
      return { ...d, port: (a && a.port) ?? d.port, added: a ? a.host : null };
    });
    const unseen = rest.map((a) => ({
      identifier: a.cast_id || `added:${a.host}`,
      name: a.name || a.host,
      address: a.host,
      port: a.port,
      model: null,
      // Answered a cast scan when added: it can take casts. Otherwise it is offered for mirroring only.
      services: a.cast_id ? ["AirPlay"] : [],
      added: a.host,
      unseen: true,
    }));
    return [...found, ...unseen];
  }

  // Checks the address (Rust: format, then that something answers), asks the cast sidecar who is
  // there, and saves. Resolves to the saved list; rejects with the plain sentence to show.
  async function add(form, { invoke, scanHosts }) {
    const addr = await invoke("added_device_check", { input: form.input, port: String(form.port ?? "").trim() || null });
    let found = null;
    try {
      // The device at that address; a hostname resolves elsewhere, so then only a lone answer counts.
      const devices = await scanHosts(addr.host);
      found = devices.find((d) => d.address === addr.host) || (devices.length === 1 ? devices[0] : null);
    } catch { /* not a cast device, or no sidecar: it can still be mirrored to */ }
    const name = String(form.name ?? "").trim() || (found && found.name) || null;
    return invoke("added_device_save", { device: { host: addr.host, port: addr.port, name, cast_id: (found && found.identifier) || null } });
  }

  // ---------- markup (APPROVED.html: state 1's last Send to row, state 16's form, the approved Remove) ----------
  const ICON = {
    plus: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round"><path d="M8 3.2v9.6M3.2 8h9.6" stroke-width="1.6"/></svg>',
    back: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><path d="M10 3.5 5.5 8l4.5 4.5" stroke-width="1.7"/></svg>',
    net: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="6.3"/><path d="M1.7 8h12.6M8 1.7c2 2 2 10.6 0 12.6M8 1.7c-2 2-2 10.6 0 12.6"/></svg>',
    chev: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3.5 10.5 8 6 12.5" stroke-width="1.6"/></svg>',
  };
  const esc = (v) => String(v ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const SUBTITLE = "Added by address";

  // The last row of Send to.
  function addRowHtml() {
    return `<button class="ad-row" data-add-device="open"><span class="ad-ico">${ICON.plus}</span><span class="ad-grow"><b>Add device by address…</b></span></button>`;
  }

  // The form page's content; the host puts it in an empty page container and shows it with its own navigation.
  function pageHtml() {
    return `<form class="ad-page" data-add-device="form" autocomplete="off" novalidate>
      <header class="oai-popover-head is-sub"><button type="button" class="oai-icon-btn" data-add-device="cancel" aria-label="Back">${ICON.back}</button><span class="oai-dico">${ICON.net}</span><div class="oai-tt"><div class="oai-t">Add a device</div><div class="oai-s">For when it doesn’t show up by itself</div></div></header>
      <div class="ad-body">
        <label class="oai-flabel" for="ad-input">IP address or name</label><input class="oai-input" id="ad-input" name="input" placeholder="192.168.1.40" spellcheck="false" autocapitalize="off">
        <label class="oai-flabel" for="ad-name">Name in the list (optional)</label><input class="oai-input" id="ad-name" name="name" maxlength="63" placeholder="Office TV">
        <button type="button" class="oai-disclose" data-add-device="advanced" aria-expanded="false">${ICON.chev}<span>Advanced</span><span class="oai-spacer"></span><em>Port 7000</em></button>
        <div class="ad-advanced" hidden><label class="oai-flabel" for="ad-port">Port</label><input class="oai-input" id="ad-port" name="port" inputmode="numeric" placeholder="7000"></div>
        <div class="ad-error" role="alert" hidden></div>
        <div class="oai-btns"><button type="button" class="oai-btn" data-add-device="cancel">Cancel</button><button type="submit" class="oai-btn is-primary">Add</button></div>
        <div class="oai-hint">Added devices are remembered and show in Send to with “${SUBTITLE}”. Remove one from its page.</div>
      </div>
    </form>`;
  }

  // For the device page, below the tiles or the drop zone: only for a device that was added by address.
  function removeHtml(row) {
    return row && row.added ? `<button class="oai-linkbtn ad-link" data-add-device="remove" data-host="${esc(row.added)}">Remove from this list</button>` : "";
  }

  // Wires the form, the Add row and Remove. Host seams:
  //   invoke(cmd, args)    Tauri invoke
  //   scanHosts(host)      the cast sidecar's scan with hosts: resolves to its device list
  //   open()               shows the form page (the host's goTo)
  //   done()               after a save, a remove or Cancel: back to Send to (the host's goTo)
  //   refresh()            reloads prefs.added_devices and re-renders the list
  /** Removes an added device and reloads the list. Resolves to null when both worked, or to the
   *  plain sentence to show on the device page (which stays open so the user can try again). */
  async function remove(addedHost, { invoke, refresh }) {
    try {
      await invoke("added_device_remove", { host: addedHost });
    } catch (err) {
      return `Couldn't remove this device. ${String(err && err.message ? err.message : err)}`;
    }
    try {
      await refresh();
    } catch (_) {
      return "Removed, but the list didn't update. Go back to see it.";
    }
    return null;
  }

  function mount(root, host) {
    const form = root.querySelector('[data-add-device="form"]');
    const field = (name) => form.elements[name];
    const error = form.querySelector(".ad-error");
    const submit = form.querySelector('button[type="submit"]');
    const showError = (text) => {
      error.textContent = text || "";
      error.hidden = !text;
    };
    const reset = () => {
      form.reset();
      showError("");
      setAdvanced(false);
    };
    const setAdvanced = (open) => {
      const toggle = form.querySelector('[data-add-device="advanced"]');
      toggle.setAttribute("aria-expanded", String(open));
      toggle.classList.toggle("is-open", open);
      toggle.querySelector("em").hidden = open;
      form.querySelector(".ad-advanced").hidden = !open;
    };

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (submit.disabled) return;
      submit.disabled = true;
      submit.classList.add("is-busy");
      showError("");
      try {
        await add({ input: field("input").value, name: field("name").value, port: field("port").value }, host);
        reset();
        await host.refresh();
        host.done();
      } catch (err) {
        showError(String(err && err.message ? err.message : err));
        field("input").focus();
      } finally {
        submit.disabled = false;
        submit.classList.remove("is-busy");
      }
    });

    document.addEventListener("click", async (e) => {
      const el = e.target.closest("[data-add-device]");
      if (!el) return;
      switch (el.dataset.addDevice) {
        case "open":
          reset();
          host.open();
          setTimeout(() => field("input").focus(), 0);
          return;
        case "cancel":
          reset();
          host.done();
          return;
        case "advanced":
          setAdvanced(el.getAttribute("aria-expanded") !== "true");
          return;
        case "remove": {
          el.disabled = true;
          const problem = await remove(el.dataset.host, host);
          el.disabled = false;
          // The error sits under the link; it goes when the page is drawn again.
          let line = el.nextElementSibling;
          if (!line || !line.classList.contains("ad-remove-error")) {
            line = document.createElement("div");
            line.className = "inline-error ad-remove-error";
            line.setAttribute("role", "alert");
            el.after(line);
          }
          line.textContent = problem || "";
          line.hidden = !problem;
          if (!problem) host.done();
          return;
        }
      }
    });
  }

  return { merge, add, remove, addRowHtml, pageHtml, removeHtml, mount, SUBTITLE };
})();

// A top-level const isn't a window property; the panel's list looks for window.AddDevice.
if (typeof window !== "undefined") window.AddDevice = AddDevice;
if (typeof module !== "undefined") module.exports = AddDevice;
